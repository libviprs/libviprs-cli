//! `viprs` command-line interface.
//!
//! See also: [interactive CLI reference](https://libviprs.org/cli/) for runnable
//! examples and per-flag anchors used throughout the field-level documentation
//! below.

use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use clap::{ArgGroup, Parser, ValueEnum};
use libviprs::{
    BlankTileStrategy, ChecksumAlgo, CollectingObserver, DedupeStrategy, DirectoryPyramidReader,
    EngineBuilder, EngineConfig, EngineKind, FailurePolicy, FsSink, GeoCoord, GeoTransform, Layout,
    ManifestV1, MigrateOptions, PyramidPlanner, Raster, ResumeMode, RetryPolicy, TileFormat,
    extract_page_image, migrate_to_pmtiles,
    streaming::{BudgetPolicy, compute_strip_height, estimate_streaming_memory},
    streaming_mapreduce::{compute_inflight_strips, estimate_mapreduce_peak_memory},
};
// The PMTiles container is always compiled into the core crate and is never
// behind a cargo feature here. `libviprs-tests` builds this binary with
// `--release --no-default-features` for its black-box suite, so anything
// `viprs pmtiles` needs has to survive that build.
use libviprs::pmtiles::{
    Compression, Entry, FileRangeReader, RangeReader as _, Reader, TileType, directory,
    tileid_to_zxy,
};
// PDFium vector rasterisation is gated behind the `pdfium` feature (on by
// default). Without it the `--render` path is compiled out and `render_page_pdfium_with`
// does not exist in the core crate, so the import is feature-gated too.
#[cfg(feature = "pdfium")]
use libviprs::pdf::render_page_pdfium_with;

/// Per-family op registry (`CLI_CONTRACT.md` §6). The pyramid/info/plan/
/// test-image commands below stay in `main.rs` untouched; every op family is
/// additive under `src/ops/`.
mod ops;

/// `viprs features`, and the refusal a format gets when its cargo feature was
/// left out of this build (libviprs-cli#64).
mod features;

/// The single decode path every command loads an input through: SVG routing
/// and the missing-feature refusal (libviprs-cli#64).
mod input;

/// `viprs pdf`, `viprs geo` and the planner query flags (#68).
mod pdf_geo_plan;

/// The pipeline controls on `viprs pyramid`, and `viprs verify`
/// (libviprs-cli#66).
mod pipeline;
mod verify;

/// Upper bound, in megabytes, accepted for `--memory-limit` and
/// `--memory-budget`. Values above this are rejected at parse time. The cap is
/// 16 Ti MB, so the byte conversion (`mb * 1024 * 1024`) tops out at 2^54,
/// which stays well inside `u64` and can never wrap.
const MEMORY_MB_CAP: u64 = 16 * 1024 * 1024;

/// Convert a megabyte count into bytes.
///
/// Callers only pass values sourced from `--memory-limit` / `--memory-budget`,
/// which clap caps at [`MEMORY_MB_CAP`] during parsing, so the `checked_mul`
/// never returns `None`. The check is kept as a defensive guard: it turns any
/// future gap in the parse-time cap into a controlled panic instead of a silent
/// wraparound that would invert the memory guard's meaning.
fn mb_to_bytes(mb: u64) -> u64 {
    mb.checked_mul(1024 * 1024).expect(
        "memory value in MB is capped at parse time, so the byte conversion cannot overflow",
    )
}

#[derive(Parser)]
#[command(name = "viprs", about = "Generate tile pyramids from images and PDFs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Generate a tile pyramid from a PDF or image file.
    Pyramid(Box<PyramidArgs>),

    /// Show info about a PDF or image file.
    Info(InfoArgs),

    /// Show the pyramid plan without generating tiles.
    Plan(PlanArgs),

    /// Generate a synthetic test image (RGB8 gradient).
    TestImage(TestImageArgs),

    /// Inspect, read and unpack a PMTiles v3 archive.
    ///
    /// A container utility rather than a vips operation, so it lives here as a
    /// first-class built-in and never under `src/ops/` or in `OP_MAP.md`.
    Pmtiles(PmtilesArgs),

    /// List the cargo features this binary was built with, one per line.
    ///
    /// These are this crate's cargo features, not a list of capabilities. A
    /// feature that brings another with it is listed under its own name only:
    /// an `s3` build has the core's object-store sink (the core's `s3` is an
    /// alias for `object-store-sink`) and lists `s3`, not `object-store-sink`.
    ///
    /// A format whose feature is missing is refused with a message naming the
    /// feature to rebuild with; this is how to check before trying.
    Features(features::FeaturesArgs),

    /// Inspect a PDF and extract one page, with a password, background, DPI or budget.
    Pdf(pdf_geo_plan::PdfArgs),

    /// Map pixels to geographic positions and back, and find tile centres.
    Geo(pdf_geo_plan::GeoArgs),
    /// Check a finished pyramid (a PMTiles archive or a tile tree) and name
    /// any tile that is missing or damaged.
    Verify(verify::VerifyArgs),
}

#[derive(Parser)]
#[non_exhaustive]
#[command(group(
    ArgGroup::new("checksums")
        .required(false)
        .multiple(true)
        .args(["manifest_emit_checksums", "dedupe_all", "checksum"]),
))]
struct PyramidArgs {
    /// Input file (PDF, PNG, JPEG, or TIFF). Use "-" for stdin.
    input: String,

    /// Where the pyramid goes.
    ///
    /// Optional under the default `--storage pmtiles`: with no output the
    /// archive takes the input's name with a `.pmtiles` extension, so
    /// `viprs pyramid drawing.tif` writes `drawing.pmtiles`. Required for
    /// `--storage directory`, for `--packfile`, and for stdin input, none of
    /// which has a name to derive one from.
    output: Option<PathBuf>,

    /// Tile size in pixels.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-tile-size).
    #[arg(long, default_value = "256", value_parser = clap::value_parser!(u32).range(1..))]
    tile_size: u32,

    /// Tile overlap in pixels.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-overlap).
    #[arg(long, default_value = "0")]
    overlap: u32,

    /// Tile layout format.
    ///
    /// The default depends on where the pyramid is going, because the two
    /// storage backends do not address tiles the same way. `--storage pmtiles`
    /// defaults to `xyz`, which is the only addressing PMTiles v3 has;
    /// `--storage directory` defaults to `deep-zoom`, which is what this
    /// command has always written. Asking for `deep-zoom` into an archive is a
    /// usage error rather than a silent reinterpretation: a Deep Zoom tier is
    /// not a slippy zoom, and an archive built from one is addressable but
    /// renders as nonsense in every PMTiles viewer.
    ///
    /// This is not the PMTiles layout. It picks the pyramid scheme, which
    /// decides what a tile is and how it is addressed; `--pmtiles-layout`
    /// picks where the tile bytes sit inside an archive, and is a different
    /// setting altogether.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-layout).
    #[arg(long)]
    layout: Option<LayoutArg>,

    /// Where the pyramid is written: one PMTiles v3 archive, or a loose tree.
    ///
    /// `pmtiles` writes a single indexed `.pmtiles` file and is the default.
    /// `directory` restores the `{z}/{x}/{y}` (or Deep Zoom) tree earlier
    /// versions wrote, and takes an explicit output directory.
    ///
    /// Conflicts with `--sink` and `--packfile`, which name their target
    /// themselves. A `pmtiles://` URI through `--sink` is the long spelling of
    /// the default.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-storage).
    #[arg(
        long,
        default_value = "pmtiles",
        value_name = "KIND",
        conflicts_with_all = ["sink", "packfile"],
        help_heading = "Output",
    )]
    storage: StorageArg,

    /// Tile image format.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-format).
    #[arg(long, default_value = "png")]
    format: FormatArg,

    /// JPEG quality (1-100, only used with --format jpeg).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-quality).
    #[arg(long, default_value = "85", value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,

    /// DPI for PDF rendering/page-size scaling (default matches libvips).
    #[arg(long, default_value = "72", value_parser = clap::value_parser!(u32).range(1..))]
    dpi: u32,

    /// How a PDF page's pixel size comes out of --dpi. `exact` (default) is
    /// the size libvips gives, round to nearest at `dpi / 72`. `legacy-truncated`
    /// keeps the 0.5.x sizes, which are 0 to 2 px smaller for some pages.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-page-sizing).
    #[arg(long, value_enum, default_value_t = PageSizingArg::Exact)]
    page_sizing: PageSizingArg,

    /// PDF page number to extract (1-based, only used for PDF inputs).
    #[arg(long, default_value = "1", value_parser = page_number)]
    page: usize,

    /// Number of worker threads (0 = single-threaded).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-concurrency).
    #[arg(long, default_value = "0")]
    concurrency: usize,

    /// Maximum tiles buffered between producer and sink (backpressure control).
    #[arg(long, default_value = "64")]
    buffer_size: usize,

    /// Geo-reference origin as "longitude,latitude" (top-left pixel). Needs
    /// `--geo-scale`.
    #[arg(long, requires = "geo_scale", allow_hyphen_values = true)]
    geo_origin: Option<String>,

    /// Geo-reference pixel scale as "scale_x,scale_y" (degrees per pixel).
    /// Needs `--geo-origin`.
    #[arg(long, requires = "geo_origin", allow_hyphen_values = true)]
    geo_scale: Option<String>,

    /// Use PDFium for PDF rendering (required for vector PDFs).
    /// Without this flag, embedded raster images are extracted directly.
    #[arg(long)]
    render: bool,

    /// After extracting a raster from a PDF, resize it to match the PDF page
    /// dimensions at the specified --dpi. This produces output consistent with
    /// libvips' default PDF handling. Has no effect with --render.
    #[arg(long)]
    match_page_size: bool,

    /// Skip writing tiles where all pixels are identical (blank tile optimization).
    /// Mutually exclusive with --blank-tolerance (which is a strict superset).
    #[arg(long, conflicts_with = "blank_tolerance")]
    skip_blank: bool,

    /// Centre the image within the tile grid (even padding on all sides).
    #[arg(long)]
    centre: bool,

    /// Memory limit in MB for the raster pipeline. If the estimated peak
    /// memory exceeds this limit, the command exits with an error before
    /// rendering. Use 0 to disable the check (default).
    #[arg(
        long,
        default_value = "0",
        value_parser = clap::value_parser!(u64).range(..=MEMORY_MB_CAP)
    )]
    memory_limit: u64,

    /// Memory budget in megabytes for streaming pyramid generation.
    ///
    /// When set, the engine processes the image in horizontal strips instead
    /// of materialising the full canvas, reducing peak memory from O(canvas²)
    /// to O(canvas_w × strip_h). The strip height is maximised within this
    /// budget.
    ///
    /// When set to 0, the engine auto-selects: monolithic if the image fits
    /// within a default budget (1/4 of estimated monolithic peak), streaming
    /// otherwise.
    ///
    /// When omitted, the monolithic engine is used (original behavior).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-memory-budget).
    #[arg(
        long,
        value_name = "MB",
        value_parser = clap::value_parser!(u64).range(..=MEMORY_MB_CAP)
    )]
    memory_budget: Option<u64>,

    /// Use the parallel MapReduce engine for strip processing.
    ///
    /// When combined with --memory-budget, renders multiple strips concurrently
    /// (bounded by the budget) for higher throughput on multi-core systems.
    /// The --concurrency flag controls per-strip tile worker threads.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-parallel).
    #[arg(long)]
    parallel: bool,

    // -------------------------------------------------------------------------
    // Phase 3 hardening flags
    // -------------------------------------------------------------------------
    /// Sink URI: pmtiles://path.pmtiles, fs://path, or
    /// packfile://path.tar[.gz]/.zip.
    /// Defaults to the positional output as a PMTiles archive.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-sink).
    #[arg(long, value_name = "URI", help_heading = "Output")]
    sink: Option<String>,

    /// Resume from checkpoint if present.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-resume).
    #[arg(
        long,
        conflicts_with_all = ["overwrite", "verify"],
        help_heading = "Resume",
    )]
    resume: bool,

    /// Wipe the output directory and regenerate from scratch.
    ///
    /// This is the default behaviour when none of --resume, --overwrite, or
    /// --verify is supplied — running `viprs pyramid IN OUT` twice wipes
    /// `OUT` the second time and regenerates a clean pyramid.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-overwrite).
    #[arg(
        long,
        conflicts_with_all = ["resume", "verify"],
        help_heading = "Resume",
    )]
    overwrite: bool,

    /// Verify existing output against checksums rather than regenerate.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-verify).
    #[arg(
        long,
        conflicts_with_all = ["resume", "overwrite"],
        help_heading = "Resume",
    )]
    verify: bool,

    /// Manifest schema version to emit (only `1` is accepted today;
    /// anything else is rejected at parse time).
    #[arg(
        long,
        default_value = "1",
        value_name = "N",
        value_parser = clap::builder::PossibleValuesParser::new(["1"]),
        help_heading = "Manifest",
    )]
    manifest_version: String,

    /// Emit per-tile checksums into the manifest.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-manifest-emit-checksums).
    #[arg(long, help_heading = "Manifest")]
    manifest_emit_checksums: bool,

    /// Hash algorithm used for per-tile checksums (blake3 or sha256).
    /// Only meaningful in combination with --manifest-emit-checksums or
    /// --dedupe-all; clap rejects the flag otherwise.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-checksum-algo).
    #[arg(
        long,
        default_value = "blake3",
        value_name = "ALGO",
        requires = "checksums",
        help_heading = "Manifest"
    )]
    checksum_algo: ChecksumAlgoArg,

    /// If set, treat tiles within this channel delta of blank as blank
    /// (enables PlaceholderWithTolerance blank tile strategy).
    #[arg(long, value_name = "DELTA", help_heading = "Dedupe")]
    blank_tolerance: Option<u8>,

    /// How to react when a sink write fails.
    ///
    /// Accepts one of:
    /// - `fail-fast` — abort on the first error (default).
    /// - `retry=N,DURATION` — retry up to N times with initial backoff
    ///   DURATION, then abort.
    /// - `retry-skip=N,DURATION` — retry up to N times with initial backoff
    ///   DURATION, then skip the tile.
    ///
    /// DURATION is parsed with a simple ms/s/us suffix (e.g. `50ms`, `2s`).
    #[arg(
        long = "on-failure",
        default_value = "fail-fast",
        value_name = "SPEC",
        value_parser = parse_failure_policy,
        help_heading = "Reliability",
    )]
    on_failure: FailurePolicy,

    /// If set, initialise tracing-subscriber at this log level.
    /// Requires a build with `--features tracing`; otherwise the command
    /// exits with an error when this flag is supplied.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-trace-level).
    #[arg(long, value_name = "LVL")]
    trace_level: Option<String>,

    /// Shorthand for --sink packfile://<output>.tar (requires packfile feature).
    /// Conflicts with --sink, --dedupe-all, --dedupe-blanks, and
    /// --manifest-emit-checksums.
    ///
    /// The packfile sink writes a self-describing archive but does not carry
    /// the versioned per-tile checksum manifest that FsSink emits, so pairing
    /// it with --manifest-emit-checksums is rejected rather than silently
    /// dropping the checksum request.
    #[arg(
        long,
        conflicts_with_all = ["sink", "dedupe_all", "dedupe_blanks", "manifest_emit_checksums"],
        help_heading = "Output",
    )]
    packfile: bool,

    /// Deduplicate blank (uniform-colour) tiles only (DedupeStrategy::Blanks).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-dedupe-blanks).
    #[arg(long, conflicts_with = "dedupe_all", help_heading = "Dedupe")]
    dedupe_blanks: bool,

    /// Deduplicate all tiles by content hash, using --checksum-algo (mutually exclusive with --dedupe-blanks).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-dedupe-all).
    #[arg(long, conflicts_with = "dedupe_blanks", help_heading = "Dedupe")]
    dedupe_all: bool,

    #[command(flatten)]
    pipeline: pipeline::PipelineArgs,
}

