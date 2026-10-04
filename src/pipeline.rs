//! The pipeline controls on `viprs pyramid` (libviprs-cli#66).
//!
//! The core grew a PMTiles layout choice, a dedupe budget and ordered emission
//! (EPIC M), resume and retry knobs, cancellation, progress events, region runs
//! and `skip_blanks`, and none of it had a flag. This module is where those
//! flags live and where a run that can use them is driven.
//!
//! # Which runs come through here
//!
//! Every run on the monolithic engine into a PMTiles archive, a loose tree or
//! an object store. That is nearly all of them, and it is all of them by
//! default, so Ctrl-C cancels cleanly whether or not a pipeline flag was typed.
//! The two paths that stay in `main.rs` are `--packfile` and `--memory-budget`
//! (the streaming and MapReduce engines), and a pipeline flag given to either
//! of those is a usage error rather than a flag that silently does nothing.
//!
//! # Two flags that look like older ones
//!
//! `--pmtiles-layout` is not `--layout`. `--layout` picks the pyramid scheme
//! (Deep Zoom, XYZ, Google), which decides what a tile *is*; this one picks
//! where the PMTiles writer puts the tile bytes inside the archive, which
//! decides nothing about the tiles at all.
//!
//! `--skip-blanks` is not `--skip-blank`. The older flag writes a one-byte
//! placeholder for every blank tile, so the file count is unchanged; this one
//! drops blank tiles from the output altogether, the way `dzsave --skip-blanks`
//! does.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Args, ValueEnum};
use libviprs::observe::{EngineEvent, EngineObserver};
use libviprs::pmtiles::writer::DEDUPE_WINDOW_WAYS;
use libviprs::pmtiles::{Layout as PmTilesLayout, WriterOptions};
use libviprs::resume::DEFAULT_RESUME_CHECKPOINT_EVERY;
use libviprs::{
    CancelToken, ChecksumAlgo, ChecksumMode, EngineBuilder, EngineConfig, EngineError,
    EngineResult, FailurePolicy, FsSink, ManifestBuilder, PmTilesSink, PyramidPlan, PyramidPlanner,
    Raster, ResumeMode, ResumePolicy, RetryPolicy, TileCoord,
};

use crate::{PyramidArgs, operational_error, usage_error};

/// What one deduplication set costs the PMTiles writer.
///
/// The writer charges 65 bytes per payload it remembers and keeps them in sets
/// of [`DEDUPE_WINDOW_WAYS`]. The 65 is documented on
/// `WriterOptions::dedupe_memory_bytes` but the constant behind it is private,
/// so it is spelled here. Below this the writer quietly rounds up to one set,
/// which is a budget the caller did not ask for, so the flag refuses instead.
pub(crate) const DEDUPE_MEMORY_FLOOR: usize = DEDUPE_WINDOW_WAYS * 65;

/// Exit status for a run stopped by SIGINT, the shell's own convention.
const EXIT_INTERRUPTED: i32 = 130;

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
    /// level's raster is held at once instead of one at a time. Archives only.
    #[arg(long, help_heading = "PMTiles")]
    pub(crate) ordered_emission: bool,

    /// Memory the PMTiles writer may spend recognising duplicate tiles.
    ///
    /// About 65 bytes per distinct tile remembered; the default is 8 MiB.
    /// Two identical tiles further apart than this window are stored twice,
    /// which is still a correct archive, just a larger one. Below 520 bytes
    /// (one dedupe set) the value is refused. Archives only.
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

    /// Retry a failed tile write up to N times.
    ///
    /// A tile still failing after that is skipped and counted, and the run
    /// carries on, unless `--fail-fast` is also given. Shorthand for
    /// `--on-failure retry-skip=N,...`, so the two cannot be combined.
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

    /// Abort the run on a tile that still fails after its retries.
    ///
    /// On its own, abort on the first failure without retrying.
    #[arg(long, conflicts_with = "on_failure", help_heading = "Reliability")]
    pub(crate) fail_fast: bool,

    /// Record a per-tile checksum in the manifest and re-hash every tile on
    /// disk before the run reports success. Tile trees only.
    ///
    /// `viprs verify <tree>` checks a tree against these afterwards.
    #[arg(long, help_heading = "Manifest")]
    pub(crate) checksum: bool,

    /// Record the BLAKE3 of the decoded source pixels in the manifest.
    /// Tile trees only.
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
    /// tile and so keeps one file per tile.
    #[arg(long, help_heading = "Dedupe")]
    pub(crate) skip_blanks: bool,

    /// Print one line per engine event on stdout: `none`, `text` or `json`.
    #[arg(long, value_name = "FORMAT", default_value = "none")]
    pub(crate) events: EventsArg,

    /// Directory the `s3://bucket/prefix` sink writes into (needs the `s3` feature).
    ///
    /// This build carries no network transport, so the object-store sink
    /// writes through a local stub store: object `KEY` in bucket `B` lands at
    /// `DIR/B/KEY`.
    #[arg(long, value_name = "DIR", help_heading = "Output")]
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
        push(self.checksum, "--checksum");
        push(self.manifest_source_hash, "--manifest-source-hash");
        push(self.region.is_some(), "--region");
        push(self.skip_blanks, "--skip-blanks");
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
    fn failure_policy(&self) -> Option<FailurePolicy> {
        match (self.retries, self.fail_fast) {
            (Some(n), fail_fast) => {
                let backoff = Duration::from_millis(self.retry_backoff_ms.unwrap_or(100));
                let policy = RetryPolicy::new(n, backoff);
                Some(if fail_fast {
                    FailurePolicy::RetryThenFail(policy)
                } else {
                    FailurePolicy::RetryThenSkip(policy)
                })
            }
            (None, true) => Some(FailurePolicy::FailFast),
            (None, false) => None,
        }
    }
}

