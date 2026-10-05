//! The pipeline controls on `viprs pyramid`, and the driver every pyramid run
//! goes through (libviprs-cli#66).
//!
//! The core grew a PMTiles layout choice, a dedupe budget and ordered emission
//! (EPIC M), resume and retry knobs, cancellation, progress events, region runs
//! and `skip_blanks`, and none of it had a flag. This module is where those
//! flags live and where a run that can use them is driven.
//!
//! # Which runs come through here
//!
//! [`run`] drives every run into a PMTiles archive, a loose tree or an object
//! store on the monolithic engine, which is all of them by default. Two runs
//! stay on the drivers in `main.rs`: `--packfile`, and a fresh `--memory-budget`
//! run into a tile tree (the streaming and MapReduce engines). A pipeline flag
//! given to either of those is a usage error rather than a flag that silently
//! does nothing. They still share this module's [`prepare`], [`engine_config`],
//! [`tree_sink`] and [`conclude`], so there is one copy of the plan, the memory
//! check, the engine config and the exit codes, and Ctrl-C is installed before
//! either driver is picked.
//!
//! The `@doc-snippet` and `@doc-flag` markers libviprs.org reads sit here for
//! the same reason: they annotate the code that runs.
//!
//! # Two flags that look like older ones
//!
//! `--pmtiles-layout` is not `--layout`. `--layout` picks the pyramid scheme
//! (Deep Zoom, XYZ, Google), which decides what a tile *is*; this one picks
//! where the PMTiles writer puts the tile bytes inside the archive, which
//! decides nothing about the tiles at all.
//!
//! `--drop-blanks` is not `--skip-blank`. The older flag writes a one-byte
//! placeholder for every blank tile, so the file count is unchanged; this one
//! drops blank tiles from the output altogether, the way `dzsave --skip-blanks`
//! does. The two cannot be given together.

use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Args, ValueEnum};
use libviprs::observe::{EngineEvent, EngineObserver};
use libviprs::pmtiles::writer::{DEDUPE_BYTES_PER_PAYLOAD, DEDUPE_WINDOW_WAYS};
use libviprs::pmtiles::{Layout as PmTilesLayout, WriterOptions};
use libviprs::resume::DEFAULT_RESUME_CHECKPOINT_EVERY;
use libviprs::{
    CancelToken, ChecksumAlgo, ChecksumMode, EngineBuilder, EngineConfig, EngineError,
    EngineResult, FailurePolicy, FsSink, Layout, ManifestBuilder, PixelFormat, PmTilesSink,
    PyramidPlan, PyramidPlanner, Raster, ResumeMode, ResumePolicy, RetryPolicy, TileCoord,
    TileFormat,
};

use crate::{PyramidArgs, operational_error, usage_error};

/// What one deduplication set costs the PMTiles writer: the writer's own
/// `WriterOptions::MIN_DEDUPE_MEMORY_BYTES` (libviprs#1169), one set of
/// [`DEDUPE_WINDOW_WAYS`] payloads at [`DEDUPE_BYTES_PER_PAYLOAD`] each.
/// Below this the writer quietly rounds up to one set, which is a budget the
/// caller did not ask for, so the flag refuses instead.
pub(crate) const DEDUPE_MEMORY_FLOOR: usize = WriterOptions::MIN_DEDUPE_MEMORY_BYTES;

/// Exit status for a run stopped by SIGINT, the shell's own convention.
const EXIT_INTERRUPTED: i32 = 130;

/// The version of the `--events json` line format. Every line carries it as
/// `"v"`, so a consumer can tell a format it knows from one it does not. The
/// event names are core's (`EngineEvent::name`), so a core
/// `EngineEvent::NAMES_VERSION` bump is a bump here too; a unit test holds the
/// two together.
const EVENTS_SCHEMA_VERSION: u32 = 1;

/// The pipeline flags, flattened into `viprs pyramid`'s arguments.
#[derive(Args, Clone, Debug)]
pub(crate) struct PipelineArgs {
    /// Where the PMTiles writer puts the tile bytes: `tile-id` or `arrival`.
    ///
    /// This is not `--layout`. `--layout` chooses the pyramid scheme (what a
    /// tile is and how it is addressed); this chooses the order of the tile
    /// data inside one archive, and changes no tile.
    ///
    /// `tile-id` (the default) sorts at the end and writes the data in tile id
    /// order: the archive is a pure function of the tiles and is always
    /// clustered, at the cost of a scratch copy. `arrival` appends tiles as
    /// they come and writes every byte once; the archive then depends on the
    /// order tiles arrived in, and is clustered only if that order was tile id
    /// order (see `--ordered-emission`). Archives only.
    #[arg(long, value_name = "ORDER", help_heading = "PMTiles")]
    pub(crate) pmtiles_layout: Option<PmtilesLayoutArg>,

    /// Ask the engine for tiles in ascending tile id order.
    ///
    /// Under `--pmtiles-layout arrival` this makes the archive byte-identical
    /// to the `tile-id` one with no reordering pass. It costs memory: every
    /// level's raster is held at once instead of one at a time, and the
    /// `--memory-limit` estimate counts them. Archives only.
    #[arg(long, help_heading = "PMTiles")]
    pub(crate) ordered_emission: bool,

    /// Memory the PMTiles writer may spend recognising duplicate tiles.
    ///
    /// About 65 bytes per distinct tile remembered; the default is 8 MiB.
    /// Two identical tiles further apart than this window are stored twice,
    /// which is still a correct archive, just a larger one. Below 520 bytes
    /// (one dedupe set) the value is refused. Above a slot for every tile in
    /// the plan it is lowered to that, and the run says so, because the writer
    /// allocates the whole window up front. Archives only.
    #[arg(long, value_name = "BYTES", help_heading = "PMTiles")]
    pub(crate) dedupe_memory_bytes: Option<usize>,

    /// Write the resume checkpoint every N tiles.
    ///
    /// Defaults to 1000 for a tile tree, so Ctrl-C leaves a job `--resume`
    /// can pick up. `0` writes it only when the run finishes. Tile trees only:
    /// a PMTiles archive cannot be resumed.
    #[arg(long, value_name = "N", help_heading = "Resume")]
    pub(crate) checkpoint_every: Option<u64>,

    /// Keep the resume checkpoint here instead of inside the output tree.
    #[arg(long, value_name = "DIR", help_heading = "Resume")]
    pub(crate) checkpoint_root: Option<PathBuf>,