#[derive(Parser)]
struct InfoArgs {
    /// PDF or image file to inspect.
    input: PathBuf,

    /// Print one JSON object instead of text: `{"v": 1, "kind": "image",
    /// "path", "width", "height", "format", "bytes"}` for an image, or
    /// `{"v": 1, "kind": "pdf", "path", "pages", "page_sizes": [{"page",
    /// "width_pts", "height_pts", "has_images"}]}` for a PDF. A failure
    /// prints nothing on stdout and exits 1 as the text form does.
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct PlanArgs {
    /// Image width in pixels (or path to an image/PDF file to read dimensions from).
    width_or_input: String,

    /// Image height in pixels (required when width is given as a number).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    height: Option<u32>,

    /// Tile size in pixels.
    #[arg(long, default_value = "256", value_parser = clap::value_parser!(u32).range(1..))]
    tile_size: u32,

    /// Tile overlap in pixels.
    #[arg(long, default_value = "0")]
    overlap: u32,

    /// Tile layout format. Besides the three `pyramid` writes, `plan` takes
    /// `zoomify` and `iiif` for its sidecar and tile-path queries.
    #[arg(long, default_value = "deep-zoom")]
    layout: pdf_geo_plan::PlanLayoutArg,

    /// DPI for PDF dimensions (only used when input is a PDF).
    #[arg(long, default_value = "72", value_parser = clap::value_parser!(u32).range(1..))]
    dpi: u32,

    /// How a PDF page's pixel size comes out of --dpi (only used when input
    /// is a PDF). `exact` (default) is the size libvips gives. `legacy-truncated`
    /// keeps the 0.5.x sizes.
    #[arg(long, value_enum, default_value_t = PageSizingArg::Exact)]
    page_sizing: PageSizingArg,

    /// PDF page number (1-based, only used when input is a PDF).
    #[arg(long, default_value = "1", value_parser = page_number)]
    page: usize,

    /// Centre the image within the tile grid (even padding on all sides).
    #[arg(long)]
    centre: bool,

    #[command(flatten)]
    queries: pdf_geo_plan::PlanQueryArgs,
}

#[derive(Parser)]
struct TestImageArgs {
    /// Output image file path.
    output: PathBuf,

    /// Image width in pixels.
    #[arg(long, default_value = "1024", value_parser = clap::value_parser!(u32).range(1..))]
    width: u32,

    /// Image height in pixels.
    #[arg(long, default_value = "1024", value_parser = clap::value_parser!(u32).range(1..))]
    height: u32,
}

/// clap `value_parser` for a 1-based `--page`: 0 names no page in any
/// document, so it is a usage mistake (exit 2) before anything is opened. A
/// page past the end of a real document is a 1, since only the file knows.
fn page_number(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(0) => Err("pages are numbered from 1".to_string()),
        Ok(page) => Ok(page),
        Err(e) => Err(e.to_string()),
    }
}

/// Refuse an overlap that leaves no tile, before any input is read.
///
/// The planner refuses it too, but only once the input has been decoded and
/// as an operational failure, and the mistake is in the command line whatever
/// the input turns out to be (#81).
fn check_overlap(tile_size: u32, overlap: u32) {
    if overlap >= tile_size {
        usage_error(
            &format!("--overlap {overlap} must be less than --tile-size {tile_size}"),
            "an overlap as wide as the tile leaves no tile; lower --overlap or raise --tile-size",
        );
    }
}

#[derive(Clone, ValueEnum)]
enum LayoutArg {
    DeepZoom,
    Xyz,
    Google,
}

impl From<LayoutArg> for Layout {
    fn from(arg: LayoutArg) -> Self {
        match arg {
            LayoutArg::DeepZoom => Layout::DeepZoom,
            LayoutArg::Xyz => Layout::Xyz,
            LayoutArg::Google => Layout::Google,
        }
    }
}

/// CLI representation of where a pyramid is stored.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StorageArg {
    /// One PMTiles v3 archive.
    Pmtiles,
    /// A loose tree of tile files.
    Directory,
}

/// Tile encodings this CLI can produce.
///
/// `webp` is lossless and has no quality knob. [`TileFormat::Webp`] is
/// fieldless in the core crate on purpose: the encoder behind it encodes
/// lossless WebP and nothing else, so a `--quality` that appeared to apply to
/// it would be an argument thrown away. `--quality` therefore stays
/// JPEG-only, and `--format webp --quality 50` writes exactly what
/// `--format webp` writes.
///
/// This list carried the opposite note until issue #60. It said there was
/// deliberately no `webp` here, because core's `TileFormat` had no WebP
/// variant and offering the choice would have advertised a capability that
/// did not exist. That was right, and libviprs#1123 removed its premise.
///
/// The half of it that survives is worth keeping: `viprs pmtiles info`
/// reported a WebP tile type, and `viprs pmtiles extract` gave those tiles the
/// right extension, for as long as this flag refused the format. Reading an
/// archive somebody else wrote was never the same claim as being able to write
/// one.
/// `--page-sizing`: the CLI face of `libviprs::PageSizing` (libviprs#1199).
#[derive(Clone, Copy, ValueEnum)]
enum PageSizingArg {
    /// The libvips size: `rint(pts * (dpi / 72.0))`, ties to even.
    Exact,
    /// The 0.5.x size: f32 truncation, then pdfium aspect-fit.
    LegacyTruncated,
}

impl From<PageSizingArg> for libviprs::PageSizing {
    fn from(a: PageSizingArg) -> Self {
        match a {
            PageSizingArg::Exact => libviprs::PageSizing::Exact,
            PageSizingArg::LegacyTruncated => libviprs::PageSizing::LegacyTruncated,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum FormatArg {
    Png,
    Jpeg,
    Raw,
    Webp,
}

#[derive(Parser)]
struct PmtilesArgs {
    #[command(subcommand)]
    command: PmtilesCommand,
}

#[derive(clap::Subcommand)]
enum PmtilesCommand {
    /// Summarise an archive: version, tile type, zoom range, counts, bounds.
    Info(PmtilesInfoArgs),

    /// Write one tile's bytes to a file or to stdout.
    Tile(PmtilesTileArgs),

    /// Check an archive's header and directory structure.
    Verify(PmtilesVerifyArgs),

    /// Unpack an archive back into a loose `{z}/{x}/{y}` tile tree.
    Extract(PmtilesExtractArgs),

    /// Pack a loose `{z}/{x}/{y}` tile tree into an archive.
    Pack(PmtilesPackArgs),
}

#[derive(Parser)]
struct PmtilesInfoArgs {
    /// The `.pmtiles` archive to summarise.
    archive: PathBuf,
}

#[derive(Parser)]
struct PmtilesTileArgs {
    /// The `.pmtiles` archive to read from.
    archive: PathBuf,

    /// Zoom level.
    z: u8,

    /// Tile column.
    x: u32,

    /// Tile row.
    y: u32,

    /// Where the tile bytes go. `-` (the default) is stdout.
    #[arg(long, short, value_name = "FILE", default_value = "-")]
    output: String,
}

#[derive(Parser)]
struct PmtilesVerifyArgs {
    /// The `.pmtiles` archive to check.
    archive: PathBuf,
}

#[derive(Parser)]
struct PmtilesExtractArgs {
    /// The `.pmtiles` archive to unpack.
    archive: PathBuf,

    /// Directory to write the `{z}/{x}/{y}.{ext}` tree into.
    output: PathBuf,
}

/// `viprs pmtiles pack`: the inverse of `extract`.
///
/// Every plan field is either read from a manifest or named on the command
/// line, and the two routes are mutually exclusive so that two descriptions of
/// one plan can never disagree. There is no third route and there must not be
/// one: see the doc on [`run_pmtiles_pack`].
#[derive(Parser)]
#[command(group(
    ArgGroup::new("plan_from_manifest")
        .arg("manifest")
        .conflicts_with("plan_explicit"),
))]
#[command(group(
    ArgGroup::new("plan_explicit")
        .args(["width", "height", "tile_size", "overlap", "layout", "format"])
        .multiple(true),
))]
struct PmtilesPackArgs {
    /// The `{z}/{x}/{y}` tile tree to pack.
    input: PathBuf,

    /// The `.pmtiles` archive to write.
    archive: PathBuf,

    /// Take the plan from the manifest.json of a `viprs pyramid` run that
    /// wrote one (for example with `--manifest-emit-checksums`).
    ///
    /// That run puts it inside the tree and beside it. With neither this nor
    /// any plan flag given, `<INPUT>/manifest.json` is used when it is there
    /// (and under 16 MiB), and the run says so.
    #[arg(long)]
    manifest: Option<PathBuf>,

    /// Source image width in pixels.
    ///
    /// Deliberately has no default. A tree cannot be packed without knowing
    /// the grid that placed it, and a default here would be a guess wearing a
    /// number.
    #[arg(long)]
    width: Option<u32>,

    /// Source image height in pixels. No default, for the same reason.
    #[arg(long)]
    height: Option<u32>,

    /// Tile size in pixels.
    #[arg(long)]
    tile_size: Option<u32>,

    /// Tile overlap in pixels.
    #[arg(long)]
    overlap: Option<u32>,

    /// Tile layout that placed the tree.
    #[arg(long)]
    layout: Option<LayoutArg>,

    /// Encoding the stored tiles are in.
    #[arg(long)]
    format: Option<FormatArg>,

    /// The tree was written with the image centred in the grid.
    ///
    /// Outside the manifest group on purpose: `GenerationSettings` records
    /// tile size, overlap, layout and format but NOT centring, so a manifest
    /// cannot answer this and a centred tree still needs to be told.
    #[arg(long)]
    centre: bool,
}

/// CLI representation of the checksum algorithm (maps to [`ChecksumAlgo`]).
#[derive(Clone, ValueEnum)]
enum ChecksumAlgoArg {
    Blake3,
    Sha256,
}

impl From<ChecksumAlgoArg> for ChecksumAlgo {
    fn from(arg: ChecksumAlgoArg) -> Self {
        match arg {
            ChecksumAlgoArg::Blake3 => ChecksumAlgo::Blake3,
            ChecksumAlgoArg::Sha256 => ChecksumAlgo::Sha256,
        }
    }
}

/// Parse a short duration literal (e.g. `50ms`, `2s`, `500us`).
fn parse_duration_literal(s: &str) -> Result<std::time::Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    // Longest suffix first so `ms` wins over `s`.
    let (num, mul_ns): (&str, u64) = if let Some(rest) = s.strip_suffix("ms") {
        (rest, 1_000_000)
    } else if let Some(rest) = s.strip_suffix("us") {
        (rest, 1_000)
    } else if let Some(rest) = s.strip_suffix("ns") {
        (rest, 1)
    } else if let Some(rest) = s.strip_suffix('s') {
        (rest, 1_000_000_000)
    } else {
        // Bare number: assume milliseconds (matches old --retry-backoff semantics).
        (s, 1_000_000)
    };
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|e| format!("invalid duration \"{s}\": {e}"))?;
    Ok(std::time::Duration::from_nanos(n.saturating_mul(mul_ns)))
}

