//! `viprs` command-line interface.
//!
//! See also: [interactive CLI reference](https://libviprs.org/cli/) for runnable
//! examples and per-flag anchors used throughout the field-level documentation
//! below.

use std::io::Read as _;
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use clap::{ArgGroup, Parser, ValueEnum};
use libviprs::{
    BlankTileStrategy, ChecksumAlgo, ChecksumMode, CollectingObserver, DedupeStrategy,
    EngineBuilder, EngineConfig, EngineKind, FailurePolicy, FsSink, GeoCoord, GeoTransform, Layout,
    ManifestBuilder, PyramidPlanner, Raster, ResumeMode, ResumePolicy, RetryPolicy, TileFormat,
    extract_page_image,
    streaming::{BudgetPolicy, compute_strip_height, estimate_streaming_memory},
    streaming_mapreduce::{compute_inflight_strips, estimate_mapreduce_peak_memory},
};
// PDFium vector rasterisation is gated behind the `pdfium` feature (on by
// default). Without it the `--render` path is compiled out and `render_page_pdfium`
// does not exist in the core crate, so the import is feature-gated too.
#[cfg(feature = "pdfium")]
use libviprs::pdf::render_page_pdfium;

/// Per-family op registry (`CLI_CONTRACT.md` §6). The pyramid/info/plan/
/// test-image commands below stay in `main.rs` untouched; every op family is
/// additive under `src/ops/`.
mod ops;

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
}

#[derive(Parser)]
#[non_exhaustive]
#[command(group(
    ArgGroup::new("checksums")
        .required(false)
        .multiple(true)
        .args(["manifest_emit_checksums", "dedupe_all"]),
))]
struct PyramidArgs {
    /// Input file (PDF, PNG, JPEG, or TIFF). Use "-" for stdin.
    input: String,

    /// Output directory for tiles.
    output: PathBuf,

    /// Tile size in pixels.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-tile-size).
    #[arg(long, default_value = "256")]
    tile_size: u32,

    /// Tile overlap in pixels.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-overlap).
    #[arg(long, default_value = "0")]
    overlap: u32,

    /// Tile layout format.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-layout).
    #[arg(long, default_value = "deep-zoom")]
    layout: LayoutArg,

    /// Tile image format.
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-format).
    #[arg(long, default_value = "png")]
    format: FormatArg,

    /// JPEG quality (1-100, only used with --format jpeg).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-quality).
    #[arg(long, default_value = "85")]
    quality: u8,

    /// DPI for PDF rendering/page-size scaling (default matches libvips).
    #[arg(long, default_value = "72")]
    dpi: u32,

    /// PDF page number to extract (1-based, only used for PDF inputs).
    #[arg(long, default_value = "1")]
    page: usize,

    /// Number of worker threads (0 = single-threaded).
    ///
    /// See also: [interactive example](https://libviprs.org/cli/#flag-concurrency).
    #[arg(long, default_value = "0")]
    concurrency: usize,

    /// Maximum tiles buffered between producer and sink (backpressure control).
    #[arg(long, default_value = "64")]
    buffer_size: usize,

    /// Geo-reference origin as "longitude,latitude" (top-left pixel).
    #[arg(long)]
    geo_origin: Option<String>,

    /// Geo-reference pixel scale as "scale_x,scale_y" (degrees per pixel).
    #[arg(long)]
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
    /// Sink URI: fs://path or packfile://path.tar[.gz]/.zip.
    /// Defaults to the positional output directory as a filesystem sink.
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
}

#[derive(Parser)]
struct InfoArgs {
    /// PDF or image file to inspect.
    input: PathBuf,
}

#[derive(Parser)]
struct PlanArgs {
    /// Image width in pixels (or path to an image/PDF file to read dimensions from).
    width_or_input: String,

    /// Image height in pixels (required when width is given as a number).
    #[arg(long)]
    height: Option<u32>,

    /// Tile size in pixels.
    #[arg(long, default_value = "256")]
    tile_size: u32,

    /// Tile overlap in pixels.
    #[arg(long, default_value = "0")]
    overlap: u32,

    /// Tile layout format.
    #[arg(long, default_value = "deep-zoom")]
    layout: LayoutArg,

    /// DPI for PDF dimensions (only used when input is a PDF).
    #[arg(long, default_value = "72")]
    dpi: u32,

    /// PDF page number (1-based, only used when input is a PDF).
    #[arg(long, default_value = "1")]
    page: usize,

    /// Centre the image within the tile grid (even padding on all sides).
    #[arg(long)]
    centre: bool,
}

