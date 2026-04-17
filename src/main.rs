use std::io::Read as _;
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use libviprs::{
    BlankTileStrategy, ChecksumAlgo, ChecksumMode, CollectingObserver, DedupeStrategy,
    EngineConfig, FailurePolicy, FsSink, GeoCoord, GeoTransform, Layout, ManifestBuilder,
    MapReduceConfig, PyramidPlanner, Raster, ResumeMode, RetryPolicy, StreamingConfig, TileFormat,
    compute_inflight_strips, estimate_mapreduce_peak_memory, extract_page_image,
    generate_pyramid_auto, generate_pyramid_mapreduce_auto, generate_pyramid_observed,
    generate_pyramid_resumable, pdf::render_page_pdfium,
};

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
struct PyramidArgs {
    /// Input file (PDF, PNG, JPEG, or TIFF). Use "-" for stdin.
    input: String,

    /// Output directory for tiles.
    output: PathBuf,

    /// Tile size in pixels.
    #[arg(long, default_value = "256")]
    tile_size: u32,

    /// Tile overlap in pixels.
    #[arg(long, default_value = "0")]
    overlap: u32,

    /// Tile layout format.
    #[arg(long, default_value = "deep-zoom")]
    layout: LayoutArg,

    /// Tile image format.
    #[arg(long, default_value = "png")]
    format: FormatArg,

    /// JPEG quality (1-100, only used with --format jpeg).
    #[arg(long, default_value = "85")]
    quality: u8,

    /// DPI for PDF rendering/page-size scaling (default matches libvips).
    #[arg(long, default_value = "72")]
    dpi: u32,

    /// PDF page number to extract (1-based, only used for PDF inputs).
    #[arg(long, default_value = "1")]
    page: usize,

    /// Number of worker threads (0 = single-threaded).
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
    #[arg(long)]
    skip_blank: bool,

    /// Centre the image within the tile grid (even padding on all sides).
    #[arg(long)]
    centre: bool,

    /// Memory limit in MB for the raster pipeline. If the estimated peak
    /// memory exceeds this limit, the command exits with an error before
    /// rendering. Use 0 to disable the check (default).
    #[arg(long, default_value = "0")]
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
    #[arg(long, value_name = "MB")]
    memory_budget: Option<u64>,

    /// Use the parallel MapReduce engine for strip processing.
    ///
    /// When combined with --memory-budget, renders multiple strips concurrently
    /// (bounded by the budget) for higher throughput on multi-core systems.
    /// The --concurrency flag controls per-strip tile worker threads.
    #[arg(long)]
    parallel: bool,

    // -------------------------------------------------------------------------
    // Phase 3 hardening flags
    // -------------------------------------------------------------------------
    /// Sink URI: fs://path, s3://bucket/prefix, or packfile://path.tar[.gz]/.zip.
    /// Defaults to the positional output directory as a filesystem sink.
    #[arg(long, value_name = "URI")]
    sink: Option<String>,

    /// Resume from checkpoint if present (mutually exclusive with --overwrite and --verify).
    #[arg(long, conflicts_with_all = ["overwrite", "verify"])]
    resume: bool,

    /// Overwrite existing output (default behaviour when no resume/verify flag is set).
    #[arg(long, conflicts_with_all = ["resume", "verify"])]
    overwrite: bool,

    /// Verify existing output against checksums rather than regenerate.
    #[arg(long, conflicts_with_all = ["resume", "overwrite"])]
    verify: bool,

    /// Manifest schema version to emit (only 1 is supported today).
    #[arg(long, default_value = "1", value_name = "N")]
    manifest_version: u32,

    /// Emit per-tile checksums into the manifest.
    #[arg(long)]
    manifest_emit_checksums: bool,

    /// Hash algorithm used for per-tile checksums (blake3 or sha256).
    #[arg(long, default_value = "blake3", value_name = "ALGO")]
    checksum_algo: ChecksumAlgoArg,

    /// If set, treat tiles within this channel delta of blank as blank
    /// (enables PlaceholderWithTolerance blank tile strategy).
    #[arg(long, value_name = "DELTA")]
    blank_tolerance: Option<u8>,