    /// Retry a failed tile write up to N times, then fail the run.
    ///
    /// Add `--skip-failed` to skip a tile that still fails and carry on
    /// instead. Shorthand for `--on-failure retry=N,...`, so the two cannot
    /// be combined.
    #[arg(
        long,
        value_name = "N",
        conflicts_with = "on_failure",
        help_heading = "Reliability"
    )]
    pub(crate) retries: Option<u32>,

    /// First retry delay in milliseconds (doubles on each attempt). Default 100.
    #[arg(
        long,
        value_name = "MS",
        requires = "retries",
        help_heading = "Reliability"
    )]
    pub(crate) retry_backoff_ms: Option<u64>,

    /// Abort the run on the first tile that fails (the default).
    ///
    /// With `--retries N`, abort once a tile has failed all N retries, which
    /// is what `--retries` does on its own anyway.
    #[arg(
        long,
        conflicts_with_all = ["on_failure", "skip_failed"],
        help_heading = "Reliability"
    )]
    pub(crate) fail_fast: bool,

    /// Skip a tile that still fails after its retries, and carry on.
    ///
    /// The output then has holes, so the run still exits 1 at the end and
    /// says how many tiles it skipped. Without `--retries` a failing tile is
    /// skipped on its first failure.
    #[arg(long, conflicts_with = "on_failure", help_heading = "Reliability")]
    pub(crate) skip_failed: bool,

    /// Record a per-tile checksum in the manifest and re-hash every tile on
    /// disk before the run reports success. Tile trees only.
    ///
    /// `viprs verify <tree>` checks a tree against these afterwards.
    #[arg(long, help_heading = "Manifest")]
    pub(crate) checksum: bool,

    /// Record the BLAKE3 of the source file's bytes in the manifest.
    ///
    /// Tile trees only, and not for an input read from stdin, which leaves no
    /// file to hash.
    #[arg(long, help_heading = "Manifest")]
    pub(crate) manifest_source_hash: bool,

    /// Pyramid only this rectangle of the input, `x,y,width,height` in pixels.
    ///
    /// Crop first, then pyramid: the output is exactly what the cropped image
    /// would give. Not resumable, and emits no per-tile events.
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_region)]
    pub(crate) region: Option<Region>,

    /// Leave blank (single-colour) tiles out of the output entirely.
    ///
    /// Not `--skip-blank`, which writes a one-byte placeholder for each blank
    /// tile and so keeps one file per tile; the two cannot be combined.
    /// `viprs verify` needs `--drop-blanks` as well to check such a pyramid.
    #[arg(
        long,
        conflicts_with_all = ["skip_blank", "blank_tolerance"],
        help_heading = "Dedupe"
    )]
    pub(crate) drop_blanks: bool,

    /// Print one line per engine event on stdout: `none`, `text` or `json`.
    ///
    /// Each `json` line is an object with `"v": 1` and an `"event"` name.
    #[arg(long, value_name = "FORMAT", default_value = "none")]
    pub(crate) events: EventsArg,

    /// Directory the `s3://bucket/prefix` sink writes into (needs the `s3` feature).
    ///
    /// Hidden: this build carries no network transport, so the object-store
    /// sink writes through a local stub store (object `KEY` in bucket `B`
    /// lands at `DIR/B/KEY`). That is a test seam, not something to point a
    /// job at, and it stays out of the help until a real transport exists.
    #[arg(long, value_name = "DIR", hide = true)]
    pub(crate) object_store_root: Option<PathBuf>,
}

/// `--pmtiles-layout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum PmtilesLayoutArg {
    /// Sort at the end and write the tile data in tile id order (default).
    TileId,
    /// Append tile data in the order tiles arrive.
    Arrival,
}

impl From<PmtilesLayoutArg> for PmTilesLayout {
    fn from(arg: PmtilesLayoutArg) -> Self {
        match arg {
            PmtilesLayoutArg::TileId => PmTilesLayout::TileId,
            PmtilesLayoutArg::Arrival => PmTilesLayout::Arrival,
        }
    }
}

/// `--events`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum EventsArg {
    None,
    Text,
    Json,
}

/// `--region x,y,width,height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn parse_region(s: &str) -> Result<Region, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let [x, y, w, h] = parts.as_slice() else {
        return Err(format!(
            "expected four comma-separated numbers `x,y,width,height`, got {s:?}"
        ));
    };
    let num = |what: &str, text: &str| {
        text.parse::<u32>()
            .map_err(|e| format!("{what} {text:?} in --region is not a pixel count: {e}"))
    };
    let region = Region {
        x: num("x", x)?,
        y: num("y", y)?,
        width: num("width", w)?,
        height: num("height", h)?,
    };
    if region.width == 0 || region.height == 0 {
        return Err("a --region needs a non-zero width and height".to_string());
    }
    Ok(region)
}

impl PipelineArgs {
    /// Names of the pipeline flags that were given, for the refusal on the
    /// paths that do not come through here.
    fn given(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut push = |on: bool, name| {
            if on {
                out.push(name);
            }
        };
        push(self.pmtiles_layout.is_some(), "--pmtiles-layout");
        push(self.ordered_emission, "--ordered-emission");
        push(self.dedupe_memory_bytes.is_some(), "--dedupe-memory-bytes");
        push(self.checkpoint_every.is_some(), "--checkpoint-every");
        push(self.checkpoint_root.is_some(), "--checkpoint-root");
        push(self.retries.is_some(), "--retries");
        push(self.fail_fast, "--fail-fast");
        push(self.skip_failed, "--skip-failed");
        push(self.checksum, "--checksum");
        push(self.manifest_source_hash, "--manifest-source-hash");
        push(self.region.is_some(), "--region");
        push(self.drop_blanks, "--drop-blanks");
        push(self.events != EventsArg::None, "--events");
        push(self.object_store_root.is_some(), "--object-store-root");
        out
    }

    fn pmtiles_only(&self) -> Option<&'static str> {
        if self.pmtiles_layout.is_some() {
            Some("--pmtiles-layout")
        } else if self.ordered_emission {
            Some("--ordered-emission")
        } else if self.dedupe_memory_bytes.is_some() {
            Some("--dedupe-memory-bytes")
        } else {
            None
        }
    }

    fn tree_only(&self) -> Option<&'static str> {
        if self.checkpoint_every.is_some() {
            Some("--checkpoint-every")
        } else if self.checkpoint_root.is_some() {
            Some("--checkpoint-root")
        } else if self.checksum {
            Some("--checksum")
        } else if self.manifest_source_hash {
            Some("--manifest-source-hash")
        } else {
            None
        }
    }

    /// The failure policy these flags spell, or `None` to keep `--on-failure`.
    ///
    /// `--retries N` retries and then fails, like `--on-failure retry=N,..`;
    /// only `--skip-failed` turns a tile that keeps failing into a hole.
    fn failure_policy(&self) -> Option<FailurePolicy> {
        let policy = || {
            let backoff = Duration::from_millis(self.retry_backoff_ms.unwrap_or(100));
            let policy = RetryPolicy::new(self.retries.unwrap_or(0), backoff);
            // The core caps every delay at `max_backoff` (5 s by default). A
            // first delay above that would be clamped without a word, so the
            // cap moves up to meet it instead.
            if backoff > policy.max_backoff {
                policy.with_max_backoff(backoff)
            } else {
                policy
            }
        };
        if self.skip_failed {
            Some(FailurePolicy::RetryThenSkip(policy()))
        } else if self.retries.is_some() {
            Some(FailurePolicy::RetryThenFail(policy()))
        } else if self.fail_fast {
            Some(FailurePolicy::FailFast)
        } else {
            None
        }
    }
}

/// A flag combination with no meaning: what was wrong and what fixes it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    message: String,
    hint: String,
}

impl Refusal {
    fn new(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: hint.into(),
        }
    }

    /// Report it as a usage error (exit 2).
    fn exit(self) -> ! {
        usage_error(&self.message, &self.hint)
    }
}

/// Whether this run comes through the pipeline, refusing pipeline flags on a
/// path that would ignore them.
///
/// `--memory-budget` only means something to a fresh run into a tile tree:
/// that is the one place `main.rs` hands it to the streaming engines. Into an
/// archive, an object store, or with `--resume`/`--verify`, the run comes here
/// and says the budget does not apply, which is what it always did without
/// saying so.
pub(crate) fn takes_over(args: &PyramidArgs) -> bool {
    let legacy = if args.packfile
        || args
            .sink
            .as_deref()
            .is_some_and(|u| u.starts_with("packfile://"))
    {
        Some("--packfile")
    } else if budget_streams(args) {
        Some("--memory-budget")
    } else {
        None
    };
    let Some(path) = legacy else { return true };
    let given = args.pipeline.given();
    if !given.is_empty() {
        usage_error(
            &format!("{} cannot be combined with {path}", given.join(", ")),
            "the packfile sink and the streaming engines do not take these controls yet; \
             drop them, or drop the flag that picked that path",
        );
    }
    false
}

/// Whether `--memory-budget` reaches the streaming engines on this run.
fn budget_streams(args: &PyramidArgs) -> bool {
    args.memory_budget.is_some()
        && !args.resume
        && !args.verify
        && matches!(target_of(args), Ok(Target::Tree(_)))
}

/// Where the run is going, resolved from the sink URI.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Archive(PathBuf),
    Tree(PathBuf),
    ObjectStore { bucket: String, prefix: String },
}