/// Whether this run comes through the pipeline, refusing pipeline flags on a
/// path that would ignore them.
pub(crate) fn takes_over(args: &PyramidArgs) -> bool {
    let legacy = if args.packfile
        || args
            .sink
            .as_deref()
            .is_some_and(|u| u.starts_with("packfile://"))
    {
        Some("--packfile")
    } else if args.memory_budget.is_some() {
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

/// Where the run is going, resolved from the sink URI.
enum Target {
    Archive(PathBuf),
    Tree(PathBuf),
    ObjectStore { bucket: String, prefix: String },
}

fn resolve_target(args: &PyramidArgs, uri: &str) -> Target {
    if let Some(path) = uri.strip_prefix("pmtiles://") {
        Target::Archive(PathBuf::from(path))
    } else if let Some(rest) = uri.strip_prefix("s3://") {
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() {
            usage_error(
                &format!("{uri} names no bucket"),
                "spell it s3://bucket/prefix",
            );
        }
        Target::ObjectStore {
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
        }
    } else if let Some(path) = uri.strip_prefix("fs://") {
        Target::Tree(PathBuf::from(path))
    } else if let Some(output) = args.output.clone() {
        // An unknown scheme has always fallen through to the positional output.
        Target::Tree(output)
    } else {
        Target::Tree(PathBuf::from(uri))
    }
}

/// Every refusal that depends only on the flags, before anything is read.
fn check_combinations(args: &PyramidArgs, target: &Target) {
    let p = &args.pipeline;
    if let Some(bytes) = p.dedupe_memory_bytes
        && bytes < DEDUPE_MEMORY_FLOOR
    {
        usage_error(
            &format!("--dedupe-memory-bytes {bytes} is below the {DEDUPE_MEMORY_FLOOR}-byte floor"),
            &format!(
                "one dedupe set of {DEDUPE_WINDOW_WAYS} payloads at 65 bytes each is the \
                 smallest window the writer has, and it would round {bytes} up to that \
                 without saying so. Ask for {DEDUPE_MEMORY_FLOOR} or more"
            ),
        );
    }
    let is_archive = matches!(target, Target::Archive(_));
    let is_tree = matches!(target, Target::Tree(_));
    if !is_archive && let Some(flag) = p.pmtiles_only() {
        usage_error(
            &format!("{flag} only applies to a PMTiles archive"),
            "it configures the PMTiles writer; drop it, or drop --storage directory / --sink",
        );
    }
    if !is_tree {
        if let Some(flag) = p.tree_only() {
            usage_error(
                &format!("{flag} only applies to a tile tree (--storage directory)"),
                "an archive keeps no manifest and cannot be resumed",
            );
        }
        if args.resume {
            usage_error(
                "--resume only applies to a tile tree (--storage directory)",
                "a PMTiles archive cannot be resumed: the writer's staging is not \
                 reconstructible from a checkpoint, so rerun without --resume",
            );
        }
    }
    if p.object_store_root.is_some() && !matches!(target, Target::ObjectStore { .. }) {
        usage_error(
            "--object-store-root only applies to an s3:// sink",
            "pass --sink s3://bucket/prefix as well, or drop it",
        );
    }
    if p.region.is_some() {
        let resumable = args.resume || p.checkpoint_every.is_some() || p.checkpoint_root.is_some();
        if resumable {
            usage_error(
                "--region runs are not resumable",
                "a region run crops first and keeps no checkpoint; drop --resume and the \
                 checkpoint flags",
            );
        }
        if p.events != EventsArg::None {
            usage_error(
                "--region emits no per-tile events",
                "drop --events, or pre-crop the input and run without --region",
            );
        }
    }
}

/// Run `viprs pyramid` through the pipeline.
pub(crate) fn run(args: PyramidArgs) {
    let start = Instant::now();

    let uri = crate::resolve_sink_uri(&args);
    let target = resolve_target(&args, &uri);
    check_combinations(&args, &target);
    let to_archive = matches!(target, Target::Archive(_));
    let layout = crate::resolve_layout(&args, to_archive);
    let tile_format = crate::resolve_tile_format(&args, to_archive);
    let object_store = match &target {
        Target::ObjectStore { bucket, prefix } => {
            Some(object_store::prepare(&args, bucket, prefix))
        }
        _ => None,
    };

    crate::maybe_init_tracing(&args.trace_level);
    let raster = crate::load_source(&args);
    let (w, h) = (raster.width(), raster.height());
    eprintln!(
        "Source: {}x{} {:?} ({:.1} MB)",
        w,
        h,
        raster.format(),
        raster.data().len() as f64 / (1024.0 * 1024.0)
    );
    if let Some(geo) = crate::build_geo_transform(&args, w, h) {
        let bounds = geo.image_bounds(w, h);
        eprintln!(
            "Geo bounds: ({:.6}, {:.6}) → ({:.6}, {:.6})",
            bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y
        );
    }

    let region = args.pipeline.region;
    let (plan_w, plan_h) = match region {
        Some(r) => {
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
        None => (w, h),
    };

    let planner = match PyramidPlanner::new(plan_w, plan_h, args.tile_size, args.overlap, layout) {
        Ok(p) => p.with_centre(args.centre),
        Err(e) => operational_error(&format!("creating pyramid plan: {e}")),
    };
    let peak_memory = planner.estimate_peak_memory();
    let (canvas_w, canvas_h) = planner.canvas_dimensions();
    eprintln!(
        "Memory estimate: {:.1} MB peak (canvas: {}x{}, source: {}x{})",
        peak_memory as f64 / (1024.0 * 1024.0),
        canvas_w,
        canvas_h,
        plan_w,
        plan_h
    );
    if args.memory_limit > 0 && peak_memory > crate::mb_to_bytes(args.memory_limit) {
        eprintln!(
            "Error: estimated peak memory ({:.1} MB) exceeds --memory-limit ({} MB)",
            peak_memory as f64 / (1024.0 * 1024.0),
            args.memory_limit
        );
        eprintln!("Hint: reduce --dpi or image dimensions to lower memory usage");
        process::exit(1);
    }
    let plan = planner.plan();
    eprintln!(
        "Plan: {} levels, {} tiles, tile_size={}, overlap={}",
        plan.level_count(),
        plan.total_tile_count(),
        args.tile_size,
        args.overlap
    );

    let source_hash = args
        .pipeline
        .manifest_source_hash
        .then(|| ChecksumAlgo::Blake3.hash(raster.data()));
    let config = engine_config(&args);
    let cancel = install_sigint();
    let events = EventPrinter::new(args.pipeline.events);

    let run = Run {
        args: &args,
        raster: &raster,
        plan: &plan,
        config,
        cancel: cancel.clone(),
        events: events.clone(),
    };

    let (result, output) = match target {
        Target::Archive(path) => {
            let mut options = WriterOptions::default();
            if let Some(layout) = args.pipeline.pmtiles_layout {
                options = options.with_layout(layout.into());
            }
            if let Some(bytes) = args.pipeline.dedupe_memory_bytes {
                options = options.with_dedupe_memory_bytes(bytes);
            }
            let sink = match PmTilesSink::builder(&path)
                .plan(plan.clone())
                .tile_format(tile_format)
                .writer_options(options)
                .ordered_emission(args.pipeline.ordered_emission)
                .build()
            {
                Ok(s) => s,
                Err(e) => operational_error(&format!("creating the PMTiles archive failed: {e}")),
            };
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
            let result = run.engine(&sink, Some(policy));
            if let Some(hash) = &source_hash
                && let Err(e) = record_source_hash(&dir, hash)
            {
                operational_error(&format!("recording the source hash in the manifest: {e}"));
            }
            (result, dir)
        }
        Target::ObjectStore { bucket, prefix } => {
            let store = object_store.expect("prepared above for an s3:// target");
            object_store::run(&run, store, &bucket, &prefix, tile_format)
        }
    };

    let result = match result {
        Ok(r) => r,
        Err(EngineError::Cancelled) => {
            eprintln!("Interrupted: stopped at a tile boundary, nothing half-written.");
            if to_archive {
                eprintln!("Hint: a PMTiles archive cannot be resumed; run the command again.");
            } else {
                eprintln!(
                    "Hint: run the same command with --resume to finish the tiles that are missing."
                );
            }
            process::exit(EXIT_INTERRUPTED);
        }
        Err(e @ EngineError::PlanHashMismatch { .. }) => operational_error(&format!(
            "{e}\nHint: --resume needs the flags the interrupted run used; with different \
             ones, run without --resume to start over"
        )),
        Err(e) => operational_error(&format!("generating pyramid: {e}")),
    };
    events.summary(&result);
    crate::finish_run(result.clone(), &output, start);
    if result.retry_count > 0 || result.skipped_due_to_failure > 0 {
        eprintln!(
            "Retries: {}, tiles skipped after failing every retry: {}",
            result.retry_count, result.skipped_due_to_failure
        );
    }
}

/// `--resume`, `--verify` or neither, as the policy the engine takes.
fn resume_policy(args: &PyramidArgs) -> ResumePolicy {
    match crate::resolve_resume_mode(args) {
        ResumeMode::Overwrite => ResumePolicy::overwrite(),
        ResumeMode::Resume => ResumePolicy::resume(),
        ResumeMode::Verify => ResumePolicy::verify(),
    }
}

/// The engine config: the flags `main.rs` already reads, plus this module's.
fn engine_config(args: &PyramidArgs) -> EngineConfig {
    let mut config = EngineConfig::default()
        .with_concurrency(args.concurrency)
        .with_buffer_size(args.buffer_size)
        .with_blank_tile_strategy(crate::build_blank_tile_strategy(args))
        .with_failure_policy(
            args.pipeline
                .failure_policy()
                .unwrap_or_else(|| crate::build_failure_policy(args)),
        )
        .skip_blanks(args.pipeline.skip_blanks);
    if let Some(ds) = crate::build_dedupe_strategy(args) {
        config = config.with_dedupe_strategy(ds);
    }
    config
}

/// The filesystem sink with its manifest and checksum options.
fn tree_sink(
    args: &PyramidArgs,
    dir: &Path,
    plan: &PyramidPlan,
    format: libviprs::TileFormat,
) -> FsSink {
    let algo: ChecksumAlgo = args.checksum_algo.clone().into();
    let p = &args.pipeline;
    let mut sink = FsSink::new(dir, plan.clone()).with_format(format);
    if args.manifest_emit_checksums || p.checksum || p.manifest_source_hash {
        let mut builder = ManifestBuilder::new().include_source_hash(p.manifest_source_hash);
        if args.manifest_emit_checksums || p.checksum {
            builder = builder.with_checksums(algo);
        }
        sink = sink.with_manifest(builder);
    }
    if args.manifest_emit_checksums {
        sink = sink.with_checksums(ChecksumMode::EmitOnly, algo);
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
    sink
}

/// Put the source digest into both copies of the manifest the sink wrote.
///
/// `ManifestBuilder::include_source_hash` is recorded by the core and never
/// read: `FsSink` writes `bytes_hash: null` whatever the builder says. Until
/// that is fixed there, the digest is filled in here, after the sink has
/// finished, in the two places the sink puts the manifest.
fn record_source_hash(dir: &Path, hash: &str) -> std::io::Result<()> {
    let mut paths = vec![dir.join("manifest.json")];
    if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
        let mut sibling = name.to_os_string();
        sibling.push(".manifest.json");
        paths.push(parent.join(sibling));
    }
    for path in paths.into_iter().filter(|p| p.is_file()) {
        let bytes = std::fs::read(&path)?;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(source) = value.get_mut("source").and_then(|s| s.as_object_mut()) {
            source.insert("bytes_hash".to_string(), serde_json::Value::from(hash));
        }
        let text = serde_json::to_vec(&value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
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
fn install_sigint() -> CancelToken {
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
/// and summary lines go to stderr, so a consumer can read events off stdout
/// without filtering. Each line is written and flushed under one lock, so
/// concurrent workers cannot interleave half-lines.
pub(crate) struct EventPrinter {
    format: EventsArg,
    out: Mutex<std::io::Stdout>,
}

impl EventPrinter {
    fn new(format: EventsArg) -> Arc<Self> {
        Arc::new(Self {
            format,
            out: Mutex::new(std::io::stdout()),
        })
    }

    fn emit(&self, name: &str, fields: Vec<(&'static str, serde_json::Value)>) {
        let line = match self.format {
            EventsArg::None => return,
            EventsArg::Json => {
                let mut map = serde_json::Map::new();
                map.insert("event".to_string(), name.into());
                for (k, v) in fields {
                    map.insert(k.to_string(), v);
                }
                serde_json::Value::Object(map).to_string()
            }
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

/// `TileCompleted` becomes `tile_completed`, and so on for a variant this
/// build has no explicit arm for.
fn snake_name(event: &EngineEvent) -> String {
    let debug = format!("{event:?}");
    let variant: String = debug
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut out = String::new();
    for (i, c) in variant.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

impl EngineObserver for EventPrinter {
    fn on_event(&self, event: EngineEvent) {
        if self.format == EventsArg::None {
            return;
        }
        let name = snake_name(&event);
        let fields = match &event {
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
        };
        self.emit(&name, fields);
    }
}

// ---------------------------------------------------------------------------
// The object-store sink
// ---------------------------------------------------------------------------

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
mod object_store {
    use std::path::{Component, Path, PathBuf};
    use std::sync::Arc;

    use libviprs::{
        EngineError, EngineResult, ObjectStore, ObjectStoreConfig, ObjectStoreSink, SinkError,
        TileFormat,
    };

    use super::Run;
    use crate::{PyramidArgs, operational_error, usage_error};

    /// The local stub store: object `key` in `bucket` is the file
    /// `root/bucket/key`, written atomically.
    pub(super) struct LocalStore {
        root: PathBuf,
    }

    impl LocalStore {
        fn path_of(&self, key: &str) -> Result<PathBuf, SinkError> {
            let rel = Path::new(key);
            if key.is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
                return Err(SinkError::Other(format!(
                    "object key {key:?} would escape the stub store"
                )));
            }
            Ok(self.root.join(rel))
        }
    }

    impl ObjectStore for LocalStore {
        fn put(&self, key: &str, bytes: &[u8]) -> Result<(), SinkError> {
            let path = self.path_of(key)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut tmp = path.clone().into_os_string();
            tmp.push(".part");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, &path)?;
            Ok(())
        }
    }

    /// Check the flags and build the store, before the input is read.
    pub(super) fn prepare(args: &PyramidArgs, bucket: &str, _prefix: &str) -> Arc<LocalStore> {
        let Some(root) = args.pipeline.object_store_root.clone() else {
            usage_error(
                "an s3:// sink needs somewhere to write: --object-store-root DIR",
                "this build has no network transport, so the object-store sink writes \
                 through a local stub store; name its directory",
            );
        };
        Arc::new(LocalStore {
            root: root.join(bucket),
        })
    }

    pub(super) fn run(
        run: &Run<'_>,
        store: Arc<LocalStore>,
        bucket: &str,
        prefix: &str,
        format: TileFormat,
    ) -> (Result<EngineResult, EngineError>, PathBuf) {
        let image_name = Path::new(&run.args.input)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| s != "-")
            .unwrap_or_else(|| "image".to_string());
        let root = store.root.clone();
        let cfg = ObjectStoreConfig::s3("file://stub", bucket)
            .with_key_prefix(prefix)
            .with_image_name(image_name)
            .with_object_store(store);
        let sink = match ObjectStoreSink::new(cfg, run.plan.clone(), format) {
            Ok(s) => s,
            Err(e) => operational_error(&format!("creating the object-store sink failed: {e}")),
        };
        (run.engine(&sink, None), root)
    }
}

#[cfg(not(any(feature = "s3", feature = "object-store-sink")))]
mod object_store {
    use std::path::PathBuf;

    use libviprs::{EngineError, EngineResult, TileFormat};

    use super::Run;
    use crate::{PyramidArgs, usage_error};

    /// Never built: [`prepare`] refuses the run first.
    pub(super) struct LocalStore;

    pub(super) fn prepare(_args: &PyramidArgs, _bucket: &str, _prefix: &str) -> LocalStore {
        usage_error(
            "the s3:// sink needs the `s3` feature, which this viprs was built without",
            "rebuild with `--features s3`; `viprs features` lists what this build has",
        );
    }

    pub(super) fn run(
        _run: &Run<'_>,
        _store: LocalStore,
        _bucket: &str,
        _prefix: &str,
        _format: TileFormat,
    ) -> (Result<EngineResult, EngineError>, PathBuf) {
        unreachable!("prepare refuses an s3:// target in a build without the feature")
    }
}