/// clap `value_parser` for `--on-failure`.
///
/// Accepts:
/// - `fail-fast`
/// - `retry=N,DURATION`
/// - `retry-skip=N,DURATION`
fn parse_failure_policy(s: &str) -> Result<FailurePolicy, String> {
    if s == "fail-fast" {
        return Ok(FailurePolicy::FailFast);
    }

    let (kind, rest) = s.split_once('=').ok_or_else(|| {
        format!(
            "invalid --on-failure value \"{s}\": expected `fail-fast`, `retry=N,DURATION`, or `retry-skip=N,DURATION`"
        )
    })?;

    let (n_str, dur_str) = rest.split_once(',').ok_or_else(|| {
        format!("invalid --on-failure value \"{s}\": expected `{kind}=N,DURATION`")
    })?;

    let n: u32 = n_str
        .trim()
        .parse()
        .map_err(|e| format!("invalid retry count in --on-failure \"{s}\": {e}"))?;
    let backoff = parse_duration_literal(dur_str)?;

    let policy = RetryPolicy::new(n, backoff);

    match kind {
        "retry" => Ok(FailurePolicy::RetryThenFail(policy)),
        "retry-skip" => Ok(FailurePolicy::RetryThenSkip(policy)),
        other => Err(format!(
            "unknown --on-failure kind \"{other}\": expected `fail-fast`, `retry`, or `retry-skip`"
        )),
    }
}

/// The derived `viprs` command (pyramid/info/plan/test-image only), before the
/// op families and `__dump-commands` are unioned in by [`ops::assembled_cli`].
///
/// Kept as a tiny seam so the op registry can rebuild the same base without
/// depending on the private [`Cli`] type directly.
pub(crate) fn base_cli() -> clap::Command {
    use clap::CommandFactory;
    Cli::command()
}

fn main() {
    use clap::FromArgMatches;

    // In debug builds, fail loud at startup if the command registry is
    // inconsistent — a duplicate command name across families/built-ins, or a
    // command with no meta (or vice-versa) would otherwise mis-route silently
    // (`CLI_CONTRACT.md` §6).
    debug_assert!(
        ops::registry_is_consistent(),
        "CLI command registry is inconsistent (duplicate command name or command/meta mismatch)"
    );

    // Assemble the full CLI: frozen derived commands ∪ every family's commands
    // ∪ hidden `__dump-commands` (`CLI_CONTRACT.md` §6). Dispatch on the matched
    // subcommand: built-ins deserialize through the derive `Cli`; op families
    // route to their `run()`.
    // Before clap, which would report an extra positional ahead of the
    // swallowed flag, or take the flag silently when nothing follows it.
    pdf_geo_plan::refuse_swallowed_flag(&std::env::args_os().collect::<Vec<_>>());
    let matches = ops::assembled_cli().get_matches();

    match matches.subcommand() {
        Some((
            "pyramid" | "info" | "plan" | "test-image" | "pmtiles" | "features" | "pdf" | "geo"
            | "verify",
            _,
        )) => {
            let cli = Cli::from_arg_matches(&matches)
                .expect("a built-in subcommand deserializes through the derive Cli");
            match cli.command {
                Command::Pyramid(args) => run_pyramid(*args),
                Command::Info(args) => run_info(args),
                Command::Plan(args) => run_plan(args),
                Command::TestImage(args) => run_test_image(args),
                Command::Pmtiles(args) => run_pmtiles(args),
                Command::Features(args) => features::run(args),
                Command::Pdf(args) => pdf_geo_plan::run_pdf(args),
                Command::Geo(args) => pdf_geo_plan::run_geo(args),
                Command::Verify(args) => verify::run(args),
            }
        }
        Some(("__dump-commands", sub)) => ops::run_dump(sub),
        Some((name, sub)) => {
            // `CLI_CONTRACT.md` §8: clap already exited 2 for what it checks.
            // A value the op refuses on the command line alone comes back as
            // an `ops::UsageError` and is a 2 as well; everything else (the
            // input, a matrix file, the core op) is an operational failure, 1.
            if let Err(e) = ops::dispatch(name, sub) {
                if ops::is_usage_error(&e) {
                    usage_error(&format!("{e:#}"), &format!("see 'viprs {name} --help'"));
                }
                operational_error(&format!("{e:#}"));
            }
        }
        None => {
            // clap enforces the required subcommand and exits 2 before we get
            // here; this stays as a defensive usage-error path.
            let _ = ops::assembled_cli().print_help();
            process::exit(2);
        }
    }
}

/// Report a usage error and exit 2 (`CLI_CONTRACT.md` §8).
///
/// A usage error is a flag combination with no defined meaning, and every one
/// of them here names the flag that fixes it. An exit code on its own tells a
/// user that something was wrong with what they typed and nothing about what.
fn usage_error(message: &str, hint: &str) -> ! {
    eprintln!("Error: {message}");
    if !hint.is_empty() {
        eprintln!("Hint: {hint}");
    }
    process::exit(2);
}

/// Report an operational failure and exit 1 (`CLI_CONTRACT.md` §8).
fn operational_error(message: &str) -> ! {
    eprintln!("Error: {message}");
    process::exit(1);
}

/// Whether a path names a directory rather than an archive file.
///
/// Three signals, any one of which is enough: it already exists as a
/// directory, it is spelled with a trailing separator, or it carries no
/// extension at all. The last is the one that matters, because it is how every
/// example of this command was written before the storage default moved:
/// `viprs pyramid drawing.tif tiles` used to mean a directory called `tiles`
/// and would now mean an archive called `tiles`, which is almost never what
/// the person typing it wanted.
fn looks_like_a_directory_target(path: &Path) -> bool {
    let spelled = path.to_string_lossy();
    path.is_dir()
        || spelled.ends_with('/')
        || spelled.ends_with(std::path::MAIN_SEPARATOR)
        || path.extension().is_none()
}

/// Resolve the effective sink URI from flags.
///
/// Priority:
/// 1. `--packfile` shorthand  → `packfile://<output>.tar`
/// 2. `--sink <URI>`          → as-is
/// 3. `--storage directory`   → `fs://<output>`
/// 4. (none)                  → `pmtiles://<output, or the input stem>`
///
/// The fourth line is the default as of 0.4.0 and it is the one behaviour
/// change in this release: `viprs pyramid drawing.tif` writes
/// `drawing.pmtiles` where it used to write a `drawing/` tree.
///
/// `--storage`, `--packfile` and `--sink` are declared as `conflicts_with` at
/// the clap layer (a defaulted `--storage` does not trip that, only an
/// explicit one), but the `--packfile` versus `--sink` check is kept here so
/// this function is safe to call however `PyramidArgs` was built.
fn resolve_sink_uri(args: &PyramidArgs) -> String {
    if args.packfile && args.sink.is_some() {
        usage_error("--packfile and --sink are mutually exclusive", "");
    }

    if args.packfile {
        let Some(output) = args.output.as_ref() else {
            usage_error(
                "--packfile has no output path to name the archive after",
                "--packfile composes packfile://<output>.tar, so give the output \
                 positionally: `viprs pyramid drawing.tif tiles --packfile` writes tiles.tar",
            );
        };
        return format!("packfile://{}.tar", output.display());
    }

    if let Some(ref uri) = args.sink {
        return uri.clone();
    }

    match args.storage {
        StorageArg::Directory => {
            let Some(output) = args.output.as_ref() else {
                usage_error(
                    "--storage directory has no output directory to write into",
                    "a tile tree gets no invented name, so name one: \
                     `viprs pyramid drawing.tif tiles/ --storage directory`",
                )
            };
            format!("fs://{}", output.display())
        }
        StorageArg::Pmtiles => format!("pmtiles://{}", resolve_archive_path(args).display()),
    }
}

/// Where the `.pmtiles` archive goes when no sink URI named it.
///
/// With an explicit output that is the output, once it has been checked for
/// being a directory in disguise. With no output at all the archive takes the
/// input's name with a `.pmtiles` extension, which is what makes
/// `viprs pyramid drawing.tif` a complete command.
fn resolve_archive_path(args: &PyramidArgs) -> PathBuf {
    if let Some(output) = args.output.as_ref() {
        if looks_like_a_directory_target(output) {
            usage_error(
                &format!(
                    "`{}` looks like a directory, and a pyramid now goes into one PMTiles archive",
                    output.display()
                ),
                "add `--storage directory` to write a loose tile tree there, or name the \
                 archive with an extension, as in `tiles.pmtiles`",
            );
        }
        return output.clone();
    }

    if args.input == "-" {
        usage_error(
            "reading the image from stdin leaves no name to derive the archive from",
            "name the output: `viprs pyramid - drawing.pmtiles`",
        );
    }

    let input = PathBuf::from(&args.input);
    let derived = input.with_extension("pmtiles");
    if derived == input {
        usage_error(
            &format!(
                "the derived archive name for `{}` is the input itself",
                args.input
            ),
            "name a different output: `viprs pyramid in.pmtiles out.pmtiles`",
        );
    }
    derived
}

/// Resolve the tile layout, which depends on where the pyramid is going.
///
/// PMTiles v3 addresses a tile as a slippy `(z, x, y)` and has no other
/// addressing, so an archive gets `Layout::Xyz` unless `--layout` said
/// otherwise. A directory keeps the Deep Zoom default it has always had.
///
/// The refusal is the important half. `Layout::DeepZoom` numbers tiers rather
/// than slippy zooms, and `zxy_to_tileid` accepts those coordinates happily
/// because they stay inside `col < 2^level`, so nothing downstream notices.
/// The archive would be readable, addressable, and wrong in every viewer.
fn resolve_layout(args: &PyramidArgs, to_archive: bool) -> Layout {
    match (args.layout.clone(), to_archive) {
        (Some(LayoutArg::DeepZoom), true) => usage_error(
            "--layout deep-zoom cannot be addressed inside a PMTiles archive",
            "PMTiles v3 addresses tiles as slippy z/x/y, and a Deep Zoom tier is not a slippy \
             zoom. Use `--storage directory` for a Deep Zoom tree, or drop `--layout` to get \
             the xyz addressing the archive wants",
        ),
        (Some(chosen), _) => chosen.into(),
        (None, true) => Layout::Xyz,
        (None, false) => Layout::DeepZoom,
    }
}

/// Resolve the tile encoding, refusing the one PMTiles cannot carry.
///
/// A PMTiles tile is a self-describing blob a viewer hands to a decoder. Raw
/// pixel bytes carry neither their dimensions nor their pixel format, so
/// `TileType::try_from_tile_format` rejects `TileFormat::Raw` and there is no
/// tile type to write into the header. `--format raw` worked before the
/// storage default moved, so it gets a usage error naming the way out rather
/// than a sink failure part way through a run.
fn resolve_tile_format(args: &PyramidArgs, to_archive: bool) -> TileFormat {
    if to_archive && matches!(args.format, FormatArg::Raw) {
        usage_error(
            "--format raw has no tile type in a PMTiles archive",
            "a PMTiles tile is a blob a viewer hands to a decoder, and raw pixel bytes carry \
             neither their dimensions nor their pixel format. Use `--format png` or \
             `--format jpeg`, or `--storage directory` to keep writing raw tiles",
        );
    }
    match args.format {
        FormatArg::Png => TileFormat::Png,
        FormatArg::Jpeg => TileFormat::Jpeg {
            quality: args.quality,
        },
        FormatArg::Raw => TileFormat::Raw,
        FormatArg::Webp => TileFormat::Webp,
    }
}

/// Determine the [`ResumeMode`] from the three mutually-exclusive flags.
///
/// When none of `--resume`, `--overwrite`, or `--verify` is supplied the
/// default is [`ResumeMode::Overwrite`] (wipe + regenerate), matching the
/// semantics documented on `--overwrite`.
fn resolve_resume_mode(args: &PyramidArgs) -> ResumeMode {
    if args.resume {
        return ResumeMode::Resume;
    }
    if args.verify {
        return ResumeMode::Verify;
    }
    // Explicit --overwrite or no flag at all: wipe & regenerate.
    ResumeMode::Overwrite
}

/// Extract the [`FailurePolicy`] resolved by the `--on-failure` value parser.
///
/// Kept as a free function so the resolver surface stays `Args -> Config`.
fn build_failure_policy(args: &PyramidArgs) -> FailurePolicy {
    args.on_failure.clone()
}

/// Build the [`BlankTileStrategy`] from the CLI flags.
fn build_blank_tile_strategy(args: &PyramidArgs) -> BlankTileStrategy {
    if let Some(delta) = args.blank_tolerance {
        BlankTileStrategy::PlaceholderWithTolerance {
            max_channel_delta: delta,
        }
    } else if args.skip_blank {
        BlankTileStrategy::Placeholder
    } else {
        BlankTileStrategy::Emit
    }
}

/// Build the optional [`DedupeStrategy`] from the CLI flags.
fn build_dedupe_strategy(args: &PyramidArgs) -> Option<DedupeStrategy> {
    if args.dedupe_all {
        let algo: ChecksumAlgo = args.checksum_algo.clone().into();
        Some(DedupeStrategy::All { algo })
    } else if args.dedupe_blanks {
        Some(DedupeStrategy::Blanks)
    } else {
        None
    }
}

/// Initialise the tracing subscriber if `--trace-level` was provided.
///
/// If the CLI was compiled without the `tracing` feature, passing
/// `--trace-level` is a hard error rather than a silent warning.
fn maybe_init_tracing(level: &Option<String>) {
    let Some(_level) = level else { return };

    #[cfg(feature = "tracing")]
    {
        use tracing_subscriber::EnvFilter;
        // @doc-snippet:begin slot=tracing-init imports=tracing_subscriber::EnvFilter
        // stderr, like every other diagnostic: stdout carries `--events`.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            // @doc-test: phase3_tracing.rs::emits_pipeline_span:371
            .with_env_filter(EnvFilter::new(_level)) // @doc-flag: trace-level kind=param param_name=trace-level
            .init();
        // @doc-snippet:end slot=tracing-init
    }
    #[cfg(not(feature = "tracing"))]
    {
        eprintln!(
            "Error: --trace-level requires libviprs-cli built with the `tracing` feature (rebuild with `--features tracing`)."
        );
        // A feature this build left out is an operational failure, not a
        // usage mistake (README, "Exit codes").
        process::exit(1);
    }
}