#[derive(Parser)]
struct TestImageArgs {
    /// Output image file path.
    output: PathBuf,

    /// Image width in pixels.
    #[arg(long, default_value = "1024")]
    width: u32,

    /// Image height in pixels.
    #[arg(long, default_value = "1024")]
    height: u32,
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

#[derive(Clone, ValueEnum)]
enum FormatArg {
    Png,
    Jpeg,
    Raw,
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
    let matches = ops::assembled_cli().get_matches();

    match matches.subcommand() {
        Some(("pyramid" | "info" | "plan" | "test-image", _)) => {
            let cli = Cli::from_arg_matches(&matches)
                .expect("a built-in subcommand deserializes through the derive Cli");
            match cli.command {
                Command::Pyramid(args) => run_pyramid(*args),
                Command::Info(args) => run_info(args),
                Command::Plan(args) => run_plan(args),
                Command::TestImage(args) => run_test_image(args),
            }
        }
        Some(("__dump-commands", sub)) => ops::run_dump(sub),
        Some((name, sub)) => {
            // Op failure → exit 1 (`CLI_CONTRACT.md` §8); usage errors already
            // exited 2 inside clap parsing above.
            if let Err(e) = ops::dispatch(name, sub) {
                eprintln!("Error: {e:#}");
                process::exit(1);
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

/// Resolve the effective sink URI from flags.
///
/// Priority:
/// 1. `--packfile` shorthand  → `packfile://<output>.tar`
/// 2. `--sink <URI>`          → as-is
/// 3. (none)                  → `fs://<output>`
///
/// `--packfile` and `--sink` are declared as `conflicts_with` at the clap
/// layer, but we keep a defensive check here so `resolve_sink_uri` is safe
/// to call from any caller regardless of how `PyramidArgs` was built.
fn resolve_sink_uri(args: &PyramidArgs) -> String {
    if args.packfile && args.sink.is_some() {
        eprintln!("Error: --packfile and --sink are mutually exclusive");
        process::exit(2);
    }
    if args.packfile {
        return format!("packfile://{}.tar", args.output.display());
    }
    if let Some(ref uri) = args.sink {
        return uri.clone();
    }
    format!("fs://{}", args.output.display())
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
        tracing_subscriber::fmt()
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
        process::exit(2);
    }
}

fn run_pyramid(args: PyramidArgs) {
    let start = Instant::now();

    // Initialise tracing if requested (exits with an error when the feature is off).
    maybe_init_tracing(&args.trace_level);

    // Load the source raster
    let raster = load_source(&args);

    let w = raster.width();
    let h = raster.height();
    eprintln!(
        "Source: {}x{} {:?} ({:.1} MB)",
        w,
        h,
        raster.format(),
        raster.data().len() as f64 / (1024.0 * 1024.0)
    );

    // Geo-reference (optional)
    if let Some(geo) = build_geo_transform(&args, w, h) {
        let bounds = geo.image_bounds(w, h);
        eprintln!(
            "Geo bounds: ({:.6}, {:.6}) → ({:.6}, {:.6})",
            bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y
        );
    }

    // Plan
    let layout: Layout = args.layout.clone().into();
    // @doc-snippet:begin slot=planner imports=PyramidPlanner,Layout
    let planner = match PyramidPlanner::new(
        w,
        h,
        // @doc-test: blank_tile_strategy.rs::emit_solid_white_matches_expected:138
        args.tile_size, // @doc-flag: tile-size kind=param param_name=tile-size
        // @doc-test: builder_sink_fs.rs::two_arg_new_defaults_to_png:47
        args.overlap, // @doc-flag: overlap kind=param param_name=overlap
        // @doc-test: google_centre_pyramid.rs::google_centre_portrait_plan_structure:107
        layout, // @doc-flag: layout kind=param param_name=layout
    ) {
        // @doc-test: google_centre_pyramid.rs::google_centre_portrait_plan_structure:107
        Ok(p) => p.with_centre(args.centre), // @doc-flag: centre kind=append
        Err(e) => {
            eprintln!("Error creating pyramid plan: {e}");
            process::exit(1);
        }
    };
    // @doc-snippet:end slot=planner

    // Pre-render memory check
    let peak_memory = planner.estimate_peak_memory();
    let (canvas_w, canvas_h) = planner.canvas_dimensions();
    eprintln!(
        "Memory estimate: {:.1} MB peak (canvas: {}x{}, source: {}x{})",
        peak_memory as f64 / (1024.0 * 1024.0),
        canvas_w,
        canvas_h,
        w,
        h
    );

    // @doc-snippet:begin slot=memory-limit
    // @doc-test: streaming_engine.rs::estimate_streaming_memory_reasonable:435
    if args.memory_limit > 0 {
        // @doc-flag: memory-limit kind=param param_name=memory-limit
        let limit_bytes = mb_to_bytes(args.memory_limit);
        if peak_memory > limit_bytes {
            eprintln!(
                "Error: estimated peak memory ({:.1} MB) exceeds --memory-limit ({} MB)",
                peak_memory as f64 / (1024.0 * 1024.0),
                args.memory_limit
            );
            eprintln!("Hint: reduce --dpi or image dimensions to lower memory usage");
            process::exit(1);
        }
    }
    // @doc-snippet:end slot=memory-limit

    let plan = planner.plan();
    eprintln!(
        "Plan: {} levels, {} tiles, tile_size={}, overlap={}",
        plan.level_count(),
        plan.total_tile_count(),
        args.tile_size,
        args.overlap
    );

    // Tile format
    let tile_format = match args.format {
        FormatArg::Png => TileFormat::Png,
        FormatArg::Jpeg => TileFormat::Jpeg {
            quality: args.quality,
        },
        FormatArg::Raw => TileFormat::Raw,
    };

    // Resolve engine configuration
    let blank_strategy = build_blank_tile_strategy(&args);
    let failure_policy = build_failure_policy(&args);
    let dedupe_strategy = build_dedupe_strategy(&args);
    let checksum_algo: ChecksumAlgo = args.checksum_algo.clone().into();

    // Manifest builder (attached to sinks that support it)
    let manifest_builder = if args.manifest_emit_checksums {
        Some(ManifestBuilder::new().with_checksums(checksum_algo))
    } else {
        None
    };

    // Engine config
    // @doc-snippet:begin slot=engine-config imports=EngineConfig,BlankTileStrategy,FailurePolicy,DedupeStrategy,RetryPolicy
    let mut engine_config = EngineConfig::default()
        // @doc-test: builder_engine_surface.rs::builder_honours_with_concurrency:100
        .with_concurrency(args.concurrency) // @doc-flag: concurrency kind=appendChain
        // @doc-test: builder_engine_surface.rs::builder_honours_with_buffer_size:119
        .with_buffer_size(args.buffer_size) // @doc-flag: buffer-size kind=appendChain
        // @doc-test: blank_tile_strategy.rs::placeholder_solid_white_matches_expected:201
        .with_blank_tile_strategy(blank_strategy) // @doc-flag: skip-blank kind=append
        // @doc-test: phase3_blank_tolerance.rs::engine_with_tolerance_writes_placeholder_for_near_white_tiles:248
        // @doc-flag: blank-tolerance kind=append
        // @doc-test: phase3_retry.rs::retries_on_transient_errors:256
        // @doc-flag: retry-max kind=param param_name=retry-max
        // @doc-test: phase3_retry.rs::retries_on_transient_errors:256
        // @doc-flag: retry-backoff kind=param param_name=retry-backoff
        // @doc-test: builder_resume_retry.rs::builder_with_failure_policy_accepts_every_variant:145
        .with_failure_policy(failure_policy); // @doc-flag: failure-policy kind=param param_name=failure-policy

    if let Some(ds) = dedupe_strategy {
        // @doc-test: phase3_dedupe_blanks.rs::blanks_dedupe_manifest_lists_references:364
        // @doc-flag: dedupe-blanks kind=append
        // @doc-test: phase3_dedupe_blanks.rs::all_mode_dedupes_identical_non_blank_tiles:467
        engine_config = engine_config.with_dedupe_strategy(ds); // @doc-flag: dedupe-all kind=append
    }
    // @doc-snippet:end slot=engine-config

    // Resolve sink URI and build the appropriate sink.
    let sink_uri = resolve_sink_uri(&args);
    let resume_mode = resolve_resume_mode(&args);

    // We dispatch on the URI scheme.  The code below builds the appropriate
    // sink and then runs the engine.  Feature-gated variants fall back to a
    // friendly error when the feature is not compiled in.
    if let Some(rest) = sink_uri.strip_prefix("s3://") {
        run_pyramid_s3(
            rest,
            &args,
            &raster,
            &plan,
            tile_format,
            engine_config,
            resume_mode,
            start,
        );
    } else if let Some(rest) = sink_uri.strip_prefix("packfile://") {
        run_pyramid_packfile(
            rest,
            &args,
            &raster,
            &plan,
            tile_format,
            engine_config,
            resume_mode,
            start,
        );
    } else {
        // fs:// (strip optional scheme prefix)
        let base_dir = if let Some(p) = sink_uri.strip_prefix("fs://") {
            PathBuf::from(p)
        } else {
            args.output.clone()
        };

        // Build FsSink with Phase 3 options
        // @doc-snippet:begin slot=sink-fs imports=FsSink,TileFormat,ChecksumMode,ChecksumAlgo,ManifestBuilder
        let mut sink = FsSink::new(&base_dir, plan.clone())
            // @doc-test: builder_sink_fs.rs::with_format_overrides_default:62
            .with_format(tile_format); // @doc-flag: format kind=param param_name=format
        // @doc-test: builder_sink_fs.rs::with_format_overrides_default:62
        // @doc-flag: quality kind=param param_name=quality
        if let Some(mb) = manifest_builder {
            // @doc-test: builder_sink_fs.rs::compose_format_checksums_manifest_resume:110
            sink = sink.with_manifest(mb); // @doc-flag: manifest-emit-checksums kind=append
        }
        if args.manifest_emit_checksums {
            // @doc-test: phase3_checksum.rs::emit_only_populates_manifest_checksums:223
            sink = sink.with_checksums(ChecksumMode::EmitOnly, checksum_algo); // @doc-flag: checksum-algo kind=param param_name=checksum-algo
        }
        if let Some(ds) = build_dedupe_strategy(&args) {
            sink = sink.with_dedupe(ds);
        }
        if args.resume {
            sink = sink.with_resume(true);
        }
        // @doc-snippet:end slot=sink-fs

        // The resumable entry point honours `resume_mode` (Overwrite wipes
        // the output, Resume continues from a checkpoint, Verify checks).
        // The streaming / MapReduce paths do not yet understand resume modes,
        // so we only route through `run_generate` when `--memory-budget` is
        // supplied *and* the user has not explicitly asked for resume/verify.
        let result = if args.memory_budget.is_some()
            && !matches!(resume_mode, ResumeMode::Resume | ResumeMode::Verify)
        {
            // Budgeted streaming / MapReduce path. Default here is still
            // overwrite-in-place (no wipe); that's acceptable because the
            // user opted into a different engine.
            run_generate(&args, &raster, &plan, &sink, engine_config, start)
        } else {
            let policy = match resume_mode {
                ResumeMode::Overwrite => ResumePolicy::overwrite(),
                ResumeMode::Resume => ResumePolicy::resume(),
                ResumeMode::Verify => ResumePolicy::verify(),
            };
            match EngineBuilder::new(&raster, plan.clone(), &sink)
                .with_config(engine_config.clone())
                .with_resume(policy)
                .run()
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error generating pyramid: {e}");
                    process::exit(1);
                }
            }
        };

        finish_run(result, &base_dir, start);
    }
}

/// Entry point for the monolithic / streaming / mapreduce generation paths
/// (filesystem sink only).  Returns the [`libviprs::EngineResult`] for
/// summary printing.
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
    _start: Instant,
) -> libviprs::EngineResult {
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
        .with_failure_policy(engine_config.failure_policy.clone());
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

    match builder.run() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error generating pyramid: {e}");
            process::exit(1);
        }
    }
    // @doc-snippet:end slot=engine-builder
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
// S3 sink dispatch (feature-gated)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_pyramid_s3(
    _rest: &str,
    _args: &PyramidArgs,
    _raster: &Raster,
    _plan: &libviprs::PyramidPlan,
    _tile_format: TileFormat,
    _engine_config: EngineConfig,
    _resume_mode: ResumeMode,
    _start: Instant,
) {
    #[cfg(feature = "s3")]
    {
        // @doc-snippet:begin slot=sink-s3 imports=ObjectStoreSink
        // TODO Phase 3: parse bucket/prefix from _rest, build ObjectStoreConfig,
        // construct ObjectStoreSink, run generate_pyramid_resumable or
        // generate_pyramid_observed as appropriate.
        // @doc-test: phase3_packfile.rs::tar_sink_produces_valid_archive:177
        // @doc-flag: sink kind=override
        eprintln!("Error: s3:// sink is not yet fully wired (Phase 3 TODO).");
        process::exit(2);
        // @doc-snippet:end slot=sink-s3
    }
    #[cfg(not(feature = "s3"))]
    {
        eprintln!("Error: s3:// sink requires the `s3` feature — rebuild with `--features s3`.");
        process::exit(2);
    }
}

