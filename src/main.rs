use std::io::Read as _;
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use libviprs::{
    BlankTileStrategy, CollectingObserver, EngineConfig, FsSink, GeoCoord, GeoTransform, Layout,
    PyramidPlanner, Raster, TileFormat, extract_page_image, generate_pyramid_observed,
    pdf::render_page_pdfium,
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
    Pyramid(PyramidArgs),

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

    /// DPI for PDF rasterization (only used for PDF inputs).
    #[arg(long, default_value = "150")]
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

    /// Skip writing tiles where all pixels are identical (blank tile optimization).
    #[arg(long)]
    skip_blank: bool,
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
    #[arg(long, default_value = "150")]
    dpi: u32,

    /// PDF page number (1-based, only used when input is a PDF).
    #[arg(long, default_value = "1")]
    page: usize,
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
}

impl From<LayoutArg> for Layout {
    fn from(arg: LayoutArg) -> Self {
        match arg {
            LayoutArg::DeepZoom => Layout::DeepZoom,
            LayoutArg::Xyz => Layout::Xyz,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum FormatArg {
    Png,
    Jpeg,
    Raw,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Pyramid(args) => run_pyramid(args),
        Command::Info(args) => run_info(args),
        Command::Plan(args) => run_plan(args),
        Command::TestImage(args) => run_test_image(args),
    }
}

fn run_pyramid(args: PyramidArgs) {
    let start = Instant::now();

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
    let layout: Layout = args.layout.into();
    let planner = match PyramidPlanner::new(w, h, args.tile_size, args.overlap, layout) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error creating pyramid plan: {e}");
            process::exit(1);
        }
    };
    let plan = planner.plan();
    eprintln!(
        "Plan: {} levels, {} tiles, tile_size={}, overlap={}",
        plan.level_count(),
        plan.total_tile_count(),
        args.tile_size,
        args.overlap
    );

    // Sink
    let tile_format = match args.format {
        FormatArg::Png => TileFormat::Png,
        FormatArg::Jpeg => TileFormat::Jpeg {
            quality: args.quality,
        },
        FormatArg::Raw => TileFormat::Raw,
    };
    let sink = FsSink::new(&args.output, plan.clone(), tile_format);

    // Engine config
    let config = EngineConfig::default()
        .with_concurrency(args.concurrency)
        .with_buffer_size(args.buffer_size)
        .with_blank_tile_strategy(if args.skip_blank {
            BlankTileStrategy::Placeholder
        } else {
            BlankTileStrategy::Emit
        });

    // Generate
    let observer = CollectingObserver::new();
    let result = match generate_pyramid_observed(&raster, &plan, &sink, &config, &observer) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error generating pyramid: {e}");
            process::exit(1);
        }
    };

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
    eprintln!("Output: {}", args.output.display());
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
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error creating pyramid plan: {e}");
            process::exit(1);
        }
    };
    let plan = planner.plan();

    println!("Image: {}x{}", w, h);
    println!(
        "Tile size: {}, overlap: {}, layout: {:?}",
        args.tile_size, args.overlap, layout
    );
    println!(
        "Levels: {}, total tiles: {}",
        plan.level_count(),
        plan.total_tile_count()
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
            match extract_page_image(&path, args.page) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error extracting image from PDF: {e}");
                    eprintln!(
                        "Hint: use --render for vector PDFs that don't contain embedded images."
                    );
                    process::exit(1);
                }
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