fn run_pyramid(args: PyramidArgs) {
    // Ctrl-C first, before either driver is picked, so every engine below
    // stops at a tile boundary on it and the run exits 130.
    let cancel = pipeline::install_sigint();
    check_overlap(args.tile_size, args.overlap);
    if pipeline::takes_over(&args) {
        return pipeline::run(args, cancel);
    }

    // What is left is `--packfile`, and a fresh `--memory-budget` run into a
    // tile tree, on the streaming engines. Both resolve every flag before the
    // input is touched: a usage error must not depend on whether the file
    // happens to decode, and a refused run must not have written anything.
    let start = Instant::now();
    let sink_uri = resolve_sink_uri(&args);
    let layout = resolve_layout(&args, false);
    let tile_format = resolve_tile_format(&args, false);

    let pipeline::Prepared { raster, plan } = pipeline::prepare(&args, layout);
    let engine_config = pipeline::engine_config(&args);
    let events = pipeline::EventPrinter::new(pipeline::EventsArg::None);

    let (result, output, resumable) = if let Some(path) = sink_uri.strip_prefix("packfile://") {
        let (result, output) = run_pyramid_packfile(
            path,
            &raster,
            &plan,
            tile_format,
            engine_config,
            resolve_resume_mode(&args),
            cancel,
        );
        (result, output, pipeline::Resumable::No)
    } else {
        let dir = pipeline::tree_dir(&args, &sink_uri);
        let sink = pipeline::tree_sink(&args, &dir, &plan, tile_format);
        let result = run_generate(&args, &raster, &plan, &sink, engine_config, cancel);
        (result, dir, pipeline::Resumable::Yes)
    };
    pipeline::conclude(result, &output, start, resumable, &events);
}

/// Entry point for the streaming / mapreduce generation paths behind
/// `--memory-budget` (filesystem sink only). Hands the engine's answer back for
/// [`pipeline::conclude`] to turn into the summary and the exit code, so a
/// Ctrl-C here exits 130 like every other run.
///
/// Routes through [`EngineBuilder`] so the CLI never constructs a
/// `StreamingConfig` / `MapReduceConfig` / free-function call by hand —
/// every knob flows through a single typed builder.
fn run_generate(
    args: &PyramidArgs,
    raster: &Raster,
    plan: &libviprs::PyramidPlan,
    sink: &FsSink,
    engine_config: EngineConfig,
    cancel: libviprs::CancelToken,
) -> Result<libviprs::EngineResult, libviprs::EngineError> {
    let observer = CollectingObserver::new();

    // Pick the engine kind + memory budget up-front so the diagnostic logging
    // and the builder share a single decision point.
    let (engine_kind, memory_budget) = match args.memory_budget {
        None => (EngineKind::Monolithic, None),
        Some(budget_mb) => {
            let budget_bytes = if budget_mb == 0 {
                let mono_est = plan.estimate_peak_memory_for_format(raster.format());
                mono_est / 4
            } else {
                mb_to_bytes(budget_mb)
            };
            let mono_est = plan.estimate_peak_memory_for_format(raster.format());

            if args.parallel {
                if mono_est <= budget_bytes {
                    eprintln!(
                        "MapReduce: budget {:.1} MB >= monolithic peak {:.1} MB, using monolithic engine",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        mono_est as f64 / (1024.0 * 1024.0),
                    );
                } else {
                    let strip_h = compute_strip_height(plan, raster.format(), budget_bytes);
                    let sh = strip_h.unwrap_or(2 * args.tile_size);
                    // Mirror the engine's channel-backlog charge (issue #103 in
                    // core): with tile workers enabled (--concurrency > 0), the
                    // parallel emission path holds up to `buffer_size +
                    // concurrency` decoded tiles in its bounded channel. Charge
                    // the same backlog here so this diagnostic matches what
                    // `generate_pyramid_mapreduce` will actually compute from
                    // the --memory-budget the user supplied.
                    let channel_bytes = if engine_config.concurrency > 0 {
                        let tile_bytes = plan.tile_size as u64
                            * plan.tile_size as u64
                            * raster.format().bytes_per_pixel() as u64;
                        (engine_config.buffer_size as u64 + engine_config.concurrency as u64)
                            * tile_bytes
                    } else {
                        0
                    };
                    let inflight = compute_inflight_strips(
                        plan,
                        raster.format(),
                        sh,
                        channel_bytes,
                        budget_bytes,
                    );
                    let est = estimate_mapreduce_peak_memory(
                        plan,
                        raster.format(),
                        sh,
                        inflight,
                        channel_bytes,
                    );
                    eprintln!(
                        "MapReduce: budget {:.1} MB, strip_height={}, {} in-flight strips, estimated peak {:.1} MB",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        strip_h.map_or("min".to_string(), |h| format!("{h}")),
                        inflight,
                        est as f64 / (1024.0 * 1024.0),
                    );
                }
                (EngineKind::MapReduce, Some(budget_bytes))
            } else {
                if mono_est <= budget_bytes {
                    eprintln!(
                        "Streaming: budget {:.1} MB >= monolithic peak {:.1} MB, using monolithic engine",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        mono_est as f64 / (1024.0 * 1024.0),
                    );
                } else {
                    let strip_h = compute_strip_height(plan, raster.format(), budget_bytes);
                    let est =
                        strip_h.map(|sh| estimate_streaming_memory(plan, raster.format(), sh));
                    eprintln!(
                        "Streaming: budget {:.1} MB, strip_height={}, estimated peak {:.1} MB",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        strip_h.map_or("min".to_string(), |h| format!("{h}")),
                        est.unwrap_or(0) as f64 / (1024.0 * 1024.0),
                    );
                }
                (EngineKind::Streaming, Some(budget_bytes))
            }
        }
    };

    // Build once, run once. Every knob goes through typed setters.
    // @doc-snippet:begin slot=engine-builder imports=EngineBuilder,EngineKind,CollectingObserver,BudgetPolicy,ResumePolicy
    let mut builder = EngineBuilder::new(raster, plan.clone(), sink)
        // @doc-test: builder_match_composition.rs::engine_kind_through_match:100
        .with_engine(engine_kind) // @doc-flag: parallel kind=appendChain
        .with_observer(observer)
        .with_concurrency(engine_config.concurrency)
        .with_buffer_size(engine_config.buffer_size)
        .with_background_rgb(engine_config.background_rgb)
        .with_blank_strategy(engine_config.blank_tile_strategy)
        .with_failure_policy(engine_config.failure_policy.clone())
        .with_cancel(cancel);
    if let Some(ds) = engine_config.dedupe_strategy {
        builder = builder.with_dedupe(ds);
    }
    if let Some(bytes) = memory_budget {
        builder = builder
            // @doc-test: builder_engine_streaming.rs::with_memory_budget_drives_strip_height:92
            .with_memory_budget(bytes) // @doc-flag: memory-budget kind=appendChain
            .with_budget_policy(BudgetPolicy::Error);
    }
    // @doc-test: builder_resume_matrix.rs::monolithic_resume_with_raster_source:110
    // @doc-flag: resume kind=appendChain
    // @doc-test: builder_resume_matrix.rs::monolithic_overwrite_with_raster_source:94
    // @doc-flag: overwrite kind=appendChain
    // @doc-test: builder_resume_matrix.rs::monolithic_verify_with_raster_source:134
    // @doc-flag: verify kind=appendChain

    let result = builder.run();
    // @doc-snippet:end slot=engine-builder
    result
}

/// Print the post-run summary line.
fn finish_run(result: libviprs::EngineResult, output: &std::path::Path, start: Instant) {
    // @doc-snippet:begin slot=finish imports=libviprs::EngineResult
    let elapsed = start.elapsed();
    let mut summary = format!(
        "Done: {} tiles, {} levels, peak memory {:.1} MB, {:.2}s",
        result.tiles_produced,
        result.levels_processed,
        result.peak_memory_bytes as f64 / (1024.0 * 1024.0),
        elapsed.as_secs_f64()
    );
    if result.tiles_skipped > 0 {
        summary.push_str(&format!(" ({} blank tiles skipped)", result.tiles_skipped));
    }
    eprintln!("{summary}");
    eprintln!("Output: {}", output.display());
    // @doc-snippet:end slot=finish
}

// ---------------------------------------------------------------------------
// The `viprs pmtiles` group
// ---------------------------------------------------------------------------

/// How deep a chain of leaf directories a walk will follow before refusing.
///
/// A leaf pointing at a leaf is legal and two levels is the most any writer
/// here produces, so four is slack rather than a limit anybody reaches. It is
/// a limit and not an assertion because the archive is somebody else's file
/// and a cycle in it must cost a refusal rather than the process.
const MAX_LEAF_DEPTH: u8 = 4;

/// Cap on a decompressed leaf directory.
///
/// No length field in PMTiles v3 is an uncompressed length, so a reader cannot
/// pre-size the buffer and has to cap it instead. A leaf of the 21844 entries
/// the writer targets is well under a megabyte.
const MAX_LEAF_DECOMPRESSED: usize = 32 * 1024 * 1024;

/// Cap on a decompressed tile payload, for the archives that store compressed
/// tiles. PNG and JPEG are stored as they are and never reach this.
const MAX_TILE_DECOMPRESSED: usize = 64 * 1024 * 1024;

/// What a walk over an archive's directories found.
#[derive(Default)]
struct ArchiveWalk {
    tile_entries: u64,
    addressed_tiles: u64,
    leaf_directories: u64,
    /// Everything structurally wrong, in the order it was met. Collected
    /// rather than returned one at a time so `verify` can report an archive's
    /// problems in one pass instead of one per run.
    problems: Vec<String>,
}

/// Open an archive for reading, or exit 1 saying why not.
fn open_archive(path: &Path) -> Reader<FileRangeReader> {
    match Reader::try_open(path) {
        Ok(reader) => reader,
        Err(e) => operational_error(&format!("reading {}: {e}", path.display())),
    }
}

/// Read one leaf directory's entries.
///
/// The offset base is the part that is easy to get wrong and impossible to
/// notice: a leaf entry's `offset` is relative to the leaf *directories*
/// section, and a tile entry inside that leaf is relative to the *tile data*
/// section, not to the leaf it was found in. A writer and a reader that make
/// the same wrong choice round-trip perfectly.
fn read_leaf_directory(
    reader: &Reader<FileRangeReader>,
    entry: &Entry,
) -> Result<Vec<Entry>, String> {
    let header = reader.header();
    let length = usize::try_from(entry.length).map_err(|_| {
        format!(
            "leaf at tile {} has a length that does not fit in memory",
            entry.tile_id
        )
    })?;
    let end = entry
        .offset
        .checked_add(u64::from(entry.length))
        .ok_or_else(|| format!("leaf at tile {} overflows its own end", entry.tile_id))?;
    if end > header.leaf_directories_length {
        return Err(format!(
            "leaf at tile {} runs to {end}, past the {}-byte leaf directories section",
            entry.tile_id, header.leaf_directories_length
        ));
    }
    let at = header
        .leaf_directories_offset
        .checked_add(entry.offset)
        .ok_or_else(|| format!("leaf at tile {} overflows the archive", entry.tile_id))?;
    let raw = reader
        .source()
        .read_range(at, length)
        .map_err(|e| format!("leaf at tile {}: {e}", entry.tile_id))?;
    let bytes = header
        .internal_compression
        .decompress(&raw, MAX_LEAF_DECOMPRESSED)
        .map_err(|e| format!("leaf at tile {}: {e}", entry.tile_id))?;
    directory::deserialize_entries(&bytes)
        .map_err(|e| format!("leaf at tile {}: {e}", entry.tile_id))
}

/// Walk every directory in the archive, calling `on_tile` for each tile entry.
///
/// `Reader` answers single-tile questions and keeps its leaf cache to itself,
/// so the whole-archive routes (`verify`, `extract`) walk the tree here. One
/// leaf is held at a time and the pending list holds directories rather than
/// entries, so an archive with millions of tiles costs a page at a time.
fn walk_archive(
    reader: &Reader<FileRangeReader>,
    on_tile: &mut dyn FnMut(&Entry) -> Result<(), String>,
) -> ArchiveWalk {
    let header = reader.header();
    let mut walk = ArchiveWalk::default();
    let mut pending: Vec<(Vec<Entry>, u8)> = vec![(reader.root_entries().to_vec(), 0)];

    while let Some((entries, depth)) = pending.pop() {
        // Sorted, non-overlapping ids are what makes the lookup a binary
        // search. A directory that breaks either silently returns a
        // neighbouring tile's bytes, which decode fine and look right.
        let mut previous_end: Option<u64> = None;
        for entry in &entries {
            if let Some(end) = previous_end
                && entry.tile_id < end
            {
                walk.problems.push(format!(
                    "entry {} starts inside the run that ends at {end}",
                    entry.tile_id
                ));
            }
            previous_end = Some(
                entry
                    .tile_id
                    .saturating_add(u64::from(entry.run_length.max(1))),
            );

            if entry.is_leaf() {
                walk.leaf_directories += 1;
                if depth >= MAX_LEAF_DEPTH {
                    walk.problems.push(format!(
                        "leaf chain at tile {} is deeper than the {MAX_LEAF_DEPTH} levels this build follows",
                        entry.tile_id
                    ));
                    continue;
                }
                match read_leaf_directory(reader, entry) {
                    Ok(child) => pending.push((child, depth + 1)),
                    Err(problem) => walk.problems.push(problem),
                }
                continue;
            }

            walk.tile_entries += 1;
            walk.addressed_tiles += u64::from(entry.run_length);

            // The check the reference implementation does not make. Its own
            // `verify` never adds the length to the offset, which is how it
            // walks past a root offset of 999999 in an 1878-byte file.
            match entry.offset.checked_add(u64::from(entry.length)) {
                None => walk.problems.push(format!(
                    "tile {} has an offset and length that overflow",
                    entry.tile_id
                )),
                Some(end) if end > header.tile_data_length => walk.problems.push(format!(
                    "tile {} runs to {end}, past the {}-byte tile data section",
                    entry.tile_id, header.tile_data_length
                )),
                Some(_) => {}
            }

            if let Err(problem) = on_tile(entry) {
                walk.problems.push(problem);
            }
        }
    }

    walk
}