fn target_of(args: &PyramidArgs) -> Result<Target, Refusal> {
    resolve_target(args, &crate::resolve_sink_uri(args))
}

fn resolve_target(args: &PyramidArgs, uri: &str) -> Result<Target, Refusal> {
    if let Some(path) = uri.strip_prefix("pmtiles://") {
        Ok(Target::Archive(PathBuf::from(path)))
    } else if let Some(rest) = uri.strip_prefix("s3://") {
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() {
            return Err(Refusal::new(
                format!("{uri} names no bucket"),
                "spell it s3://bucket/prefix",
            ));
        }
        // The stub store puts a bucket at ROOT/bucket, so a bucket of `..`
        // would write beside the root rather than under it.
        let mut parts = Path::new(bucket).components();
        if !matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return Err(Refusal::new(
                format!("{bucket:?} in {uri} is not a bucket name"),
                "a bucket is one plain name, such as s3://tiles/run-1",
            ));
        }
        Ok(Target::ObjectStore {
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
        })
    } else if let Some(path) = uri.strip_prefix("fs://") {
        Ok(Target::Tree(PathBuf::from(path)))
    } else if let Some(output) = args.output.clone() {
        // An unknown scheme has always fallen through to the positional output.
        Ok(Target::Tree(output))
    } else {
        Ok(Target::Tree(PathBuf::from(uri)))
    }
}