    /// Maximum retries per tile on sink failure.
    #[arg(long, default_value = "3", value_name = "N")]
    retry_max: u32,

    /// Initial backoff in milliseconds before the first retry.
    #[arg(long, default_value = "50", value_name = "MS")]
    retry_backoff: u64,

    /// How to react when sink writes fail after retries.
    #[arg(long, default_value = "fail-fast", value_name = "POL")]
    failure_policy: FailurePolicyArg,

    /// If set, initialise tracing-subscriber at this log level (requires tracing feature).
    #[arg(long, value_name = "LVL")]
    trace_level: Option<String>,

    /// Stub: accept an OpenTelemetry / metrics scrape endpoint URL; warns if unused in this build.
    #[arg(long, value_name = "URL")]
    metrics_endpoint: Option<String>,

    /// Shorthand for --sink packfile://<output>.tar (requires packfile feature).
    #[arg(long)]
    packfile: bool,

    /// Deduplicate blank (uniform-colour) tiles only (DedupeStrategy::Blanks).
    #[arg(long, conflicts_with = "dedupe_all")]
    dedupe_blanks: bool,

    /// Deduplicate all tiles by content hash, using --checksum-algo (mutually exclusive with --dedupe-blanks).
    #[arg(long, conflicts_with = "dedupe_blanks")]
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

/// CLI representation of the failure policy (maps to [`FailurePolicy`]).
#[derive(Clone, ValueEnum)]
enum FailurePolicyArg {
    /// Abort immediately on the first sink error; no retries.
    FailFast,
    /// Retry up to --retry-max times, then abort if all retries fail.
    RetryThenFail,
    /// Retry up to --retry-max times, then skip the tile and continue.
    RetryThenSkip,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Pyramid(args) => run_pyramid(*args),
        Command::Info(args) => run_info(args),
        Command::Plan(args) => run_plan(args),
        Command::TestImage(args) => run_test_image(args),
    }
}

/// Resolve the effective sink URI from flags.
///
/// Priority:
/// 1. `--packfile` shorthand  → `packfile://<output>.tar`
/// 2. `--sink <URI>`          → as-is
/// 3. (none)                  → `fs://<output>`
fn resolve_sink_uri(args: &PyramidArgs) -> String {
    if args.packfile {
        return format!("packfile://{}.tar", args.output.display());
    }
    if let Some(ref uri) = args.sink {
        return uri.clone();
    }
    format!("fs://{}", args.output.display())
}

/// Determine the [`ResumeMode`] from the three mutually-exclusive flags.
fn resolve_resume_mode(args: &PyramidArgs) -> Option<ResumeMode> {
    if args.resume {
        return Some(ResumeMode::Resume);
    }
    if args.verify {
        return Some(ResumeMode::Verify);
    }
    if args.overwrite {
        return Some(ResumeMode::Overwrite);
    }
    None
}

/// Build the [`FailurePolicy`] from the CLI flags.
fn build_failure_policy(args: &PyramidArgs) -> FailurePolicy {
    let retry_policy = RetryPolicy {
        max_retries: args.retry_max,
        initial_backoff: std::time::Duration::from_millis(args.retry_backoff),
        multiplier: 2.0,
        max_backoff: std::time::Duration::from_secs(5),
        jitter: true,
    };
    match args.failure_policy {
        FailurePolicyArg::FailFast => FailurePolicy::FailFast,
        FailurePolicyArg::RetryThenFail => FailurePolicy::RetryThenFail(retry_policy),
        FailurePolicyArg::RetryThenSkip => FailurePolicy::RetryThenSkip(retry_policy),
    }
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
/// This function is compiled unconditionally but the actual call to
/// `tracing_subscriber` is gated on the `tracing` feature.
fn maybe_init_tracing(level: &Option<String>) {
    let Some(_level) = level else { return };

    #[cfg(feature = "tracing")]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(_level))
            .init();
    }
    #[cfg(not(feature = "tracing"))]
    {
        eprintln!(
            "Warning: --trace-level ignored — rebuild with `--features tracing` to enable tracing."
        );
    }
}

