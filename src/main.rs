use std::path::PathBuf;
use std::process;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use libviprs::{
    extract_page_image, generate_pyramid_observed, pdf::render_page_pdfium, CollectingObserver,
    EngineConfig, FsSink, GeoCoord, GeoTransform, Layout, PyramidPlanner, Raster,
    TileFormat,
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

    /// Show info about a PDF file.
    Info(InfoArgs),
}

#[derive(Parser)]
struct PyramidArgs {
    /// Input file (PDF, PNG, JPEG, or TIFF).
    input: PathBuf,

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
}

#[derive(Parser)]
struct InfoArgs {
    /// PDF file to inspect.
    input: PathBuf,
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
    let config = EngineConfig::default().with_concurrency(args.concurrency);

    // Generate
    let observer = CollectingObserver::new();
    let result =
        match generate_pyramid_observed(&raster, &plan, &sink, &config, &observer) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error generating pyramid: {e}");
                process::exit(1);
            }
        };

    let elapsed = start.elapsed();
    eprintln!(
        "Done: {} tiles, {} levels, peak memory {:.1} MB, {:.2}s",
        result.tiles_produced,
        result.levels_processed,
        result.peak_memory_bytes as f64 / (1024.0 * 1024.0),
        elapsed.as_secs_f64()
    );
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
                        if page.has_images {
                            " (has images)"
                        } else {
                            ""
                        }
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

fn load_source(args: &PyramidArgs) -> Raster {
    let path = &args.input;

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
            eprintln!("Rendering PDF page {} at {} DPI (pdfium)...", args.page, args.dpi);
            match render_page_pdfium(path, args.page, args.dpi) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error rendering PDF with pdfium: {e}");
                    eprintln!("Hint: ensure libpdfium is installed. Run without --render to extract embedded images instead.");
                    process::exit(1);
                }
            }
        } else {
            // Extract embedded raster image (scanned PDFs)
            eprintln!("Extracting image from PDF page {}...", args.page);
            match extract_page_image(path, args.page) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error extracting image from PDF: {e}");
                    eprintln!("Hint: use --render for vector PDFs that don't contain embedded images.");
                    process::exit(1);
                }
            }
        }
    } else {
        eprintln!("Decoding {}...", path.display());
        match libviprs::decode_file(path) {
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