/// Every refusal that depends only on the flags, before anything is read.
fn check_combinations(args: &PyramidArgs, target: &Target) -> Result<(), Refusal> {
    let p = &args.pipeline;
    if let Some(bytes) = p.dedupe_memory_bytes
        && bytes < DEDUPE_MEMORY_FLOOR
    {
        return Err(Refusal::new(
            format!("--dedupe-memory-bytes {bytes} is below the {DEDUPE_MEMORY_FLOOR}-byte floor"),
            format!(
                "one dedupe set of {DEDUPE_WINDOW_WAYS} payloads at {DEDUPE_BYTES_PER_PAYLOAD} bytes each is the \
                 smallest window the writer has, and it would round {bytes} up to that \
                 without saying so. Ask for {DEDUPE_MEMORY_FLOOR} or more"
            ),
        ));
    }
    let is_archive = matches!(target, Target::Archive(_));
    let is_tree = matches!(target, Target::Tree(_));
    if !is_archive && let Some(flag) = p.pmtiles_only() {
        return Err(Refusal::new(
            format!("{flag} only applies to a PMTiles archive"),
            "it configures the PMTiles writer; drop it, or drop --storage directory / --sink",
        ));
    }
    if !is_tree {
        if let Some(flag) = p.tree_only() {
            return Err(Refusal::new(
                format!("{flag} only applies to a tile tree (--storage directory)"),
                "an archive keeps no manifest and cannot be resumed",
            ));
        }
        if args.resume {
            return Err(Refusal::new(
                "--resume only applies to a tile tree (--storage directory)",
                "a PMTiles archive cannot be resumed: the writer's staging is not \
                 reconstructible from a checkpoint, so rerun without --resume",
            ));
        }
    }
    if p.object_store_root.is_some() && !matches!(target, Target::ObjectStore { .. }) {
        return Err(Refusal::new(
            "--object-store-root only applies to an s3:// sink",
            "pass --sink s3://bucket/prefix as well, or drop it",
        ));
    }
    if p.manifest_source_hash && args.input == "-" {
        return Err(Refusal::new(
            "--manifest-source-hash cannot hash an input read from stdin",
            "it records the hash of the source file's bytes, and stdin leaves no file to \
             hash; name the input file, or drop the flag",
        ));
    }
    if p.region.is_some() {
        let resumable = args.resume || p.checkpoint_every.is_some() || p.checkpoint_root.is_some();
        if resumable {
            return Err(Refusal::new(
                "--region runs are not resumable",
                "a region run crops first and keeps no checkpoint; drop --resume and the \
                 checkpoint flags",
            ));
        }
        if p.events != EventsArg::None {
            return Err(Refusal::new(
                "--region emits no per-tile events",
                "drop --events, or pre-crop the input and run without --region",
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// What both drivers share
// ---------------------------------------------------------------------------

/// The decoded input and the plan, once the memory check has passed.
pub(crate) struct Prepared {
    pub(crate) raster: Raster,
    pub(crate) plan: PyramidPlan,
}

/// Decode the input, build the plan and refuse a run that would not fit
/// `--memory-limit`. Every driver starts here.
///
/// A `--region` out of bounds is refused here too. Both checks run first on
/// the input's header, before anything is decoded (libviprs-cli#93), and
/// again on the decoded raster, which is the only check for an input whose
/// header can't be read on its own (stdin, a PDF, a GIF and the other
/// containers the core only describes by decoding). Nothing has been written
/// by either point.
pub(crate) fn prepare(args: &PyramidArgs, layout: Layout) -> Prepared {
    crate::maybe_init_tracing(&args.trace_level);
    precheck_header(args, layout);
    let raster = crate::load_source(args);
    let (w, h) = (raster.width(), raster.height());
    eprintln!(
        "Source: {}x{} {:?} ({:.1} MB)",
        w,
        h,
        raster.format(),
        raster.data().len() as f64 / (1024.0 * 1024.0)
    );
    if let Some(geo) = crate::build_geo_transform(args, w, h) {
        let bounds = geo.image_bounds(w, h);
        eprintln!(
            "Geo bounds: ({:.6}, {:.6}) → ({:.6}, {:.6})",
            bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y
        );
    }

    let (plan_w, plan_h) = region_extent(args, w, h);

    // @doc-snippet:begin slot=planner imports=PyramidPlanner,Layout
    let planner = match PyramidPlanner::new(
        plan_w,
        plan_h,
        // @doc-test: blank_tile_strategy.rs::emit_solid_white_matches_expected:138
        args.tile_size, // @doc-flag: tile-size kind=param param_name=tile-size
        // @doc-test: builder_sink_fs.rs::two_arg_new_defaults_to_png:47
        args.overlap, // @doc-flag: overlap kind=param param_name=overlap
        // @doc-test: google_centre_pyramid.rs::google_centre_portrait_plan_structure:107
        layout, // @doc-flag: layout kind=param param_name=layout
    ) {
        // @doc-test: google_centre_pyramid.rs::google_centre_portrait_plan_structure:107
        Ok(p) => p.with_centre(args.centre), // @doc-flag: centre kind=append
        Err(e) => operational_error(&format!("creating pyramid plan: {e}")),
    };
    // @doc-snippet:end slot=planner
    let plan = planner.plan();

    let extra = ExtraMemory::of(args, &plan, raster.format());
    let peak_memory = planner.estimate_peak_memory().saturating_add(extra.total());
    print_estimate(&planner, &extra, peak_memory, plan_w, plan_h);

    // @doc-snippet:begin slot=memory-limit
    // @doc-test: streaming_engine.rs::estimate_streaming_memory_reasonable:435
    if args.memory_limit > 0 {
        // @doc-flag: memory-limit kind=param param_name=memory-limit
        let limit_bytes = crate::mb_to_bytes(args.memory_limit);
        if peak_memory > limit_bytes {
            over_memory_limit(peak_memory, args.memory_limit);
        }
    }
    // @doc-snippet:end slot=memory-limit

    eprintln!(
        "Plan: {} levels, {} tiles, tile_size={}, overlap={}",
        plan.level_count(),
        plan.total_tile_count(),
        args.tile_size,
        args.overlap
    );
    Prepared { raster, plan }
}

/// The `--region` checks and the price, from the input's header alone
/// (libviprs-cli#93), so a region outside the image or a run over
/// `--memory-limit` is refused without spending the decode.
///
/// Returns without a word whenever the header can't be read on its own (see
/// [`crate::input::probe_path`]), or the plan can't be built from it: the
/// checks after the decode then run as they always have and say what's wrong.
/// The decode's width and height are the header's, so a refusal here is the
/// one the decode would have led to. The pixel format can differ in one known
/// case (an Ultra HDR file with a greyscale base probes as `Gray8` and decodes
/// as `Rgb8`), which can only make this price lower than the real one, so the
/// check after the decode still catches it.
fn precheck_header(args: &PyramidArgs, layout: Layout) {
    if args.input == "-"
        || Path::new(&args.input)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return;
    }
    let Some(header) = crate::input::probe_path(Path::new(&args.input)) else {
        return;
    };
    let (plan_w, plan_h) = region_extent(args, header.width, header.height);
    if args.memory_limit == 0 {
        return;
    }
    let Ok(planner) = PyramidPlanner::new(plan_w, plan_h, args.tile_size, args.overlap, layout)
    else {
        return;
    };
    let planner = planner.with_centre(args.centre);
    let plan = planner.plan();
    let extra = ExtraMemory::of(args, &plan, header.format);
    let peak_memory = planner.estimate_peak_memory().saturating_add(extra.total());
    if peak_memory > crate::mb_to_bytes(args.memory_limit) {
        print_estimate(&planner, &extra, peak_memory, plan_w, plan_h);
        over_memory_limit(peak_memory, args.memory_limit);
    }
}

/// The extent the plan covers: the `--region` when there is one, which has to
/// fit inside the `w` x `h` input (a usage error otherwise), or else the
/// whole input.
fn region_extent(args: &PyramidArgs, w: u32, h: u32) -> (u32, u32) {
    let Some(r) = args.pipeline.region else {
        return (w, h);
    };
    let fits = r.x.checked_add(r.width).is_some_and(|e| e <= w)
        && r.y.checked_add(r.height).is_some_and(|e| e <= h);
    if !fits {
        usage_error(
            &format!(
                "--region {},{},{},{} falls outside the {w}x{h} input",
                r.x, r.y, r.width, r.height
            ),
            "x + width and y + height must stay within the image",
        );
    }
    (r.width, r.height)
}

fn print_estimate(
    planner: &PyramidPlanner,
    extra: &ExtraMemory,
    peak_memory: u64,
    plan_w: u32,
    plan_h: u32,
) {
    let (canvas_w, canvas_h) = planner.canvas_dimensions();
    eprintln!(
        "Memory estimate: {:.1} MB peak (canvas: {}x{}, source: {}x{})",
        mb(peak_memory),
        canvas_w,
        canvas_h,
        plan_w,
        plan_h
    );
    if extra.level_rasters > 0 {
        eprintln!(
            "  including {:.1} MB for the level rasters --ordered-emission holds at once",
            mb(extra.level_rasters)
        );
    }
    if extra.dedupe_window > 0 {
        eprintln!(
            "  including {:.1} MB for the --dedupe-memory-bytes window",
            mb(extra.dedupe_window)
        );
    }
}

/// Refuse a run whose estimate is over `--memory-limit`, with exit 1.
fn over_memory_limit(peak_memory: u64, limit_mb: u64) -> ! {
    eprintln!(
        "Error: estimated peak memory ({:.1} MB) exceeds --memory-limit ({} MB)",
        mb(peak_memory),
        limit_mb
    );
    eprintln!("Hint: reduce --dpi or image dimensions to lower memory usage");
    process::exit(1);
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Memory the pipeline flags cost on top of the planner's estimate, which
/// only knows about the source and the canvas.
struct ExtraMemory {
    /// `--ordered-emission` holds every level below the top at once.
    level_rasters: u64,
    /// The dedupe window `--dedupe-memory-bytes` asks the writer for, as
    /// lowered by [`dedupe_window_bytes`].
    dedupe_window: u64,
}

impl ExtraMemory {
    fn of(args: &PyramidArgs, plan: &PyramidPlan, format: PixelFormat) -> Self {
        let p = &args.pipeline;
        Self {
            level_rasters: if p.ordered_emission {
                lower_level_bytes(plan, format)
            } else {
                0
            },
            dedupe_window: p
                .dedupe_memory_bytes
                .map_or(0, |b| dedupe_window_bytes(b, plan) as u64),
        }
    }

    fn total(&self) -> u64 {
        self.level_rasters.saturating_add(self.dedupe_window)
    }
}

/// The bytes of every level raster below the top one.
fn lower_level_bytes(plan: &PyramidPlan, format: PixelFormat) -> u64 {
    let bpp = format.bytes_per_pixel() as u64;
    let top = plan.levels.iter().map(|l| l.level).max();
    plan.levels
        .iter()
        .filter(|l| Some(l.level) != top)
        .map(|l| u64::from(l.width) * u64::from(l.height) * bpp)
        .fold(0u64, u64::saturating_add)
}

/// The dedupe window the writer gets for a `--dedupe-memory-bytes` of
/// `requested` on this plan.
///
/// The writer allocates the whole window up front and touches every page of
/// it, so a budget far past what the plan can fill is memory spent for
/// nothing (or an abort). It is lowered to whole sets with a slot for every
/// planned tile. A set-associative window that size can still evict on an
/// unlucky hash, which only means an occasional duplicate is stored twice; the
/// archive is correct either way.
fn dedupe_window_bytes(requested: usize, plan: &PyramidPlan) -> usize {
    let tiles = usize::try_from(plan.total_tile_count()).unwrap_or(usize::MAX);
    let sets = tiles.div_ceil(DEDUPE_WINDOW_WAYS).max(1);
    requested.min(sets.saturating_mul(DEDUPE_MEMORY_FLOOR))
}

/// The engine config: the flags `main.rs` already reads, plus this module's.
pub(crate) fn engine_config(args: &PyramidArgs) -> EngineConfig {
    // @doc-snippet:begin slot=engine-config imports=EngineConfig,BlankTileStrategy,FailurePolicy,DedupeStrategy,RetryPolicy
    let mut config = EngineConfig::default()
        // @doc-test: builder_engine_surface.rs::builder_honours_with_concurrency:100
        .with_concurrency(args.concurrency) // @doc-flag: concurrency kind=appendChain
        // @doc-test: builder_engine_surface.rs::builder_honours_with_buffer_size:119
        .with_buffer_size(args.buffer_size) // @doc-flag: buffer-size kind=appendChain
        // @doc-test: blank_tile_strategy.rs::placeholder_solid_white_matches_expected:201
        .with_blank_tile_strategy(crate::build_blank_tile_strategy(args)) // @doc-flag: skip-blank kind=append
        // @doc-test: phase3_blank_tolerance.rs::engine_with_tolerance_writes_placeholder_for_near_white_tiles:248
        // @doc-flag: blank-tolerance kind=append
        // @doc-test: phase3_retry.rs::retries_on_transient_errors:256
        // @doc-flag: retry-max kind=param param_name=retry-max
        // @doc-test: phase3_retry.rs::retries_on_transient_errors:256
        // @doc-flag: retry-backoff kind=param param_name=retry-backoff
        // @doc-test: builder_resume_retry.rs::builder_with_failure_policy_accepts_every_variant:145
        .with_failure_policy(
            args.pipeline
                .failure_policy()
                .unwrap_or_else(|| crate::build_failure_policy(args)),
        ) // @doc-flag: failure-policy kind=param param_name=failure-policy
        .skip_blanks(args.pipeline.drop_blanks);
    if let Some(ds) = crate::build_dedupe_strategy(args) {
        // @doc-test: phase3_dedupe_blanks.rs::blanks_dedupe_manifest_lists_references:364
        // @doc-flag: dedupe-blanks kind=append
        // @doc-test: phase3_dedupe_blanks.rs::all_mode_dedupes_identical_non_blank_tiles:467
        config = config.with_dedupe_strategy(ds); // @doc-flag: dedupe-all kind=append
    }
    // @doc-snippet:end slot=engine-config
    config
}

/// The filesystem sink with its manifest and checksum options.
pub(crate) fn tree_sink(
    args: &PyramidArgs,
    dir: &Path,
    plan: &PyramidPlan,
    format: TileFormat,
) -> FsSink {
    let algo: ChecksumAlgo = args.checksum_algo.clone().into();
    let p = &args.pipeline;
    // @doc-snippet:begin slot=sink-fs imports=FsSink,TileFormat,ChecksumMode,ChecksumAlgo,ManifestBuilder
    let mut sink = FsSink::new(dir, plan.clone())
        // @doc-test: builder_sink_fs.rs::with_format_overrides_default:62
        .with_format(format); // @doc-flag: format kind=param param_name=format
    // @doc-test: builder_sink_fs.rs::with_format_overrides_default:62
    // @doc-flag: quality kind=param param_name=quality
    if args.manifest_emit_checksums || p.checksum || p.manifest_source_hash {
        let mut builder = ManifestBuilder::new().include_source_hash(p.manifest_source_hash);
        if args.manifest_emit_checksums || p.checksum {
            builder = builder.with_checksums(algo);
        }
        // @doc-test: builder_sink_fs.rs::compose_format_checksums_manifest_resume:110
        sink = sink.with_manifest(builder); // @doc-flag: manifest-emit-checksums kind=append
    }
    if args.manifest_emit_checksums {
        // @doc-test: phase3_checksum.rs::emit_only_populates_manifest_checksums:223
        sink = sink.with_checksums(ChecksumMode::EmitOnly, algo); // @doc-flag: checksum-algo kind=param param_name=checksum-algo
    }
    if p.checksum {
        sink = sink.with_checksums(ChecksumMode::Verify, algo);
    }
    if let Some(ds) = crate::build_dedupe_strategy(args) {
        sink = sink.with_dedupe(ds);
    }
    if args.resume {
        sink = sink.with_resume(true);
    }
    // @doc-snippet:end slot=sink-fs
    sink
}

/// The directory a tree run writes into, from an `fs://` URI, the positional
/// output, or the URI itself.
pub(crate) fn tree_dir(args: &PyramidArgs, uri: &str) -> PathBuf {
    match resolve_target(args, uri) {
        Ok(Target::Tree(dir)) => dir,
        _ => unreachable!("only a tree run asks for its directory"),
    }
}

/// Whether a run that stops part way can be picked up with `--resume`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumable {
    Yes,
    No,
}

/// Turn the engine's answer into the run's exit, the same way for every
/// driver: 130 for Ctrl-C, 1 for an error, and 1 for a run that finished but
/// skipped tiles, since its output has holes. On success it prints the summary
/// and hands the result back.
pub(crate) fn conclude(
    result: Result<EngineResult, EngineError>,
    output: &Path,
    start: Instant,
    resumable: Resumable,
    events: &EventPrinter,
) -> EngineResult {
    let result = match result {
        Ok(r) => r,
        Err(EngineError::Cancelled) => {
            eprintln!("Interrupted: stopped at a tile boundary, nothing half-written.");
            match resumable {
                Resumable::Yes => eprintln!(
                    "Hint: run the same command with --resume to finish the tiles that are missing."
                ),
                Resumable::No => {
                    eprintln!("Hint: this output cannot be resumed; run the command again.")
                }
            }
            process::exit(EXIT_INTERRUPTED);
        }
        Err(e @ EngineError::PlanHashMismatch { .. }) => operational_error(&format!(
            "{e}\nHint: --resume needs the input file and the flags the interrupted run \
             used; with a different input or different flags, run without --resume to \
             start over"
        )),
        Err(e) => operational_error(&format!("generating pyramid: {e}")),
    };
    events.summary(&result);
    crate::finish_run(result.clone(), output, start);
    if result.retry_count > 0 || result.skipped_due_to_failure > 0 {
        eprintln!(
            "Retries: {}, tiles skipped after failing every retry: {}",
            result.retry_count, result.skipped_due_to_failure
        );
    }
    if result.skipped_due_to_failure > 0 {
        operational_error(&format!(
            "{} tiles were skipped after failing every retry, so {} has holes",
            result.skipped_due_to_failure,
            output.display()
        ));
    }
    result
}

// ---------------------------------------------------------------------------
// The pipeline driver
// ---------------------------------------------------------------------------

/// Run `viprs pyramid` through the pipeline. `cancel` is the token Ctrl-C
/// trips, installed by the caller before the driver was picked.
pub(crate) fn run(args: PyramidArgs, cancel: CancelToken) {
    let start = Instant::now();

    let target = target_of(&args).unwrap_or_else(|r| r.exit());
    if let Err(refusal) = check_combinations(&args, &target) {
        refusal.exit();
    }
    let to_archive = matches!(target, Target::Archive(_));
    let layout = crate::resolve_layout(&args, to_archive);
    let tile_format = crate::resolve_tile_format(&args, to_archive);
    let object_store = match &target {
        Target::ObjectStore { bucket, prefix } => {
            Some(object_store::prepare(&args, bucket, prefix))
        }
        _ => None,
    };
    if args.memory_budget.is_some() {
        eprintln!(
            "Note: --memory-budget only applies to a fresh run into a tile tree \
             (--storage directory, without --resume or --verify), so this run uses the \
             monolithic engine."
        );
    }

    let Prepared { raster, plan } = prepare(&args, layout);

    // Every tree run from a file hands the run its source digest. The core
    // folds it into the plan hash, so `--resume` refuses a checkpoint made
    // from a different image even when its size matches, and it is the digest
    // the manifest records under `--manifest-source-hash`. A run that started
    // without one could not be resumed by a run with one, so it is not left to
    // that flag. Stdin leaves no file to hash.
    let mut config = engine_config(&args);
    if matches!(target, Target::Tree(_)) && args.input != "-" {
        let digest = hash_source_file(Path::new(&args.input)).unwrap_or_else(|e| {
            operational_error(&format!("hashing {} for the resume check: {e}", args.input))
        });
        config = config.with_source_content_hash(digest);
    }
    let events = EventPrinter::new(args.pipeline.events);

    let run = Run {
        args: &args,
        raster: &raster,
        plan: &plan,
        config,
        cancel,
        events: events.clone(),
    };

    let (result, output) = match target {
        Target::Archive(path) => {
            let sink = archive_sink(&args, &path, &plan, tile_format);
            let result = run.engine(&sink, Some(resume_policy(&args)));
            (result, sink.out_path().to_path_buf())
        }
        Target::Tree(dir) => {
            let sink = tree_sink(&args, &dir, &plan, tile_format);
            let mut policy = resume_policy(&args).with_checkpoint_every(
                args.pipeline
                    .checkpoint_every
                    .unwrap_or(DEFAULT_RESUME_CHECKPOINT_EVERY),
            );
            if let Some(root) = &args.pipeline.checkpoint_root {
                policy = policy.with_checkpoint_root(root);
            }
            (run.engine(&sink, Some(policy)), dir)
        }
        Target::ObjectStore { bucket, prefix } => {
            let store = object_store.expect("prepared above for an s3:// target");
            object_store::run(&run, store, &bucket, &prefix, tile_format)
        }
    };

    let resumable = if to_archive {
        Resumable::No
    } else {
        Resumable::Yes
    };
    conclude(result, &output, start, resumable, &events);
}

/// The PMTiles sink with the writer options the flags ask for.
fn archive_sink(
    args: &PyramidArgs,
    path: &Path,
    plan: &PyramidPlan,
    format: TileFormat,
) -> PmTilesSink {
    let mut options = WriterOptions::default();
    if let Some(layout) = args.pipeline.pmtiles_layout {
        options = options.with_layout(layout.into());
    }
    if let Some(requested) = args.pipeline.dedupe_memory_bytes {
        let window = dedupe_window_bytes(requested, plan);
        if window < requested {
            eprintln!(
                "Note: --dedupe-memory-bytes {requested} lowered to {window}, a slot for every \
                 one of the plan's {} tiles; the writer would allocate the rest for nothing.",
                plan.total_tile_count()
            );
        }
        options = options.with_dedupe_memory_bytes(window);
    }
    // @doc-snippet:begin slot=sink-pmtiles imports=PmTilesSink,TileFormat
    // @doc-test: cli_e2e.rs::pyramid_default_output_is_a_pmtiles_archive:1
    // @doc-flag: storage kind=param param_name=storage
    let sink = PmTilesSink::builder(path)
        .plan(plan.clone())
        .tile_format(format)
        .writer_options(options)
        .ordered_emission(args.pipeline.ordered_emission)
        .build();
    // @doc-snippet:end slot=sink-pmtiles
    sink.unwrap_or_else(|e| operational_error(&format!("creating the PMTiles archive failed: {e}")))
}

/// `--resume`, `--verify` or neither, as the policy the engine takes.
fn resume_policy(args: &PyramidArgs) -> ResumePolicy {
    match crate::resolve_resume_mode(args) {
        ResumeMode::Overwrite => ResumePolicy::overwrite(),
        ResumeMode::Resume => ResumePolicy::resume(),
        ResumeMode::Verify => ResumePolicy::verify(),
    }
}

/// The BLAKE3 of the source file's bytes, the way the core documents
/// `SourceMetadata::bytes_hash` ("the raw source bytes").
pub(crate) fn hash_source_file(path: &Path) -> std::io::Result<String> {
    Ok(ChecksumAlgo::Blake3.hash(&std::fs::read(path)?))
}

/// Everything a run needs once the sink is built.
pub(crate) struct Run<'a> {
    args: &'a PyramidArgs,
    raster: &'a Raster,
    plan: &'a PyramidPlan,
    config: EngineConfig,
    cancel: CancelToken,
    events: Arc<EventPrinter>,
}

impl Run<'_> {
    /// Drive the engine into `sink`, or the region path when `--region` is set.
    fn engine<S: libviprs::TileSink>(
        &self,
        sink: S,
        resume: Option<ResumePolicy>,
    ) -> Result<EngineResult, EngineError> {
        let builder = EngineBuilder::new(self.raster, self.plan.clone(), sink)
            .with_config(self.config.clone())
            .with_cancel(self.cancel.clone());
        if let Some(r) = self.args.pipeline.region {
            return builder.run_region(r.x, r.y, r.width, r.height);
        }
        let mut builder = builder.with_observer_arc(self.events.clone());
        if let Some(policy) = resume {
            builder = builder.with_resume(policy);
        }
        builder.run()
    }
}

/// Cancel the run on the first Ctrl-C; exit at once on the second.
///
/// The first one asks the engine to stop at the next tile boundary, so the
/// tiles on disk are whole and the checkpoint names exactly those, and the run
/// lock is released on the way out. A second one is somebody who does not
/// want to wait for that.
pub(crate) fn install_sigint() -> CancelToken {
    let token = CancelToken::new();
    let handler = token.clone();
    let installed = ctrlc::set_handler(move || {
        if handler.is_cancelled() {
            process::exit(EXIT_INTERRUPTED);
        }
        handler.cancel();
        eprintln!("Interrupt received, stopping at the next tile (Ctrl-C again to quit now)...");
    });
    if let Err(e) = installed {
        eprintln!("Warning: Ctrl-C will not stop cleanly here: {e}");
    }
    token
}

// ---------------------------------------------------------------------------
// --events
// ---------------------------------------------------------------------------

/// Prints engine events on stdout, one line each.
///
/// stdout because nothing else in `viprs pyramid` writes there: the progress
/// and summary lines go to stderr, and so does `--trace-level` output, so a
/// consumer can read events off stdout without filtering. Each line is written
/// and flushed under one lock, so concurrent workers cannot interleave
/// half-lines.
pub(crate) struct EventPrinter {
    format: EventsArg,
    out: Mutex<std::io::Stdout>,
}

impl EventPrinter {
    pub(crate) fn new(format: EventsArg) -> Arc<Self> {
        Arc::new(Self {
            format,
            out: Mutex::new(std::io::stdout()),
        })
    }

    fn emit(&self, name: &str, fields: Vec<(&'static str, serde_json::Value)>) {
        let line = match self.format {
            EventsArg::None => return,
            EventsArg::Json => json_line(name, fields),
            EventsArg::Text => text_line(name, &fields),
        };
        let mut out = self.out.lock().unwrap_or_else(|p| p.into_inner());
        // A closed stdout (a consumer that stopped reading) must not take the
        // run down with it; the events are a view of the run, not the run.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    /// The closing line: what the run did, including the counts no
    /// per-tile event carries.
    fn summary(&self, result: &EngineResult) {
        self.emit(
            "summary",
            vec![
                ("tiles_produced", result.tiles_produced.into()),
                ("tiles_skipped", result.tiles_skipped.into()),
                ("retry_count", result.retry_count.into()),
                (
                    "skipped_due_to_failure",
                    result.skipped_due_to_failure.into(),
                ),
                ("peak_bytes", result.peak_memory_bytes.into()),
            ],
        );
    }
}

/// `{"v":1,"event":name,...fields}`.
fn json_line(name: &str, fields: Vec<(&'static str, serde_json::Value)>) -> String {
    let mut map = serde_json::Map::new();
    map.insert("v".to_string(), EVENTS_SCHEMA_VERSION.into());
    map.insert("event".to_string(), name.into());
    for (k, v) in fields {
        map.insert(k.to_string(), v);
    }
    serde_json::Value::Object(map).to_string()
}

/// `name level/col/row key=value ...` for a tile event, `name key=value ...`
/// otherwise.
fn text_line(name: &str, fields: &[(&'static str, serde_json::Value)]) -> String {
    let get = |k: &str| fields.iter().find(|(f, _)| *f == k).map(|(_, v)| v);
    let mut line = name.to_string();
    let tile = match (get("level"), get("col"), get("row")) {
        (Some(l), Some(c), Some(r)) => {
            line.push_str(&format!(" {l}/{c}/{r}"));
            true
        }
        _ => false,
    };
    for (k, v) in fields {
        if tile && matches!(*k, "level" | "col" | "row") {
            continue;
        }
        match v {
            serde_json::Value::String(s) => line.push_str(&format!(" {k}={s:?}")),
            other => line.push_str(&format!(" {k}={other}")),
        }
    }
    line
}

fn coord_fields(coord: TileCoord) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("level", coord.level.into()),
        ("col", coord.col.into()),
        ("row", coord.row.into()),
    ]
}

/// The fields an event's line carries, beyond its name.
fn event_fields(event: &EngineEvent) -> Vec<(&'static str, serde_json::Value)> {
    match event {
        EngineEvent::TileCompleted { coord, .. }
        | EngineEvent::TileSkippedOnResume { coord, .. } => coord_fields(*coord),
        EngineEvent::TileFailed { coord, error, .. } => {
            let mut f = coord_fields(*coord);
            f.push(("error", error.clone().into()));
            f
        }
        EngineEvent::RetryAttempted { coord, attempt, .. } => {
            let mut f = coord_fields(*coord);
            f.push(("attempt", (*attempt).into()));
            f
        }
        EngineEvent::LevelStarted {
            level,
            width,
            height,
            tile_count,
        } => vec![
            ("level", (*level).into()),
            ("width", (*width).into()),
            ("height", (*height).into()),
            ("tile_count", (*tile_count).into()),
        ],
        EngineEvent::LevelCompleted {
            level,
            tiles_produced,
        } => vec![
            ("level", (*level).into()),
            ("tiles_produced", (*tiles_produced).into()),
        ],
        EngineEvent::MemorySnapshot {
            current_bytes,
            peak_bytes,
            ..
        } => vec![
            ("current_bytes", (*current_bytes).into()),
            ("peak_bytes", (*peak_bytes).into()),
        ],
        EngineEvent::CheckpointFlushed { tiles } => vec![("tiles", (*tiles).into())],
        EngineEvent::Finished {
            total_tiles,
            levels,
        } => vec![
            ("total_tiles", (*total_tiles).into()),
            ("levels", (*levels).into()),
        ],
        _ => Vec::new(),
    }
}

impl EngineObserver for EventPrinter {
    fn on_event(&self, event: EngineEvent) {
        if self.format == EventsArg::None {
            return;
        }
        self.emit(event.name(), event_fields(&event));
    }
}

// ---------------------------------------------------------------------------
// The object-store sink
// ---------------------------------------------------------------------------

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
mod object_store {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use libviprs::{
        DirectoryObjectStore, EngineError, EngineResult, ObjectStoreConfig, ObjectStoreSink,
        SinkError, TileFormat,
    };

    use super::Run;
    use crate::{PyramidArgs, operational_error, usage_error};

    /// Check the flags and build the store, before the input is read.
    ///
    /// The stub store is the core's `DirectoryObjectStore`: object `key` in
    /// `bucket` is the file `root/bucket/key`. `resolve_target` has already
    /// refused a bucket that climbs out of the root; the store's own check is
    /// stricter (a backslash, say, is one plain name to a Unix path), and what
    /// it refuses is the same usage mistake, exit 2.
    pub(super) fn prepare(
        args: &PyramidArgs,
        bucket: &str,
        _prefix: &str,
    ) -> Arc<DirectoryObjectStore> {
        let Some(root) = args.pipeline.object_store_root.clone() else {
            usage_error(
                "an s3:// sink needs somewhere to write: --object-store-root DIR",
                "this build has no network transport, so the object-store sink writes \
                 through a local stub store; name its directory",
            );
        };
        match DirectoryObjectStore::for_bucket(&root, bucket) {
            Ok(store) => Arc::new(store),
            Err(SinkError::Other(why)) => {
                usage_error(&why, "a bucket is one plain name, such as s3://tiles/run-1")
            }
            Err(e) => usage_error(
                &e.to_string(),
                "a bucket is one plain name, such as s3://tiles/run-1",
            ),
        }
    }

    pub(super) fn run(
        run: &Run<'_>,
        store: Arc<DirectoryObjectStore>,
        bucket: &str,
        prefix: &str,
        format: TileFormat,
    ) -> (Result<EngineResult, EngineError>, PathBuf) {
        let image_name = Path::new(&run.args.input)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| s != "-")
            .unwrap_or_else(|| "image".to_string());
        let root = store.root().to_path_buf();
        // @doc-snippet:begin slot=sink-s3 imports=ObjectStoreSink
        // @doc-test: phase3_packfile.rs::tar_sink_produces_valid_archive:177
        // @doc-flag: sink kind=override
        let cfg = ObjectStoreConfig::s3("file://stub", bucket)
            .with_key_prefix(prefix)
            .with_image_name(image_name)
            .with_object_store(store);
        let sink = match ObjectStoreSink::new(cfg, run.plan.clone(), format) {
            Ok(s) => s,
            Err(e) => operational_error(&format!("creating the object-store sink failed: {e}")),
        };
        // @doc-snippet:end slot=sink-s3
        (run.engine(&sink, None), root)
    }
}

#[cfg(not(any(feature = "s3", feature = "object-store-sink")))]
mod object_store {
    use std::path::PathBuf;

    use libviprs::{EngineError, EngineResult, TileFormat};

    use super::Run;
    use crate::{PyramidArgs, operational_error};

    /// Never built: [`prepare`] refuses the run first.
    pub(super) struct NoStore;

    /// A feature this build left out is an operational failure, exit 1, not a
    /// usage mistake (README, "Exit codes").
    pub(super) fn prepare(_args: &PyramidArgs, _bucket: &str, _prefix: &str) -> NoStore {
        operational_error(
            "the s3:// sink needs the `s3` feature, which this viprs was built without; \
             rebuild with `--features s3`, and run `viprs features` to see what this build has",
        );
    }

    pub(super) fn run(
        _run: &Run<'_>,
        _store: NoStore,
        _bucket: &str,
        _prefix: &str,
        _format: TileFormat,
    ) -> (Result<EngineResult, EngineError>, PathBuf) {
        unreachable!("prepare refuses an s3:// target in a build without the feature")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    /// `viprs pyramid in.png out.pmtiles` plus the flags under test. The
    /// output has an extension so the default archive target resolves
    /// without the directory-shaped refusal.
    fn pyramid(extra: &[&str]) -> PyramidArgs {
        let mut argv = vec!["viprs", "in.png", "out.pmtiles"];
        argv.extend_from_slice(extra);
        PyramidArgs::try_parse_from(argv).expect("the flags under test parse")
    }

    /// The json line a pipeline event goes out as, through the printer's
    /// own name and field functions.
    fn json_for(event: &EngineEvent) -> serde_json::Value {
        serde_json::from_str(&json_line(event.name(), event_fields(event))).unwrap()
    }

    /// `PipelineComplete` went out as `unknown`, because the CLI's own name
    /// table predated it. Core names every event now (libviprs#1169), and the
    /// CLI uses those names (libviprs-cli#94).
    #[test]
    fn every_event_goes_out_under_cores_name() {
        let line = json_for(&EngineEvent::PipelineComplete);
        assert_eq!(line["event"], "pipeline_complete", "{line}");
        let line = json_for(&EngineEvent::CheckpointFlushed { tiles: 3 });
        assert_eq!(line["event"], "checkpoint_flushed", "{line}");
        assert_eq!(line["tiles"], 3, "{line}");
    }

    /// `"v"` versions the line format, and the event names are part of it, so
    /// a core rename (a `NAMES_VERSION` bump) has to be a decision here too.
    #[test]
    fn the_events_schema_tracks_cores_names_version() {
        assert_eq!(
            (EngineEvent::NAMES_VERSION, EVENTS_SCHEMA_VERSION),
            (1, 1),
            "core changed its event names; bump EVENTS_SCHEMA_VERSION and say so \
             in the README and CHANGELOG"
        );
    }

    /// The dedupe floor is the writer's own constant, not a copy of it.
    #[test]
    fn the_dedupe_floor_is_the_writers() {
        assert_eq!(
            DEDUPE_MEMORY_FLOOR,
            libviprs::pmtiles::writer::WriterOptions::MIN_DEDUPE_MEMORY_BYTES
        );
    }

    /// The copies the CLI carried until core exported them (libviprs-cli#94).
    #[test]
    fn no_private_copies_of_what_core_exports() {
        // Spelled in pieces so this test does not match itself.
        let sources = [
            ("src/pipeline.rs", include_str!("pipeline.rs")),
            ("src/verify.rs", include_str!("verify.rs")),
        ];
        let copies = [
            ["fn event", "_name("].concat(),
            ["DEDUPE_WINDOW_WAYS", " * 65"].concat(),
            ["push(\".manifest", ".json\")"].concat(),
        ];
        for (file, src) in sources {
            for copy in &copies {
                assert!(
                    !src.contains(copy.as_str()),
                    "{file} still has `{copy}`; use what core exports (libviprs-cli#94)"
                );
            }
        }
    }

    #[test]
    fn retries_alone_retries_then_fails() {
        let args = pyramid(&["--retries", "3", "--retry-backoff-ms", "20"]);
        match args.pipeline.failure_policy() {
            Some(FailurePolicy::RetryThenFail(policy)) => {
                assert_eq!(policy.max_retries, 3);
                assert_eq!(policy.initial_backoff, Duration::from_millis(20));
            }
            other => panic!("--retries on its own must retry then fail, got {other:?}"),
        }
    }

    #[test]
    fn fail_fast_alone_is_fail_fast_and_no_flag_keeps_on_failure() {
        assert!(matches!(
            pyramid(&["--fail-fast"]).pipeline.failure_policy(),
            Some(FailurePolicy::FailFast)
        ));
        assert!(pyramid(&[]).pipeline.failure_policy().is_none());
    }

    #[test]
    fn a_region_is_four_pixel_counts() {
        assert_eq!(
            parse_region("1,2,30,40"),
            Ok(Region {
                x: 1,
                y: 2,
                width: 30,
                height: 40
            })
        );
        assert_eq!(
            parse_region(" 0 , 0 , 5 , 6 "),
            Ok(Region {
                x: 0,
                y: 0,
                width: 5,
                height: 6
            }),
            "spaces around the numbers are fine"
        );
    }

    #[test]
    fn a_region_refuses_the_wrong_count_negatives_and_empty_sides() {
        for bad in [
            "1,2,3",
            "1,2,3,4,5",
            "",
            "a,0,1,1",
            "-1,0,1,1",
            "0,0,0,5",
            "0,0,5,0",
        ] {
            assert!(parse_region(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(
            parse_region("0,0,4294967296,1").is_err(),
            "a side past u32 is refused, not wrapped"
        );
    }

    #[test]
    fn skip_failed_retries_then_skips_and_skips_at_once_without_retries() {
        match pyramid(&["--retries", "2", "--skip-failed"])
            .pipeline
            .failure_policy()
        {
            Some(FailurePolicy::RetryThenSkip(p)) => assert_eq!(p.max_retries, 2),
            other => panic!("got {other:?}"),
        }
        match pyramid(&["--skip-failed"]).pipeline.failure_policy() {
            Some(FailurePolicy::RetryThenSkip(p)) => assert_eq!(p.max_retries, 0),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn skip_failed_conflicts_with_fail_fast_and_on_failure() {
        for extra in [
            &["--skip-failed", "--fail-fast"][..],
            &["--skip-failed", "--on-failure", "fail-fast"][..],
        ] {
            let mut argv = vec!["viprs", "in.png", "out.pmtiles"];
            argv.extend_from_slice(extra);
            assert!(PyramidArgs::try_parse_from(argv).is_err(), "{extra:?}");
        }
    }

    #[test]
    fn a_backoff_above_the_core_cap_raises_the_cap_instead_of_being_clamped() {
        match pyramid(&["--retries", "1", "--retry-backoff-ms", "9000"])
            .pipeline
            .failure_policy()
        {
            Some(FailurePolicy::RetryThenFail(p)) => {
                assert_eq!(p.initial_backoff, Duration::from_secs(9));
                assert!(p.max_backoff >= p.initial_backoff);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn drop_blanks_conflicts_with_both_placeholder_flags() {
        for other in [&["--skip-blank"][..], &["--blank-tolerance", "3"][..]] {
            let mut argv = vec!["viprs", "in.png", "out.pmtiles", "--drop-blanks"];
            argv.extend_from_slice(other);
            assert!(PyramidArgs::try_parse_from(argv).is_err(), "{other:?}");
        }
    }

    fn refusal(extra: &[&str]) -> Option<String> {
        let args = pyramid(extra);
        let target = match target_of(&args) {
            Ok(t) => t,
            Err(r) => return Some(r.message),
        };
        check_combinations(&args, &target).err().map(|r| r.message)
    }

    #[test]
    fn combinations_that_mean_nothing_are_refused_before_anything_is_read() {
        let cases: &[(&[&str], &str)] = &[
            (&["--dedupe-memory-bytes", "519"], "floor"),
            (
                &["--storage", "directory", "--pmtiles-layout", "arrival"],
                "PMTiles archive",
            ),
            (&["--checksum"], "tile tree"),
            (&["--resume"], "tile tree"),
            (&["--object-store-root", "/tmp/x"], "s3:// sink"),
            (
                &["--storage", "directory", "--region", "0,0,4,4", "--resume"],
                "not resumable",
            ),
            (
                &[
                    "--storage",
                    "directory",
                    "--region",
                    "0,0,4,4",
                    "--events",
                    "json",
                ],
                "per-tile events",
            ),
            (&["--sink", "s3://"], "names no bucket"),
            (&["--sink", "s3://../escaped"], "not a bucket name"),
            (&["--sink", "s3://./x"], "not a bucket name"),
        ];
        for (extra, needle) in cases {
            let got = refusal(extra);
            assert!(
                got.as_deref().is_some_and(|m| m.contains(needle)),
                "{extra:?} must be refused mentioning {needle:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn manifest_source_hash_needs_a_file_to_hash() {
        let args = PyramidArgs::try_parse_from([
            "viprs",
            "-",
            "out",
            "--storage",
            "directory",
            "--manifest-source-hash",
        ])
        .unwrap();
        let target = target_of(&args).unwrap();
        let refused = check_combinations(&args, &target).unwrap_err();
        assert!(refused.message.contains("stdin"), "{refused:?}");
    }

    #[test]
    fn combinations_that_mean_something_pass() {
        for extra in [
            &[][..],
            &["--pmtiles-layout", "arrival", "--ordered-emission"][..],
            &["--dedupe-memory-bytes", "520"][..],
            &[
                "--storage",
                "directory",
                "--checksum",
                "--checkpoint-every",
                "5",
            ][..],
            &["--storage", "directory", "--region", "0,0,4,4"][..],
            &[
                "--sink",
                "s3://tiles/run-1",
                "--object-store-root",
                "/tmp/x",
            ][..],
        ] {
            assert_eq!(refusal(extra), None, "{extra:?}");
        }
    }

    #[test]
    fn an_s3_uri_splits_into_bucket_and_prefix() {
        let args = pyramid(&["--sink", "s3://tiles//run-1/"]);
        assert_eq!(
            target_of(&args),
            Ok(Target::ObjectStore {
                bucket: "tiles".into(),
                prefix: "run-1".into()
            })
        );
    }

    fn plan(width: u32, height: u32, tile: u32) -> PyramidPlan {
        PyramidPlanner::new(width, height, tile, 0, Layout::Xyz)
            .expect("a valid plan")
            .plan()
    }

    #[test]
    fn the_dedupe_window_stops_at_a_slot_for_every_planned_tile() {
        let p = plan(200, 100, 64);
        let tiles = p.total_tile_count() as usize;
        let useful = tiles.div_ceil(DEDUPE_WINDOW_WAYS) * DEDUPE_MEMORY_FLOOR;
        assert_eq!(dedupe_window_bytes(usize::MAX, &p), useful);
        assert_eq!(dedupe_window_bytes(64_000_000_000, &p), useful);
        assert_eq!(
            dedupe_window_bytes(DEDUPE_MEMORY_FLOOR, &p),
            DEDUPE_MEMORY_FLOOR,
            "a budget under the ceiling is left alone"
        );
        assert!(useful >= tiles * 65, "every tile has a slot");
    }

    #[test]
    fn the_level_rasters_are_every_level_below_the_top() {
        let p = plan(2048, 2048, 256);
        let bytes = lower_level_bytes(&p, PixelFormat::Rgb8);
        let top = p.levels.iter().max_by_key(|l| l.level).unwrap();
        let all: u64 = p
            .levels
            .iter()
            .map(|l| u64::from(l.width) * u64::from(l.height) * 3)
            .sum();
        assert_eq!(
            bytes,
            all - u64::from(top.width) * u64::from(top.height) * 3
        );
        assert!(bytes > 0);
    }

    #[test]
    fn events_go_out_under_fixed_names() {
        // Core's names, pinned here because they are this CLI's public format.
        let coord = TileCoord::new(3, 1, 2);
        assert_eq!(EngineEvent::tile_completed(coord).name(), "tile_completed");
        assert_eq!(
            EngineEvent::LevelCompleted {
                level: 3,
                tiles_produced: 4
            }
            .name(),
            "level_completed"
        );
        assert_eq!(
            EngineEvent::CheckpointFlushed { tiles: 9 }.name(),
            "checkpoint_flushed"
        );
        assert_eq!(
            EngineEvent::Finished {
                total_tiles: 1,
                levels: 1
            }
            .name(),
            "finished"
        );
    }

    #[test]
    fn a_json_event_line_leads_with_its_version_and_name() {
        let line = json_line("tile_completed", coord_fields(TileCoord::new(3, 1, 2)));
        assert_eq!(
            line,
            r#"{"v":1,"event":"tile_completed","level":3,"col":1,"row":2}"#
        );
    }
}