fn run_pyramid(args: PyramidArgs) {
    let start = Instant::now();

    // Initialise tracing if requested (no-op when feature is off).
    maybe_init_tracing(&args.trace_level);

    // Warn about --metrics-endpoint: this is a stub in all build configurations.
    if let Some(ref url) = args.metrics_endpoint {
        eprintln!(
            "Warning: --metrics-endpoint {url} accepted but metrics push is not implemented in this build."
        );
    }

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
    let planner = match PyramidPlanner::new(w, h, args.tile_size, args.overlap, layout) {
        Ok(p) => p.with_centre(args.centre),
        Err(e) => {
            eprintln!("Error creating pyramid plan: {e}");
            process::exit(1);
        }
    };

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

    if args.memory_limit > 0 {
        let limit_bytes = args.memory_limit * 1024 * 1024;
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
    let mut engine_config = EngineConfig::default()
        .with_concurrency(args.concurrency)
        .with_buffer_size(args.buffer_size)
        .with_blank_tile_strategy(blank_strategy)
        .with_failure_policy(failure_policy);

    if let Some(ds) = dedupe_strategy {
        engine_config = engine_config.with_dedupe_strategy(ds);
    }

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
        let mut sink = FsSink::new(&base_dir, plan.clone(), tile_format);
        if let Some(mb) = manifest_builder {
            sink = sink.with_manifest(mb);
        }
        if args.manifest_emit_checksums {
            sink = sink.with_checksums(ChecksumMode::EmitOnly, checksum_algo);
        }
        if let Some(ds) = build_dedupe_strategy(&args) {
            sink = sink.with_dedupe(ds);
        }
        if args.resume {
            sink = sink.with_resume(true);
        }

        let result = if let Some(mode) = resume_mode {
            // Use the resumable entry point
            match generate_pyramid_resumable(&raster, &plan, &sink, &engine_config, mode) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error generating pyramid: {e}");
                    process::exit(1);
                }
            }
        } else {
            // Original monolithic / streaming path
            run_generate(&args, &raster, &plan, &sink, engine_config, start)
        };

        finish_run(result, &base_dir, start);
    }
}

/// Entry point for the monolithic / streaming / mapreduce generation paths
/// (filesystem sink only).  Returns the [`libviprs::EngineResult`] for
/// summary printing.
fn run_generate(
    args: &PyramidArgs,
    raster: &Raster,
    plan: &libviprs::PyramidPlan,
    sink: &FsSink,
    engine_config: EngineConfig,
    _start: Instant,
) -> libviprs::EngineResult {
    let observer = CollectingObserver::new();

    match args.memory_budget {
        None => {
            // No budget specified — use the original monolithic engine
            match generate_pyramid_observed(raster, plan, sink, &engine_config, &observer) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error generating pyramid: {e}");
                    process::exit(1);
                }
            }
        }
        Some(budget_mb) => {
            let budget_bytes = if budget_mb == 0 {
                let mono_est = plan.estimate_peak_memory_for_format(raster.format());
                mono_est / 4
            } else {
                budget_mb * 1024 * 1024
            };

            let mono_est = plan.estimate_peak_memory_for_format(raster.format());

            if args.parallel {
                let mr_config = MapReduceConfig {
                    memory_budget_bytes: budget_bytes,
                    tile_concurrency: args.concurrency,
                    buffer_size: args.buffer_size,
                    background_rgb: [255, 255, 255],
                    blank_tile_strategy: engine_config.blank_tile_strategy,
                };

                if mono_est <= budget_bytes {
                    eprintln!(
                        "MapReduce: budget {:.1} MB >= monolithic peak {:.1} MB, using monolithic engine",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        mono_est as f64 / (1024.0 * 1024.0),
                    );
                } else {
                    let strip_h =
                        libviprs::compute_strip_height(plan, raster.format(), budget_bytes);
                    let sh = strip_h.unwrap_or(2 * args.tile_size);
                    let inflight = compute_inflight_strips(plan, raster.format(), sh, budget_bytes);
                    let est = estimate_mapreduce_peak_memory(plan, raster.format(), sh, inflight);
                    eprintln!(
                        "MapReduce: budget {:.1} MB, strip_height={}, {} in-flight strips, estimated peak {:.1} MB",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        strip_h.map_or("min".to_string(), |h| format!("{h}")),
                        inflight,
                        est as f64 / (1024.0 * 1024.0),
                    );
                }

                match generate_pyramid_mapreduce_auto(raster, plan, sink, &mr_config, &observer) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("Error generating pyramid: {e}");
                        process::exit(1);
                    }
                }
            } else {
                let streaming_config = StreamingConfig {
                    memory_budget_bytes: budget_bytes,
                    engine: engine_config,
                };

                if mono_est <= budget_bytes {
                    eprintln!(
                        "Streaming: budget {:.1} MB >= monolithic peak {:.1} MB, using monolithic engine",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        mono_est as f64 / (1024.0 * 1024.0),
                    );
                } else {
                    let strip_h =
                        libviprs::compute_strip_height(plan, raster.format(), budget_bytes);
                    let est = strip_h
                        .map(|sh| libviprs::estimate_streaming_memory(plan, raster.format(), sh));
                    eprintln!(
                        "Streaming: budget {:.1} MB, strip_height={}, estimated peak {:.1} MB",
                        budget_bytes as f64 / (1024.0 * 1024.0),
                        strip_h.map_or("min".to_string(), |h| format!("{h}")),
                        est.unwrap_or(0) as f64 / (1024.0 * 1024.0),
                    );
                }

                match generate_pyramid_auto(raster, plan, sink, &streaming_config, &observer) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("Error generating pyramid: {e}");
                        process::exit(1);
                    }
                }
            }
        }
    }
}