/// A tile type's name for display: its extension where it has one, and its own
/// spelling where it does not, so an unrecognised byte is reported rather than
/// flattened into "unknown".
fn tile_type_name(tile_type: TileType) -> String {
    match tile_type.extension() {
        Some(ext) => ext.to_string(),
        None => format!("{tile_type:?}").to_lowercase(),
    }
}

/// A compression's name for display.
fn compression_name(compression: Compression) -> String {
    format!("{compression:?}").to_lowercase()
}

/// Read one tile entry's stored payload.
fn read_tile_payload(reader: &Reader<FileRangeReader>, entry: &Entry) -> Result<Vec<u8>, String> {
    let header = reader.header();
    let length = usize::try_from(entry.length).map_err(|_| {
        format!(
            "tile {} has a length that does not fit in memory",
            entry.tile_id
        )
    })?;
    let at = header
        .tile_data_offset
        .checked_add(entry.offset)
        .ok_or_else(|| format!("tile {} overflows the archive", entry.tile_id))?;
    reader
        .source()
        .read_range(at, length)
        .map_err(|e| format!("tile {}: {e}", entry.tile_id))
}

/// Route the `viprs pmtiles` group.
fn run_pmtiles(args: PmtilesArgs) {
    match args.command {
        PmtilesCommand::Info(a) => run_pmtiles_info(a),
        PmtilesCommand::Tile(a) => run_pmtiles_tile(a),
        PmtilesCommand::Verify(a) => run_pmtiles_verify(a),
        PmtilesCommand::Extract(a) => run_pmtiles_extract(a),
        PmtilesCommand::Pack(a) => run_pmtiles_pack(a),
    }
}

/// `viprs pmtiles info <archive>`: what the archive says about itself.
fn run_pmtiles_info(args: PmtilesInfoArgs) {
    let reader = open_archive(&args.archive);
    let header = reader.header();

    println!("Archive: {}", args.archive.display());
    println!("Version: 3");
    println!("Tile type: {}", tile_type_name(header.tile_type));
    println!(
        "Tile compression: {}",
        compression_name(header.tile_compression)
    );
    println!(
        "Internal compression: {}",
        compression_name(header.internal_compression)
    );
    println!("Clustered: {}", if header.clustered { "yes" } else { "no" });
    println!("Zoom: {}-{}", header.min_zoom, header.max_zoom);
    println!("Addressed tiles: {}", header.addressed_tiles_count);
    println!("Tile entries: {}", header.tile_entries_count);
    println!("Unique payloads: {}", header.tile_contents_count);
    println!("Root entries: {}", reader.root_entries().len());
    println!(
        "Leaf directories: {}",
        if header.has_leaves() { "yes" } else { "no" }
    );
    match reader.archive_size() {
        Some(size) => println!("Archive size: {size} bytes"),
        None => println!("Archive size: unknown"),
    }
    let (west, south, east, north) = header.bounds_degrees();
    println!("Bounds: {west:.6},{south:.6},{east:.6},{north:.6}");
    let (lon, lat) = header.center_degrees();
    println!("Center: {lon:.6},{lat:.6} zoom {}", header.center_zoom);

    // The metadata is somebody else's JSON and a parse failure in it is not a
    // reason to fail the summary, so it is reported rather than fatal.
    match reader.metadata() {
        Ok(metadata) => match metadata.to_json() {
            Ok(json) => println!("Metadata: {}", String::from_utf8_lossy(&json)),
            Err(e) => eprintln!("Warning: the metadata did not re-serialise: {e}"),
        },
        Err(e) => eprintln!("Warning: the metadata did not parse: {e}"),
    }
}

/// `viprs pmtiles tile <archive> <z> <x> <y>`: one tile's bytes, and nothing else.
///
/// stdout carries the tile and only the tile. Every diagnostic in this function
/// goes to stderr, including the one for a tile that is not there, because a
/// caller piping this into a decoder must not get a sentence where the bytes
/// should be.
fn run_pmtiles_tile(args: PmtilesTileArgs) {
    let reader = open_archive(&args.archive);

    let bytes = match reader.get_tile(args.z, args.x, args.y) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => operational_error(&format!(
            "{}/{}/{} is not in {}",
            args.z,
            args.x,
            args.y,
            args.archive.display()
        )),
        Err(e) => operational_error(&format!(
            "reading {}/{}/{} from {}: {e}",
            args.z,
            args.x,
            args.y,
            args.archive.display()
        )),
    };

    if args.output == "-" {
        let mut stdout = std::io::stdout().lock();
        if let Err(e) = stdout.write_all(&bytes).and_then(|()| stdout.flush()) {
            operational_error(&format!("writing the tile to stdout: {e}"));
        }
        return;
    }

    if let Err(e) = std::fs::write(&args.output, &bytes) {
        operational_error(&format!("writing the tile to {}: {e}", args.output));
    }
    eprintln!("Wrote {} bytes to {}", bytes.len(), args.output);
}

/// `viprs pmtiles verify <archive>`: structural and index validation.
///
/// Deliberately stricter than the reference implementation in one place:
/// go-pmtiles' own `verify` never adds an entry's length to its offset, so it
/// accepts entries that address bytes the archive does not have. Matching that
/// would be more compatible and less useful.
fn run_pmtiles_verify(args: PmtilesVerifyArgs) {
    let reader = open_archive(&args.archive);
    let header = reader.header();

    let mut walk = walk_archive(&reader, &mut |_entry| Ok(()));

    // The header's own counts are part of the archive, so they are part of what
    // there is to verify. A writer that miscounts produces an archive every
    // reader still reads, and nothing else would ever notice.
    if walk.tile_entries != header.tile_entries_count {
        walk.problems.push(format!(
            "the header claims {} tile entries and the directories hold {}",
            header.tile_entries_count, walk.tile_entries
        ));
    }
    if walk.addressed_tiles != header.addressed_tiles_count {
        walk.problems.push(format!(
            "the header claims {} addressed tiles and the runs cover {}",
            header.addressed_tiles_count, walk.addressed_tiles
        ));
    }

    if !walk.problems.is_empty() {
        eprintln!(
            "{}: {} problems",
            args.archive.display(),
            walk.problems.len()
        );
        for problem in &walk.problems {
            eprintln!("  {problem}");
        }
        process::exit(1);
    }

    println!("Archive: {}", args.archive.display());
    println!("Root entries: {}", reader.root_entries().len());
    println!("Leaf directories: {}", walk.leaf_directories);
    println!("Tile entries: {}", walk.tile_entries);
    println!("Addressed tiles: {}", walk.addressed_tiles);
    println!("OK");
}

/// `viprs pmtiles extract <archive> <dir>`: back to a loose tile tree.
///
/// The inverse of `viprs pyramid --storage directory --layout xyz` on the same
/// input, which is what makes it a compatibility route rather than a debug
/// aid: an archive somebody hands you turns into the tree every existing tool
/// already knows how to serve.
fn run_pmtiles_extract(args: PmtilesExtractArgs) {
    let reader = open_archive(&args.archive);
    let header = reader.header();

    let Some(extension) = header.tile_type.extension() else {
        operational_error(&format!(
            "{} does not say what its tiles are ({}), so there is no file extension to give them",
            args.archive.display(),
            tile_type_name(header.tile_type)
        ));
    };
    let compression = header.tile_compression;

    if let Err(e) = std::fs::create_dir_all(&args.output) {
        operational_error(&format!("creating {}: {e}", args.output.display()));
    }

    let mut written: u64 = 0;
    let walk = walk_archive(&reader, &mut |entry| {
        let stored = read_tile_payload(&reader, entry)?;
        let bytes = if matches!(compression, Compression::None) {
            stored
        } else {
            compression
                .decompress(&stored, MAX_TILE_DECOMPRESSED)
                .map_err(|e| format!("tile {}: {e}", entry.tile_id))?
        };

        // A run covers consecutive tile ids sharing one payload, which is how
        // the archive stores a deduplicated blank: every coordinate in the run
        // gets its own file back.
        for step in 0..u64::from(entry.run_length) {
            let tile_id = entry
                .tile_id
                .checked_add(step)
                .ok_or_else(|| format!("run at tile {} overflows", entry.tile_id))?;
            let (z, x, y) = tileid_to_zxy(tile_id)
                .map_err(|e| format!("tile id {tile_id} is not a coordinate: {e}"))?;
            let dir = args.output.join(z.to_string()).join(x.to_string());
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("creating {}: {e}", dir.display()))?;
            let path = dir.join(format!("{y}.{extension}"));
            std::fs::write(&path, &bytes)
                .map_err(|e| format!("writing {}: {e}", path.display()))?;
            written += 1;
        }
        Ok(())
    });

    if !walk.problems.is_empty() {
        eprintln!(
            "{}: {} problems",
            args.archive.display(),
            walk.problems.len()
        );
        for problem in &walk.problems {
            eprintln!("  {problem}");
        }
        process::exit(1);
    }

    println!("Extracted {written} tiles to {}", args.output.display());
}