// ---------------------------------------------------------------------------
// Packfile sink dispatch (feature-gated)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_pyramid_packfile(
    _path: &str,
    _args: &PyramidArgs,
    _raster: &Raster,
    _plan: &libviprs::PyramidPlan,
    _tile_format: TileFormat,
    _engine_config: EngineConfig,
    _resume_mode: ResumeMode,
    _start: Instant,
) {
    #[cfg(feature = "packfile")]
    {
        use libviprs::{PackfileFormat, PackfileSink};

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
            Err(e) => {
                eprintln!("Error creating packfile sink: {e}");
                process::exit(1);
            }
        };
        // @doc-snippet:end slot=sink-packfile

        let policy = match _resume_mode {
            ResumeMode::Overwrite => ResumePolicy::overwrite(),
            ResumeMode::Resume => ResumePolicy::resume(),
            ResumeMode::Verify => ResumePolicy::verify(),
        };
        let result = match EngineBuilder::new(_raster, _plan.clone(), &sink)
            .with_config(_engine_config.clone())
            .with_resume(policy)
            .run()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error generating pyramid: {e}");
                process::exit(1);
            }
        };

        finish_run(result, sink.out_path(), _start);
    }
    #[cfg(not(feature = "packfile"))]
    {
        eprintln!(
            "Error: packfile:// sink requires the `packfile` feature — rebuild with `--features packfile`."
        );
        process::exit(2);
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
        match libviprs::decode_file(path) {
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
                eprintln!("Error reading image: {e}");
                process::exit(1);
            }
        }
    }
}