/// Print the post-run summary line.
fn finish_run(result: libviprs::EngineResult, output: &std::path::Path, start: Instant) {
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
    _resume_mode: Option<ResumeMode>,
    _start: Instant,
) {
    #[cfg(feature = "s3")]
    {
        // TODO Phase 3: parse bucket/prefix from _rest, build ObjectStoreConfig,
        // construct ObjectStoreSink, run generate_pyramid_resumable or
        // generate_pyramid_observed as appropriate.
        eprintln!("Error: s3:// sink is not yet fully wired (Phase 3 TODO).");
        process::exit(2);
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
    _resume_mode: Option<ResumeMode>,
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

        let sink = match PackfileSink::new(_path, fmt, _plan.clone(), _tile_format) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error creating packfile sink: {e}");
                process::exit(1);
            }
        };

        let observer = CollectingObserver::new();
        let result = if let Some(mode) = _resume_mode {
            match generate_pyramid_resumable(_raster, _plan, &sink, &_engine_config, mode) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error generating pyramid: {e}");
                    process::exit(1);
                }
            }
        } else {
            match generate_pyramid_observed(_raster, _plan, &sink, &_engine_config, &observer) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error generating pyramid: {e}");
                    process::exit(1);
                }
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

    if ext == "pdf" {
        if args.render {
            // Use PDFium to render the page (vector PDFs)
            eprintln!(
                "Rendering PDF page {} at {} DPI (pdfium)...",
                args.page, args.dpi
            );
            match render_page_pdfium(&path, args.page, args.dpi) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error rendering PDF with pdfium: {e}");
                    eprintln!(
                        "Hint: ensure libpdfium is installed. Run without --render to extract embedded images instead."
                    );
                    process::exit(1);
                }
            }
        } else {
            // Extract embedded raster image (scanned PDFs)
            eprintln!("Extracting image from PDF page {}...", args.page);
            let raster = match extract_page_image(&path, args.page) {
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
            if args.match_page_size {
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
}

fn build_geo_transform(args: &PyramidArgs, _w: u32, _h: u32) -> Option<GeoTransform> {
    let origin_str = args.geo_origin.as_ref()?;
    let scale_str = args.geo_scale.as_ref()?;

    let origin = parse_coord_pair(origin_str, "geo-origin");
    let scale = parse_coord_pair(scale_str, "geo-scale");

    Some(GeoTransform::from_origin_and_scale(
        GeoCoord::new(origin.0, origin.1),
        scale.0,
        scale.1,
    ))
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