/// `viprs pmtiles pack <tree> <archive>`: the inverse of `extract`.
///
/// **This command will not guess a plan, and this is where somebody proposes a
/// `--guess` flag.** `DirectoryPyramidReader::try_open` refuses to infer one
/// because a directory of tiles does not say what its level indices mean, how
/// big a tile is, or which layout placed it. Guess wrong about the layout, the
/// level base, the tile size or the row axis and every tile lands at the wrong
/// tile id, producing a structurally perfect archive that `viprs pmtiles
/// verify` passes and that a round trip through our own reader cannot catch,
/// because the reader resolves through the same wrong plan. Getting the row
/// axis wrong flips y, and the archive looks right until it is opened over a
/// real basemap. See libviprs#1118.
///
/// So the plan arrives one of two ways and they are mutually exclusive at parse
/// time: a manifest, which is reporting rather than guessing, or the explicit
/// flags, where `--width` and `--height` have no defaults so the command cannot
/// run blind.
///
/// The refusals ask the library's own gates rather than restating them, so
/// they cannot drift: `layout_is_zxy` for a layout PMTiles cannot address,
/// `TileType::try_from_tile_format` for `TileFormat::Raw`, and
/// `tile_coord_to_zxy` on the top level's far corner for a plan too deep to
/// address. They run before the tree is opened, which is what makes a bad plan
/// on the command line a usage error (2) rather than a failure halfway into a
/// walk of billions of coordinates. A plan out of a manifest that fails the
/// same gates is an operational failure (1).
///
/// Packing nothing is a failure too. A wrong `--format` or the wrong
/// directory visits every coordinate, finds nothing, and the writer would
/// publish a valid empty archive; this exits 1 instead and removes that
/// archive. Absent tiles and tile-shaped files the plan never visited are
/// warnings, because a `--skip-blank` tree legitimately has holes.
///
/// An interrupted pack simply restarts: the writer stages to a temp file and
/// renames, so a half-done archive never wears the final name.
fn run_pmtiles_pack(args: PmtilesPackArgs) {
    // Where the plan came from decides what a bad plan is. Named on the
    // command line it is the person's typo and a usage error (2); read out of
    // a manifest it is a file that does not say what it should (1).
    enum PlanSource {
        Flags,
        Manifest(PathBuf),
        TreeManifest(PathBuf),
    }

    let explicit = args.width.is_some()
        || args.height.is_some()
        || args.tile_size.is_some()
        || args.overlap.is_some()
        || args.layout.is_some()
        || args.format.is_some();
    let source = match args.manifest.clone() {
        Some(path) => PlanSource::Manifest(path),
        None if !explicit => match manifest_inside_tree(&args.input) {
            Some(path) => PlanSource::TreeManifest(path),
            None => PlanSource::Flags,
        },
        None => PlanSource::Flags,
    };
    let from_flags = matches!(source, PlanSource::Flags);
    let refuse = |message: &str, hint: &str| refuse_pack_plan(from_flags, message, hint);

    let (width, height, tile_size, overlap, layout, format) = match &source {
        PlanSource::Manifest(path) | PlanSource::TreeManifest(path) => {
            let manifest = match ManifestV1::read_from(path) {
                Ok(m) => m,
                Err(e) => operational_error(&format!("reading {}: {e}", path.display())),
            };
            (
                manifest.source.width,
                manifest.source.height,
                manifest.generation.tile_size,
                manifest.generation.overlap,
                manifest.generation.layout,
                manifest.generation.format,
            )
        }
        PlanSource::Flags => {
            let (Some(width), Some(height)) = (args.width, args.height) else {
                usage_error(
                    "pack needs the grid that placed the tree, and --width/--height were not given",
                    "pass --width and --height (plus --tile-size, --overlap, --layout and --format \
                     if they were not the defaults), or point --manifest at the manifest.json a \
                     `viprs pyramid` run wrote (for example with --manifest-emit-checksums). \
                     There is no inference here on purpose: a wrong plan produces an archive that \
                     verifies and is still wrong",
                );
            };
            let layout: Layout = args.layout.unwrap_or(LayoutArg::Xyz).into();
            let format = match args.format.unwrap_or(FormatArg::Png) {
                // Quality does not describe an existing tile, it describes one
                // being encoded, and pack copies payloads through byte for
                // byte. The library reads the tile type off this and nothing
                // re-encodes, so the number is inert.
                FormatArg::Jpeg => TileFormat::Jpeg { quality: 80 },
                FormatArg::Png => TileFormat::Png,
                FormatArg::Raw => TileFormat::Raw,
                FormatArg::Webp => TileFormat::Webp,
            };
            (
                width,
                height,
                args.tile_size.unwrap_or(256),
                args.overlap.unwrap_or(0),
                layout,
                format,
            )
        }
    };

    let planner = match PyramidPlanner::new(width, height, tile_size, overlap, layout) {
        Ok(p) => p.with_centre(args.centre),
        Err(e) => refuse(
            &format!("that plan does not describe a pyramid: {e}"),
            "check --width, --height, --tile-size and --overlap",
        ),
    };

    // The three refusals below are the library's own gates, asked up front
    // rather than restated, so they cannot drift from what the writer checks.
    // Asking them here, before anything is opened, is what makes them usage
    // errors instead of a failure halfway into a walk.
    if !libviprs::sink_pmtiles::layout_is_zxy(layout) {
        refuse(
            &format!(
                "the {} layout is not addressed by (z, x, y), so PMTiles has no tile ids for it",
                layout_name(layout)
            ),
            "only an xyz or google tree can be packed",
        );
    }
    if let Err(e) = libviprs::pmtiles::TileType::try_from_tile_format(format) {
        refuse(
            &format!(
                "--format {} cannot go in an archive: {e}",
                format_name(format)
            ),
            "PMTiles stores encoded tiles; pack a png, jpeg or webp tree",
        );
    }
    let plan = planner.plan();
    // Every other level is smaller than the top one, so the top level's far
    // corner is the one coordinate that decides whether the whole plan is
    // addressable. Checked before the walk: `--width 4294967295 --tile-size 1`
    // plans fine and would otherwise stat billions of absent coordinates
    // before the writer ever saw a tile it could refuse.
    if let Some(top) = plan.levels.iter().max_by_key(|l| l.level) {
        let corner = libviprs::planner::TileCoord {
            level: top.level,
            col: top.cols.saturating_sub(1),
            row: top.rows.saturating_sub(1),
        };
        if let Err(e) = libviprs::sink_pmtiles::tile_coord_to_zxy(corner) {
            refuse(
                &format!(
                    "that plan's top level is zoom {} with {}x{} tiles, which PMTiles cannot \
                     address: {e}",
                    top.level, top.cols, top.rows
                ),
                "check --width, --height and --tile-size for a slipped digit",
            );
        }
    }

    match &source {
        PlanSource::Flags => println!("Plan (from the command line):"),
        PlanSource::Manifest(path) => println!("Plan (from {}):", path.display()),
        PlanSource::TreeManifest(path) => {
            println!("Plan (from {}, found in the tree):", path.display())
        }
    }
    println!("  width x height      : {width} x {height}");
    println!("  tile size           : {tile_size}");
    println!("  overlap             : {overlap}");
    println!("  layout              : {}", layout_name(layout));
    println!("  format              : {}", format_name(format));
    println!(
        "  centre              : {}",
        if args.centre { "yes" } else { "no" }
    );

    let inner = match DirectoryPyramidReader::try_open(&args.input, plan.clone(), format) {
        Ok(r) => r,
        Err(e) => operational_error(&format!("opening {}: {e}", args.input.display())),
    };

    // Counted before the walk, so a tree with nothing of this format in it is
    // refused before an archive exists, and so the walk's own counts can be
    // held against what is really on disk afterwards.
    let census = tile_shaped_files(&args.input, format.extension());
    if census.matching == 0 {
        operational_error(&format!(
            "no tiles to pack: {} holds no {{z}}/{{x}}/{{y}}.{} files{}",
            args.input.display(),
            format.extension(),
            census.describe_others(),
        ));
    }

    let reader = GuardedTreeReader::new(inner, tile_size, overlap);
    let report = match migrate_to_pmtiles(&reader, &plan, &args.archive, MigrateOptions::default())
    {
        Ok(r) => r,
        Err(e) => operational_error(&format!("packing {}: {e}", args.input.display())),
    };

    if report.tiles_written == 0 {
        // The writer has already published a valid, empty archive under the
        // final name. It is this run's own output, and leaving it there is how
        // an empty archive ends up being served.
        let _ = std::fs::remove_file(&report.out_path);
        operational_error(&format!(
            "no tiles were packed: the plan visited {} coordinates and found none of the {} \
             .{} files in {}, so the plan does not match the tree{}",
            report.coords_visited,
            census.matching,
            format.extension(),
            args.input.display(),
            census.describe_others(),
        ));
    }

    println!("Wrote {}", report.out_path.display());
    println!("  coordinates visited : {}", report.coords_visited);
    println!("  tiles written       : {}", report.tiles_written);
    println!("  tiles absent        : {}", report.tiles_absent);
    println!("  distinct payloads   : {}", report.distinct_payloads);
    println!(
        "  tile format         : {}",
        format_name(report.tile_format)
    );

    // Warnings, not failures: a --skip-blank tree legitimately has holes, and
    // the archive that was written is exactly what the plan found.
    if report.tiles_absent > 0 {
        eprintln!(
            "Warning: {} of the {} tiles the plan names were absent from the tree. That is \
             expected for a tree written with --skip-blank; otherwise check the plan.",
            report.tiles_absent, report.coords_visited
        );
    }
    let unvisited = census.matching.saturating_sub(report.tiles_written);
    if unvisited > 0 {
        eprintln!(
            "Warning: {unvisited} tile-shaped .{} file(s) in the tree were never visited by the \
             plan and are not in the archive, so the plan probably does not match the tree.",
            format.extension()
        );
    }
    if !census.others.is_empty() {
        eprintln!(
            "Warning: the tree also holds tile-shaped files in another format, which were not \
             packed{}",
            census.describe_others()
        );
    }
}

/// Refuse a pack plan as a usage error when it came off the command line, or
/// as an operational failure when a manifest supplied it.
fn refuse_pack_plan(from_flags: bool, message: &str, hint: &str) -> ! {
    if from_flags {
        usage_error(message, hint)
    } else {
        operational_error(message)
    }
}

/// The largest `manifest.json` pack will pick up from inside a tree on its
/// own. A real one is a few hundred bytes, plus a checksum line per tile when
/// checksums were asked for, so this is generous; past it the file is not
/// read unasked, and `--manifest` still takes it explicitly.
const TREE_MANIFEST_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// `<tree>/manifest.json`, when it is a regular file small enough to read
/// unasked. A link, a directory or an oversized file is passed over with a
/// warning rather than followed.
fn manifest_inside_tree(tree: &Path) -> Option<PathBuf> {
    let path = tree.join("manifest.json");
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        eprintln!(
            "Warning: {} is not a regular file, so it was not used as the plan",
            path.display()
        );
        return None;
    }
    if meta.len() > TREE_MANIFEST_MAX_BYTES {
        eprintln!(
            "Warning: {} is {} bytes, more than pack reads unasked; pass it with --manifest to use it",
            path.display(),
            meta.len()
        );
        return None;
    }
    Some(path)
}

fn layout_name(layout: Layout) -> String {
    format!("{layout:?}").to_lowercase()
}

fn format_name(format: TileFormat) -> &'static str {
    match format {
        TileFormat::Png => "png",
        TileFormat::Jpeg { .. } => "jpeg",
        TileFormat::Raw => "raw",
        TileFormat::Webp => "webp",
    }
}

/// What a tree holds that is shaped like a `{z}/{x}/{y}.{ext}` tile.
struct TileCensus {
    /// Files under the extension the plan reads.
    matching: u64,
    /// Files under any other extension, by extension.
    others: std::collections::BTreeMap<String, u64>,
}

impl TileCensus {
    fn describe_others(&self) -> String {
        if self.others.is_empty() {
            return String::new();
        }
        let listed: Vec<String> = self
            .others
            .iter()
            .map(|(ext, n)| format!("{n} .{ext}"))
            .collect();
        format!(
            " (it does hold {} tile-shaped files: pass the --format they are in)",
            listed.join(", ")
        )
    }
}

/// Count the tile-shaped files under `root` without following a link.
///
/// Three levels deep and numeric at every level, which is the shape both
/// addressable layouts write (`z/x/y` and google's `z/y/x`). A symlinked file
/// counts, so the tile read that refuses it is reached; a symlinked directory
/// is not descended into.
fn tile_shaped_files(root: &Path, ext: &str) -> TileCensus {
    let mut census = TileCensus {
        matching: 0,
        others: std::collections::BTreeMap::new(),
    };
    let numeric = |name: &std::ffi::OsStr| {
        name.to_str()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    };
    let dirs = |dir: &Path| -> Vec<std::fs::DirEntry> {
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().collect())
            .unwrap_or_default()
    };
    for z in dirs(root) {
        if !(z.file_type().is_ok_and(|t| t.is_dir()) && numeric(&z.file_name())) {
            continue;
        }
        for x in dirs(&z.path()) {
            if !(x.file_type().is_ok_and(|t| t.is_dir()) && numeric(&x.file_name())) {
                continue;
            }
            for y in dirs(&x.path()) {
                if y.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let path = y.path();
                let (Some(stem), Some(found)) = (path.file_stem(), path.extension()) else {
                    continue;
                };
                if !numeric(stem) {
                    continue;
                }
                match found.to_str() {
                    Some(e) if e == ext => census.matching += 1,
                    Some(e) => *census.others.entry(e.to_string()).or_default() += 1,
                    None => {}
                }
            }
        }
    }
    census
}

/// A [`DirectoryPyramidReader`] whose tile reads take a tile path as it is.
///
/// The library reader is a plain `fs::read`, which follows a symlink to
/// wherever it points and reads a FIFO or `/dev/zero` until it blocks or runs
/// out of memory. Pack copies whatever it reads into an archive somebody else
/// will serve, so every component of a tile path is checked with
/// `symlink_metadata`, anything that is not a regular file is refused with the
/// reason, and one tile is capped at what a tile of the plan's size could
/// weigh. Everything else is the library reader's.
struct GuardedTreeReader {
    inner: DirectoryPyramidReader,
    max_tile_bytes: u64,
    tile_side: u64,
}

impl GuardedTreeReader {
    fn new(inner: DirectoryPyramidReader, tile_size: u32, overlap: u32) -> Self {
        let tile_side = u64::from(tile_size) + 2 * u64::from(overlap);
        // 16 bytes a pixel covers a 16-bit RGBA PNG stored with no
        // compression at all, the heaviest tile any supported format can
        // encode, and the 1 MiB covers container overhead on a small tile.
        let max_tile_bytes = tile_side
            .saturating_mul(tile_side)
            .saturating_mul(16)
            .saturating_add(1024 * 1024);
        Self {
            inner,
            max_tile_bytes,
            tile_side,
        }
    }

    fn refused(path: &Path, why: &str) -> libviprs::PyramidReadError {
        libviprs::PyramidReadError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} {why}", path.display()),
        ))
    }
}

impl libviprs::PyramidReader for GuardedTreeReader {
    fn describe(&self) -> Result<libviprs::PyramidDescription, libviprs::PyramidReadError> {
        self.inner.describe()
    }