fn run_plan(args: PlanArgs) {
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
        let h = args.height.unwrap_or_else(|| {
            eprintln!("--height is required when width is given as a number");
            process::exit(1);
        });
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
                    Some(p) => {
                        let scale = args.dpi as f64 / 72.0;
                        let w = (p.width_pts * scale) as u32;
                        let h = (p.height_pts * scale) as u32;
                        (w, h)
                    }
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
        match libviprs::decode_file(&path) {
            Ok(raster) => (raster.width(), raster.height()),
            Err(e) => {
                eprintln!("Error reading image: {e}");
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
        match libviprs::decode_bytes(&buf) {
            Ok(r) => return r,
            Err(e) => {
                eprintln!("Error decoding image from stdin: {e}");
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

    // @doc-snippet:begin slot=load-source imports=Raster,extract_page_image,render_page_pdfium,decode_file
    if ext == "pdf" {
        // @doc-test: pdfium_integration.rs::libviprs_pdfium_render_paths:34
        if args.render {
            // @doc-flag: render kind=override
            // `--render` requires the `pdfium` feature. When the binary is built
            // `--no-default-features` (pdfium-free), `render_page_pdfium` is absent,
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
                match render_page_pdfium(&path, args.page, args.dpi) {
                    // @doc-flag: dpi kind=param param_name=dpi
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
                            Some(p) => {
                                let scale = args.dpi as f64 / 72.0;
                                let w = (p.width_pts * scale) as u32;
                                let h = (p.height_pts * scale) as u32;
                                (w, h)
                            }
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
        match libviprs::decode_file(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error decoding image: {e}");
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

fn parse_coord_pair(s: &str, name: &str) -> (f64, f64) {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        eprintln!("Invalid --{name}: expected \"x,y\", got \"{s}\"");
        process::exit(1);
    }
    let x = parts[0].trim().parse::<f64>().unwrap_or_else(|e| {
        eprintln!("Invalid --{name} x value \"{}\": {e}", parts[0]);
        process::exit(1);
    });
    let y = parts[1].trim().parse::<f64>().unwrap_or_else(|e| {
        eprintln!("Invalid --{name} y value \"{}\": {e}", parts[1]);
        process::exit(1);
    });
    (x, y)
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
    fn help_does_not_advertise_s3() {
        // The s3:// sink is a compiled-in stub, so the help must not advertise
        // an `s3://` scheme users cannot actually use.
        use clap::CommandFactory;
        let help = PyramidArgs::command().render_long_help().to_string();
        assert!(
            !help.contains("s3://"),
            "help text must not advertise the unimplemented s3:// sink scheme, got:\n{help}"
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

    #[test]
    fn sink_uri_defaults_to_fs() {
        let args = parse_pyramid(&[]).expect("bare invocation must parse");
        assert_eq!(resolve_sink_uri(&args), "fs://out");
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