    fn tile(
        &self,
        coord: libviprs::planner::TileCoord,
    ) -> Result<Option<Vec<u8>>, libviprs::PyramidReadError> {
        use std::io::Read;

        let format = self.inner.tile_format().unwrap_or(TileFormat::Png);
        let Some(relative) = self.inner.plan().tile_path(coord, format.extension()) else {
            return Ok(None);
        };
        let relative = PathBuf::from(relative);
        let components: Vec<_> = relative.components().collect();
        let mut path = self.inner.base_dir().to_path_buf();
        for (i, component) in components.iter().enumerate() {
            path.push(component);
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if meta.file_type().is_symlink() {
                return Err(Self::refused(
                    &path,
                    "is a symlink; pack reads tiles as regular files and does not follow links",
                ));
            }
            if i + 1 < components.len() {
                if !meta.is_dir() {
                    // A file where a level directory should be holds no tile
                    // for this coordinate.
                    return Ok(None);
                }
                continue;
            }
            if !meta.is_file() {
                return Err(Self::refused(&path, "is not a regular file"));
            }
            if meta.len() > self.max_tile_bytes {
                return Err(Self::refused(
                    &path,
                    &format!(
                        "is {} bytes, larger than the {} bytes any {}px tile can weigh",
                        meta.len(),
                        self.max_tile_bytes,
                        self.tile_side
                    ),
                ));
            }
        }

        // Read through a cap as well as checking the length, because the file
        // can grow between the stat and the read.
        let file = std::fs::File::open(&path)?;
        let mut bytes = Vec::new();
        file.take(self.max_tile_bytes + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > self.max_tile_bytes {
            return Err(Self::refused(
                &path,
                &format!(
                    "grew past the {} bytes any {}px tile can weigh while it was read",
                    self.max_tile_bytes, self.tile_side
                ),
            ));
        }
        Ok(Some(bytes))
    }

    fn tile_format(&self) -> Option<TileFormat> {
        self.inner.tile_format()
    }
}

// ---------------------------------------------------------------------------
// Packfile sink dispatch (feature-gated)
// ---------------------------------------------------------------------------

/// Generate a pyramid into a tar, tar.gz or zip packfile. Hands the engine's
/// answer and the path written back for [`pipeline::conclude`].
fn run_pyramid_packfile(
    _path: &str,
    _raster: &Raster,
    _plan: &libviprs::PyramidPlan,
    _tile_format: TileFormat,
    _engine_config: EngineConfig,
    _resume_mode: ResumeMode,
    _cancel: libviprs::CancelToken,
) -> (
    Result<libviprs::EngineResult, libviprs::EngineError>,
    PathBuf,
) {
    #[cfg(feature = "packfile")]
    {
        use libviprs::{PackfileFormat, PackfileSink, ResumePolicy};

        // Infer archive format from path extension.
        let path_lower = _path.to_lowercase();
        let fmt = if path_lower.ends_with(".tar.gz") || path_lower.ends_with(".tgz") {
            PackfileFormat::TarGz
        } else if path_lower.ends_with(".zip") {
            PackfileFormat::Zip
        } else {
            PackfileFormat::Tar
        };

        // @doc-snippet:begin slot=sink-packfile imports=PackfileSink,PackfileFormat,TileFormat
        let sink = match PackfileSink::new(_path, fmt, _plan.clone(), _tile_format) {
            Ok(s) => s,
            Err(e) => operational_error(&format!("creating packfile sink: {e}")),
        };
        // @doc-snippet:end slot=sink-packfile

        let policy = match _resume_mode {
            ResumeMode::Overwrite => ResumePolicy::overwrite(),
            ResumeMode::Resume => ResumePolicy::resume(),
            ResumeMode::Verify => ResumePolicy::verify(),
        };
        let result = EngineBuilder::new(_raster, _plan.clone(), &sink)
            .with_config(_engine_config)
            .with_resume(policy)
            .with_cancel(_cancel)
            .run();
        (result, sink.out_path().to_path_buf())
    }
    #[cfg(not(feature = "packfile"))]
    {
        operational_error(
            "packfile:// sink requires the `packfile` feature — rebuild with `--features packfile`.",
        );
    }
}

fn run_info(args: InfoArgs) {
    let path = &args.input;

    if !path.exists() {
        eprintln!("File not found: {}", path.display());
        process::exit(1);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "pdf" {
        match libviprs::pdf_info(path) {
            Ok(info) if args.json => {
                let pages: Vec<serde_json::Value> = info
                    .pages
                    .iter()
                    .map(|page| {
                        serde_json::json!({
                            "page": page.page_number,
                            "width_pts": page.width_pts,
                            "height_pts": page.height_pts,
                            "has_images": page.has_images,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({
                        "v": 1,
                        "kind": "pdf",
                        "path": path.display().to_string(),
                        "pages": info.page_count,
                        "page_sizes": pages,
                    })
                );
            }
            Ok(info) => {
                println!("PDF: {}", path.display());
                println!("Pages: {}", info.page_count);
                for page in &info.pages {
                    println!(
                        "  Page {}: {:.1} x {:.1} pts{}",
                        page.page_number,
                        page.width_pts,
                        page.height_pts,
                        if page.has_images { " (has images)" } else { "" }
                    );
                }
            }
            Err(e) => {
                eprintln!("Error reading PDF: {e}");
                process::exit(1);
            }
        }
    } else {
        match input::decode_path_default(path) {
            Ok(raster) if args.json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "v": 1,
                        "kind": "image",
                        "path": path.display().to_string(),
                        "width": raster.width(),
                        "height": raster.height(),
                        "format": format!("{:?}", raster.format()),
                        "bytes": raster.data().len(),
                    })
                );
            }
            Ok(raster) => {
                println!("Image: {}", path.display());
                println!("Dimensions: {}x{}", raster.width(), raster.height());
                println!("Format: {:?}", raster.format());
                println!(
                    "Size: {:.1} MB",
                    raster.data().len() as f64 / (1024.0 * 1024.0)
                );
            }
            Err(e) => {
                eprintln!("Error reading image: {e:#}");
                process::exit(1);
            }
        }
    }
}

fn run_plan(args: PlanArgs) {
    check_overlap(args.tile_size, args.overlap);
    let (w, h) = resolve_plan_dimensions(&args);

    let layout: Layout = args.layout.into();
    let planner = match PyramidPlanner::new(w, h, args.tile_size, args.overlap, layout) {
        Ok(p) => p.with_centre(args.centre),
        Err(e) => {
            eprintln!("Error creating pyramid plan: {e}");
            process::exit(1);
        }
    };
    let plan = planner.plan();
    if args.queries.is_query() {
        pdf_geo_plan::run_plan_query(&args.queries, &plan, layout);
        return;
    }

    let peak_memory = planner.estimate_peak_memory();
    let (canvas_w, canvas_h) = planner.canvas_dimensions();

    println!("Image: {}x{}", w, h);
    println!(
        "Canvas: {}x{} ({:.1} MB)",
        canvas_w,
        canvas_h,
        canvas_w as f64 * canvas_h as f64 * 4.0 / (1024.0 * 1024.0)
    );
    println!(
        "Tile size: {}, overlap: {}, layout: {:?}",
        args.tile_size, args.overlap, layout
    );
    println!(
        "Levels: {}, total tiles: {}",
        plan.level_count(),
        plan.total_tile_count()
    );
    println!(
        "Estimated peak memory: {:.1} MB",
        peak_memory as f64 / (1024.0 * 1024.0)
    );
    println!();
    println!(
        "{:<8} {:<14} {:<10} {:<8}",
        "Level", "Dimensions", "Grid", "Tiles"
    );
    println!("{}", "-".repeat(42));
    for level in plan.levels.iter().rev() {
        println!(
            "{:<8} {:<14} {:<10} {:<8}",
            level.level,
            format!("{}x{}", level.width, level.height),
            format!("{}x{}", level.cols, level.rows),
            level.tile_count()
        );
    }
}

fn run_test_image(args: TestImageArgs) {
    let raster = match libviprs::generate_test_raster(args.width, args.height) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error generating test raster: {e}");
            process::exit(1);
        }
    };

    let encoded = match libviprs::sink::encode_png(&raster) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error encoding PNG: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = std::fs::write(&args.output, &encoded) {
        eprintln!("Error writing file: {e}");
        process::exit(1);
    }

    eprintln!(
        "Generated {}x{} {:?} test image: {}",
        raster.width(),
        raster.height(),
        raster.format(),
        args.output.display()
    );
}

fn resolve_plan_dimensions(args: &PlanArgs) -> (u32, u32) {
    // Try parsing as a number first
    if let Ok(w) = args.width_or_input.parse::<u32>() {
        // Both are the command line's mistake, so both are usage errors (#81).
        if w == 0 {
            usage_error(
                "a width of 0 describes no image",
                "give the width in pixels, 1 or more, or the path of an image or PDF",
            );
        }
        let Some(h) = args.height else {
            usage_error(
                "--height is required when width is given as a number",
                "pass --height, or give the path of an image or PDF to read both from",
            );
        };
        return (w, h);
    }

    // Otherwise treat as a file path
    let path = PathBuf::from(&args.width_or_input);
    if !path.exists() {
        eprintln!("Not a number or file: {}", args.width_or_input);
        process::exit(1);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "pdf" {
        match libviprs::pdf_info(&path) {
            Ok(info) => {
                let page_info = info.pages.iter().find(|p| p.page_number == args.page);
                match page_info {
                    Some(p) => libviprs::PageSizing::from(args.page_sizing).pixel_dims(
                        p.width_pts,
                        p.height_pts,
                        args.dpi,
                    ),
                    None => {
                        eprintln!(
                            "Page {} not found in PDF (has {} pages)",
                            args.page, info.page_count
                        );
                        process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading PDF: {e}");
                process::exit(1);
            }
        }
    } else {
        match input::decode_path_default(&path) {
            Ok(raster) => (raster.width(), raster.height()),
            Err(e) => {
                eprintln!("Error reading image: {e:#}");
                process::exit(1);
            }
        }
    }
}

fn load_source(args: &PyramidArgs) -> Raster {
    // Read from stdin
    if args.input == "-" {
        eprintln!("Reading from stdin...");
        let mut buf = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
            eprintln!("Error reading stdin: {e}");
            process::exit(1);
        }
        match input::decode_bytes_default(&buf) {
            Ok(r) => return r,
            Err(e) => {
                eprintln!("Error decoding image from stdin: {e:#}");
                process::exit(1);
            }
        }
    }

    let path = PathBuf::from(&args.input);

    if !path.exists() {
        eprintln!("Input file not found: {}", path.display());
        process::exit(1);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // @doc-snippet:begin slot=load-source imports=Raster,extract_page_image,render_page_pdfium_with,decode_file
    if ext == "pdf" {
        // @doc-test: pdfium_integration.rs::libviprs_pdfium_render_paths:34
        if args.render {
            // @doc-flag: render kind=override
            // `--render` requires the `pdfium` feature. When the binary is built
            // `--no-default-features` (pdfium-free), `render_page_pdfium_with` is absent,
            // so this path is compiled out and the flag fails loudly instead.
            #[cfg(not(feature = "pdfium"))]
            {
                eprintln!(
                    "Error: --render needs the `pdfium` feature, which was not compiled into this binary."
                );
                eprintln!(
                    "Hint: use a default-features build (pdfium enabled), or omit --render to extract embedded images instead."
                );
                process::exit(1);
            }
            // Use PDFium to render the page (vector PDFs)
            #[cfg(feature = "pdfium")]
            {
                eprintln!(
                    "Rendering PDF page {} at {} DPI (pdfium)...",
                    args.page, args.dpi
                );
                // @doc-test: pdfium_integration.rs::libviprs_pdfium_render_paths:34
                match render_page_pdfium_with(&path, args.page, args.dpi, args.page_sizing.into()) {
                    // @doc-flag: dpi kind=param param_name=dpi
                    // @doc-flag: page-sizing kind=param param_name=sizing
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("Error rendering PDF with pdfium: {e}");
                        eprintln!(
                            "Hint: ensure libpdfium is installed. Run without --render to extract embedded images instead."
                        );
                        process::exit(1);
                    }
                }
            }
        } else {
            // Extract embedded raster image (scanned PDFs)
            eprintln!("Extracting image from PDF page {}...", args.page);
            // @doc-test: pdf_ops.rs::extract_page_image_from_blueprint:31
            let raster = match extract_page_image(&path, args.page) {
                // @doc-flag: page kind=param param_name=page
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error extracting image from PDF: {e}");
                    eprintln!(
                        "Hint: use --render for vector PDFs that don't contain embedded images."
                    );
                    process::exit(1);
                }
            };

            // Optionally resize to match PDF page dimensions at the given DPI
            // @doc-test: pdf_to_pyramid.rs::pdf_to_georeferenced_pyramid_memory:17
            if args.match_page_size {
                // @doc-flag: match-page-size kind=append
                let page_dims = match libviprs::pdf_info(&path) {
                    Ok(info) => {
                        let page_info = info.pages.iter().find(|p| p.page_number == args.page);
                        match page_info {
                            Some(p) => libviprs::PageSizing::from(args.page_sizing).pixel_dims(
                                p.width_pts,
                                p.height_pts,
                                args.dpi,
                            ),
                            None => {
                                eprintln!("Page {} not found in PDF", args.page);
                                process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error reading PDF info for page sizing: {e}");
                        process::exit(1);
                    }
                };

                if page_dims.0 != raster.width() || page_dims.1 != raster.height() {
                    eprintln!(
                        "Resizing {}x{} → {}x{} (matching page at {} DPI)",
                        raster.width(),
                        raster.height(),
                        page_dims.0,
                        page_dims.1,
                        args.dpi
                    );
                    match libviprs::resize::downscale_to(&raster, page_dims.0, page_dims.1) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("Error resizing raster: {e}");
                            process::exit(1);
                        }
                    }
                } else {
                    raster
                }
            } else {
                raster
            }
        }
    } else {
        eprintln!("Decoding {}...", path.display());
        match input::decode_path_default(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error decoding image: {e:#}");
                process::exit(1);
            }
        }
    }
    // @doc-snippet:end slot=load-source
}

fn build_geo_transform(args: &PyramidArgs, _w: u32, _h: u32) -> Option<GeoTransform> {
    // @doc-snippet:begin slot=geo imports=GeoTransform,GeoCoord
    let origin_str = args.geo_origin.as_ref()?;
    let scale_str = args.geo_scale.as_ref()?;

    let origin = parse_coord_pair(origin_str, "geo-origin");
    let scale = parse_coord_pair(scale_str, "geo-scale");

    Some(GeoTransform::from_origin_and_scale(
        // @doc-test: pdf_to_pyramid.rs::pdf_to_georeferenced_pyramid_memory:17
        GeoCoord::new(origin.0, origin.1), // @doc-flag: geo-origin kind=param param_name=geo-origin
        // @doc-test: pdf_to_pyramid.rs::pdf_to_georeferenced_pyramid_memory:17
        scale.0, // @doc-flag: geo-scale kind=param param_name=geo-scale
        scale.1,
    ))
    // @doc-snippet:end slot=geo
}

/// `--geo-origin` / `--geo-scale` as an `x,y` pair, through the same parser
/// `viprs geo` uses, so a bad or non-finite pair is a usage error (exit 2) on
/// both commands.
fn parse_coord_pair(s: &str, name: &str) -> (f64, f64) {
    let v = pdf_geo_plan::parse_floats(s, name, 2);
    (v[0], v[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a `viprs pyramid`-style invocation from just the flags under test,
    /// filling in the two required positionals.
    fn parse_pyramid(extra: &[&str]) -> Result<PyramidArgs, clap::Error> {
        let mut argv = vec!["viprs", "in.pdf", "out"];
        argv.extend_from_slice(extra);
        PyramidArgs::try_parse_from(argv)
    }

    #[test]
    fn mb_to_bytes_converts_within_u64() {
        assert_eq!(mb_to_bytes(0), 0);
        assert_eq!(mb_to_bytes(1), 1024 * 1024);
        // The cap value still fits, proving the byte math never wraps.
        assert_eq!(mb_to_bytes(MEMORY_MB_CAP), MEMORY_MB_CAP * 1024 * 1024);
    }

    #[test]
    fn memory_limit_at_cap_is_accepted() {
        let args = parse_pyramid(&["--memory-limit", &MEMORY_MB_CAP.to_string()])
            .expect("the cap value must parse");
        assert_eq!(args.memory_limit, MEMORY_MB_CAP);
    }

    #[test]
    fn memory_limit_over_cap_is_rejected() {
        let err = parse_pyramid(&["--memory-limit", &(MEMORY_MB_CAP + 1).to_string()])
            .err()
            .expect("an over-cap memory limit must be rejected at parse time");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn memory_budget_over_cap_is_rejected() {
        let err = parse_pyramid(&["--memory-budget", &(MEMORY_MB_CAP + 1).to_string()])
            .err()
            .expect("an over-cap memory budget must be rejected at parse time");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn former_overflow_value_is_now_rejected() {
        // Before the cap, `u64::MAX * 1024 * 1024` panicked in debug and wrapped
        // to a tiny limit in release, inverting the guard. It must now be
        // rejected up front rather than reaching the byte arithmetic.
        let err = parse_pyramid(&["--memory-limit", &u64::MAX.to_string()])
            .err()
            .expect("u64::MAX must be rejected instead of wrapping");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn packfile_with_manifest_checksums_is_rejected() {
        // The packfile sink cannot carry per-tile checksums, so requesting them
        // alongside --packfile must fail loudly at parse time rather than
        // exiting 0 with the checksum request silently dropped.
        let err = parse_pyramid(&["--packfile", "--manifest-emit-checksums"])
            .err()
            .expect("--packfile with --manifest-emit-checksums must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn packfile_alone_still_parses() {
        // The new conflict must not regress the plain --packfile shorthand.
        let args = parse_pyramid(&["--packfile"]).expect("--packfile alone must still parse");
        assert!(args.packfile);
        assert!(!args.manifest_emit_checksums);
    }

    #[test]
    fn help_mentions_s3_only_beside_the_feature_it_needs() {
        // The s3:// sink was a compiled-in stub, and this test used to forbid
        // the help from mentioning it at all. It writes through the local stub
        // store now (libviprs-cli#66), but only in a build with the `s3`
        // feature, so the help may name the scheme only where it also names
        // that feature.
        use clap::CommandFactory;
        let help = PyramidArgs::command().render_long_help().to_string();
        let mentions: Vec<&str> = help
            .split("\n\n")
            .filter(|para| para.contains("s3://"))
            .collect();
        assert!(
            mentions.iter().all(|para| para.contains("`s3` feature")),
            "every help paragraph naming s3:// must say it needs the `s3` feature, got:\n{help}"
        );
    }

    // ----------------------------------------------------------------------
    // parse_duration_literal
    // ----------------------------------------------------------------------

    #[test]
    fn duration_ms_wins_over_s() {
        // The regression the issue calls out: if the `s` suffix were stripped
        // before `ms`, `"50ms"` would resolve to 50 seconds. Pin the correct
        // millisecond reading so a reordered strip-suffix chain fails here.
        assert_eq!(
            parse_duration_literal("50ms").unwrap(),
            std::time::Duration::from_millis(50)
        );
    }

    #[test]
    fn duration_bare_number_is_milliseconds() {
        // A bare number means milliseconds, matching the old
        // --retry-backoff semantics.
        assert_eq!(
            parse_duration_literal("250").unwrap(),
            std::time::Duration::from_millis(250)
        );
    }

    #[test]
    fn duration_seconds_micros_nanos_units() {
        assert_eq!(
            parse_duration_literal("2s").unwrap(),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            parse_duration_literal("500us").unwrap(),
            std::time::Duration::from_micros(500)
        );
        // The subtlety noted in the issue: `"50ns"` parses as nanoseconds
        // rather than being mistaken for a bare `ns`-less number.
        assert_eq!(
            parse_duration_literal("50ns").unwrap(),
            std::time::Duration::from_nanos(50)
        );
    }

    #[test]
    fn duration_trims_surrounding_whitespace() {
        assert_eq!(
            parse_duration_literal("  10ms  ").unwrap(),
            std::time::Duration::from_millis(10)
        );
    }

    #[test]
    fn duration_empty_is_rejected() {
        assert!(parse_duration_literal("").is_err());
        assert!(parse_duration_literal("   ").is_err());
    }

    #[test]
    fn duration_non_numeric_is_rejected() {
        assert!(parse_duration_literal("abcms").is_err());
        assert!(parse_duration_literal("ms").is_err());
    }

    #[test]
    fn duration_large_value_saturates_without_panic() {
        // A huge nanosecond count must saturate rather than overflow-panic.
        let d = parse_duration_literal(&format!("{}s", u64::MAX)).unwrap();
        assert_eq!(d, std::time::Duration::from_nanos(u64::MAX));
    }

    // ----------------------------------------------------------------------
    // parse_failure_policy
    // ----------------------------------------------------------------------

    #[test]
    fn failure_policy_fail_fast() {
        assert!(matches!(
            parse_failure_policy("fail-fast").unwrap(),
            FailurePolicy::FailFast
        ));
    }

    #[test]
    fn failure_policy_retry_reads_count_and_backoff() {
        // `retry=3,50ms` must yield RetryThenFail with 3 retries and a 50 ms
        // (not 50 s) backoff, guarding the ms-over-s ordering end to end.
        match parse_failure_policy("retry=3,50ms").unwrap() {
            FailurePolicy::RetryThenFail(policy) => {
                assert_eq!(policy.max_retries, 3);
                assert_eq!(policy.initial_backoff, std::time::Duration::from_millis(50));
            }
            other => panic!("expected RetryThenFail, got {other:?}"),
        }
    }

    #[test]
    fn failure_policy_retry_skip_variant() {
        match parse_failure_policy("retry-skip=5,2s").unwrap() {
            FailurePolicy::RetryThenSkip(policy) => {
                assert_eq!(policy.max_retries, 5);
                assert_eq!(policy.initial_backoff, std::time::Duration::from_secs(2));
            }
            other => panic!("expected RetryThenSkip, got {other:?}"),
        }
    }

    #[test]
    fn failure_policy_rejects_missing_separators() {
        // No `=`, no `,`, and an unknown kind must all be rejected.
        assert!(parse_failure_policy("retry").is_err());
        assert!(parse_failure_policy("retry=3").is_err());
        assert!(parse_failure_policy("bogus=3,50ms").is_err());
    }

    #[test]
    fn failure_policy_rejects_non_numeric_count() {
        assert!(parse_failure_policy("retry=x,50ms").is_err());
    }

    // ----------------------------------------------------------------------
    // resolve_sink_uri
    // ----------------------------------------------------------------------

    /// Parse a whole `viprs pyramid` argument vector, for the cases where the
    /// positional output is the thing under test.
    fn parse_pyramid_argv(argv: &[&str]) -> Result<PyramidArgs, clap::Error> {
        let mut full = vec!["viprs"];
        full.extend_from_slice(argv);
        PyramidArgs::try_parse_from(full)
    }

    #[test]
    fn sink_uri_defaults_to_a_pmtiles_archive() {
        // The 0.4.0 flip. This used to read `fs://out`.
        let args = parse_pyramid_argv(&["in.pdf", "out.pmtiles"]).expect("must parse");
        assert_eq!(resolve_sink_uri(&args), "pmtiles://out.pmtiles");
    }

    #[test]
    fn sink_uri_derives_the_archive_from_the_input_name() {
        let args = parse_pyramid_argv(&["drawing.tif"]).expect("must parse");
        assert_eq!(resolve_sink_uri(&args), "pmtiles://drawing.pmtiles");
    }

    #[test]
    fn sink_uri_storage_directory_is_an_fs_sink() {
        let args =
            parse_pyramid_argv(&["in.pdf", "tiles", "--storage", "directory"]).expect("must parse");
        assert_eq!(resolve_sink_uri(&args), "fs://tiles");
    }

    #[test]
    fn a_directory_shaped_target_is_recognised_by_any_of_its_three_signals() {
        // Each signal on its own, so a rewrite that drops one is visible here
        // rather than only in the end-to-end refusal.
        assert!(looks_like_a_directory_target(Path::new("tiles")));
        assert!(looks_like_a_directory_target(Path::new("tiles/")));
        assert!(looks_like_a_directory_target(Path::new(".")));
        // And the negative control, or the check would "pass" by refusing
        // everything.
        assert!(!looks_like_a_directory_target(Path::new("tiles.pmtiles")));
        assert!(!looks_like_a_directory_target(Path::new("a/b/c.pmtiles")));
    }

    #[test]
    fn layout_follows_the_storage_backend_unless_it_was_asked_for() {
        let bare = parse_pyramid_argv(&["in.pdf", "out.pmtiles"]).expect("must parse");
        assert_eq!(resolve_layout(&bare, true), Layout::Xyz);
        assert_eq!(resolve_layout(&bare, false), Layout::DeepZoom);

        let asked = parse_pyramid_argv(&["in.pdf", "out.pmtiles", "--layout", "google"])
            .expect("must parse");
        assert_eq!(resolve_layout(&asked, true), Layout::Google);
        assert_eq!(resolve_layout(&asked, false), Layout::Google);
    }

    #[test]
    fn tile_format_is_unchanged_for_everything_an_archive_can_hold() {
        let png = parse_pyramid_argv(&["in.pdf", "out.pmtiles"]).expect("must parse");
        assert_eq!(resolve_tile_format(&png, true), TileFormat::Png);

        let jpeg = parse_pyramid_argv(&[
            "in.pdf",
            "out.pmtiles",
            "--format",
            "jpeg",
            "--quality",
            "70",
        ])
        .expect("must parse");
        assert_eq!(
            resolve_tile_format(&jpeg, true),
            TileFormat::Jpeg { quality: 70 }
        );

        // Raw survives when it is not going into an archive.
        let raw = parse_pyramid_argv(&[
            "in.pdf",
            "tiles",
            "--storage",
            "directory",
            "--format",
            "raw",
        ])
        .expect("must parse");
        assert_eq!(resolve_tile_format(&raw, false), TileFormat::Raw);
    }

    #[test]
    fn storage_conflicts_with_the_flags_that_name_their_own_target() {
        assert!(
            parse_pyramid_argv(&[
                "in.pdf",
                "out.pmtiles",
                "--storage",
                "pmtiles",
                "--sink",
                "fs://x"
            ])
            .is_err(),
            "an explicit --storage next to --sink must be a parse error"
        );
        assert!(
            parse_pyramid_argv(&["in.pdf", "out", "--storage", "directory", "--packfile"]).is_err(),
            "an explicit --storage next to --packfile must be a parse error"
        );
        // The default value must not trip the conflict, or --sink and
        // --packfile would both stop working entirely.
        assert!(
            parse_pyramid_argv(&["in.pdf", "out", "--sink", "fs://x"]).is_ok(),
            "a defaulted --storage must not conflict with --sink"
        );
        assert!(
            parse_pyramid_argv(&["in.pdf", "out", "--packfile"]).is_ok(),
            "a defaulted --storage must not conflict with --packfile"
        );
    }

    #[test]
    fn sink_uri_packfile_shorthand() {
        let args = parse_pyramid(&["--packfile"]).expect("--packfile must parse");
        assert_eq!(resolve_sink_uri(&args), "packfile://out.tar");
    }

    #[test]
    fn sink_uri_explicit_sink_passthrough() {
        let args = parse_pyramid(&["--sink", "s3://bucket/key"]).expect("--sink must parse");
        assert_eq!(resolve_sink_uri(&args), "s3://bucket/key");
    }

    // ----------------------------------------------------------------------
    // resolve_resume_mode
    // ----------------------------------------------------------------------

    #[test]
    fn resume_mode_defaults_to_overwrite() {
        let args = parse_pyramid(&[]).expect("bare invocation must parse");
        assert_eq!(resolve_resume_mode(&args), ResumeMode::Overwrite);
    }

    #[test]
    fn resume_mode_resume_flag() {
        let args = parse_pyramid(&["--resume"]).expect("--resume must parse");
        assert_eq!(resolve_resume_mode(&args), ResumeMode::Resume);
    }

    #[test]
    fn resume_mode_verify_flag() {
        let args = parse_pyramid(&["--verify"]).expect("--verify must parse");
        assert_eq!(resolve_resume_mode(&args), ResumeMode::Verify);
    }

    #[test]
    fn resume_mode_overwrite_flag_is_explicit() {
        let args = parse_pyramid(&["--overwrite"]).expect("--overwrite must parse");
        assert_eq!(resolve_resume_mode(&args), ResumeMode::Overwrite);
    }
}
