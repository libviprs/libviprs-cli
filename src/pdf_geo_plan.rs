//! PDF options past the defaults, geo transforms, and the planner queries
//! (libviprs-cli#68).
//!
//! Three small surfaces that sit next to the pyramid command rather than in
//! the op families, so they live here and `main.rs` only names them:
//!
//! * `viprs pdf info | rotation | extract`: a PDF's metadata, a page's
//!   `/Rotate`, and one page as an image with a password, a background fill, a
//!   DPI, or a render budget;
//! * `viprs geo pixel-to-geo | geo-to-pixel | tile-center`: the library's
//!   [`GeoTransform`], from the same `--geo-origin` / `--geo-scale` pair
//!   `viprs pyramid` takes, or from the six affine coefficients;
//! * the query flags on `viprs plan` (`--estimate-memory`, `--dzi-manifest`,
//!   `--properties-sidecar`, `--tile-path`, `--tile-rect`), each printing what
//!   the planner returns and nothing else.
//!
//! Everything here is a door onto a library call. The command prints what the
//! call returned, so the e2e cells can compare it with the call itself.
//!
//! # Exit codes
//!
//! Usage mistakes (flags that cannot combine, a value that does not parse) are
//! exit 2, which clap does itself for the declarative ones. A run that was well
//! formed and could not do what was asked (a locked PDF, a page that is not
//! there, a layout with no manifest) is exit 1.
//!
//! # Passwords
//!
//! `libviprs::pdf_info_with_password` and `extract_page_image_with_password`
//! open an encrypted file through pdfium. They answer an empty password with
//! `PdfError::PasswordRequired` and a wrong one with `PdfError::WrongPassword`,
//! and this module turns those two into the messages a person sees. Without a
//! `--password` the same calls are made with an empty one, which is how a
//! document that is encrypted gets reported as needing a password at all. The
//! error comes back folded into an `io::Error` on the extract path, so
//! [`pdf_error_in`] looks for the `PdfError` inside the chain rather than
//! matching on text. A build without the `pdfium` feature cannot decrypt, so
//! there the library's own "not available in this build" is what gets printed.

use std::path::{Path, PathBuf};

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use libviprs::pdf::PdfError;
use libviprs::{
    GeoCoord, GeoTransform, Layout, PixelCoord, PixelFormat, PyramidPlan,
    extract_page_image_with_background, extract_page_image_with_password, pdf_info_with_password,
    planner::TileCoord,
};

use crate::{operational_error, ops, usage_error};

// ===========================================================================
// viprs pdf
// ===========================================================================

/// `viprs pdf`: inspect a PDF and pull one page out as an image.
#[derive(Parser)]
pub struct PdfArgs {
    #[command(subcommand)]
    command: PdfCommand,
}

#[derive(Subcommand)]
enum PdfCommand {
    /// Page count and per-page size, optionally for a password-protected file.
    Info(PdfInfoArgs),

    /// A page's `/Rotate` in degrees (0, 90, 180 or 270).
    Rotation(PdfRotationArgs),

    /// Write one page as an image.
    ///
    /// With no render option this extracts the page's largest embedded image at
    /// its stored size, which is the fast path for scans. `--dpi`,
    /// `--background` and `--render-budget` render the page through pdfium
    /// instead, which needs a binary built with the default `pdfium` feature
    /// and a libpdfium on the machine.
    Extract(PdfExtractArgs),
}

#[derive(Parser)]
struct PdfInfoArgs {
    /// The PDF to describe.
    input: PathBuf,

    /// Password for an encrypted PDF.
    #[arg(long)]
    password: Option<String>,
}

#[derive(Parser)]
struct PdfRotationArgs {
    /// The PDF to read.
    input: PathBuf,

    /// Page number (1-based).
    #[arg(long, default_value = "1")]
    page: usize,
}

#[derive(Parser)]
struct PdfExtractArgs {
    /// The PDF to read.
    input: PathBuf,

    /// Where the image goes (`.png`, `.tif`, `.ppm`).
    output: PathBuf,

    /// Page number (1-based).
    #[arg(long, default_value = "1")]
    page: usize,

    /// Password for an encrypted PDF. Cannot be combined with a render option,
    /// because the render path takes no password.
    #[arg(
        long,
        conflicts_with_all = ["dpi", "background", "render_budget"]
    )]
    password: Option<String>,

    /// Render the page through pdfium at this DPI (one PDF point is one pixel
    /// at 72).
    #[arg(long, value_name = "DPI", value_parser = clap::value_parser!(u32).range(1..))]
    dpi: Option<u32>,

    /// Render over a solid fill, as `r,g,b` or `r,g,b,a` with each channel
    /// 0..=255, instead of the default white. Renders at 72 DPI, so it cannot
    /// be combined with `--dpi`.
    #[arg(long, value_name = "R,G,B[,A]", conflicts_with_all = ["dpi", "render_budget"])]
    background: Option<String>,

    /// Pixel ceiling (width times height) for a render. If `--dpi` would go over
    /// it, the DPI is lowered until the page fits, and the DPI used is reported.
    #[arg(long, value_name = "PIXELS", requires = "dpi")]
    render_budget: Option<u64>,
}

pub fn run_pdf(args: PdfArgs) {
    match args.command {
        PdfCommand::Info(a) => run_pdf_info(a),
        PdfCommand::Rotation(a) => run_pdf_rotation(a),
        PdfCommand::Extract(a) => run_pdf_extract(a),
    }
}

/// Find a [`PdfError`] in `err`, either as the error itself or anywhere down
/// its source chain, including inside the `io::Error` that core wraps it in.
///
/// `io::Error::source` skips the error it wraps, so each link is also asked
/// through `get_ref` for the wrapped value.
fn pdf_error_in<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a PdfError> {
    let mut current = Some(err);
    while let Some(e) = current {
        if let Some(pdf) = e.downcast_ref::<PdfError>() {
            return Some(pdf);
        }
        if let Some(inner) = e
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            && let Some(pdf) = pdf_error_in(inner)
        {
            return Some(pdf);
        }
        current = e.source();
    }
    None
}

/// The message for a missing or a wrong password, if `err` is one of those.
fn password_failure_message(
    path: &Path,
    err: &(dyn std::error::Error + 'static),
) -> Option<String> {
    match pdf_error_in(err) {
        Some(PdfError::PasswordRequired) => Some(format!(
            "{} is encrypted and a password is needed (pass --password)",
            path.display()
        )),
        Some(PdfError::WrongPassword) => Some(format!(
            "wrong password for {} (the --password given does not open it)",
            path.display()
        )),
        _ => None,
    }
}

/// Exit 1 for a failed PDF call. A missing password and a wrong one get their
/// own messages; anything else keeps `context` and the library's wording.
fn pdf_failure(context: &str, path: &Path, err: &(dyn std::error::Error + 'static)) -> ! {
    match password_failure_message(path, err) {
        Some(message) => operational_error(&message),
        None => operational_error(&format!("{context}: {err}")),
    }
}

/// Where the password comes from. Not wired up yet: only `flag` is read.
#[allow(dead_code)]
fn resolve_password(
    flag: Option<&str>,
    _file: Option<&Path>,
    _env: Option<std::ffi::OsString>,
    _stdin: &mut dyn std::io::Read,
) -> Result<Option<String>, String> {
    Ok(flag.map(str::to_owned))
}

fn require_file(path: &Path) {
    if !path.exists() {
        operational_error(&format!("file not found: {}", path.display()));
    }
}

fn run_pdf_info(args: PdfInfoArgs) {
    require_file(&args.input);
    // An empty password is how the library is asked "is this one locked?".
    let info = pdf_info_with_password(&args.input, args.password.as_deref().unwrap_or(""));
    match info {
        Ok(info) => {
            println!("PDF: {}", args.input.display());
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
        Err(e) => pdf_failure(
            &format!("reading {}", args.input.display()),
            &args.input,
            &e,
        ),
    }
}

fn run_pdf_rotation(args: PdfRotationArgs) {
    require_file(&args.input);
    match libviprs::pdf::page_rotate(&args.input, args.page) {
        Ok(rotation) => println!("{}", rotation.as_degrees()),
        Err(e) => operational_error(&format!("reading {}: {e}", args.input.display())),
    }
}

/// Parse `--background` into the channel slice the library takes.
fn parse_background(text: &str) -> Vec<f64> {
    let channels: Vec<f64> = text
        .split(',')
        .map(|c| {
            c.trim().parse::<f64>().unwrap_or_else(|_| {
                usage_error(
                    &format!("--background channel {c:?} is not a number"),
                    "write it as r,g,b or r,g,b,a, for example 255,0,0",
                )
            })
        })
        .collect();
    if !(3..=4).contains(&channels.len()) {
        usage_error(
            &format!(
                "--background needs 3 (r,g,b) or 4 (r,g,b,a) channels, got {}",
                channels.len()
            ),
            "write it as r,g,b or r,g,b,a, for example 255,0,0",
        );
    }
    channels
}

fn run_pdf_extract(args: PdfExtractArgs) {
    require_file(&args.input);

    // Parsed before anything is rendered, so a bad value writes nothing.
    let background = args.background.as_deref().map(parse_background);
    let page_u32 = u32::try_from(args.page)
        .unwrap_or_else(|_| usage_error("--page is too large", "pages are numbered from 1"));
    if args.page == 0 {
        usage_error("--page 0 does not exist", "pages are numbered from 1");
    }

    let raster = if let Some(bg) = background {
        extract_page_image_with_background(&args.input, page_u32, &bg)
            .unwrap_or_else(|e| pdf_failure("rendering with a background", &args.input, &e))
    } else if let Some(dpi) = args.dpi {
        render_at_dpi(&args.input, args.page, dpi, args.render_budget)
    } else {
        // With no --password this asks with an empty one, so an encrypted file
        // is reported as needing a password instead of failing on its streams.
        let pw = args.password.as_deref().unwrap_or("");
        extract_page_image_with_password(&args.input, page_u32, pw).unwrap_or_else(|e| {
            pdf_failure(&format!("extracting page {}", args.page), &args.input, &e)
        })
    };

    if let Err(e) = ops::io::save(&raster, &args.output) {
        operational_error(&format!("{e:#}"));
    }
    eprintln!(
        "Wrote {}x{} {:?} to {}",
        raster.width(),
        raster.height(),
        raster.format(),
        args.output.display()
    );
}

#[cfg(feature = "pdfium")]
fn render_at_dpi(path: &Path, page: usize, dpi: u32, budget: Option<u64>) -> libviprs::Raster {
    match budget {
        Some(max_pixels) => {
            match libviprs::pdf::render_page_pdfium_budgeted(path, page, dpi, max_pixels) {
                Ok(done) => {
                    eprintln!(
                        "Rendered at {} DPI ({} the {max_pixels} pixel render budget, asked for {dpi})",
                        done.dpi_used,
                        if done.capped { "capped by" } else { "within" },
                    );
                    done.raster
                }
                Err(e) => operational_error(&format!("rendering within a budget: {e}")),
            }
        }
        None => libviprs::extract_page_image_dpi(path, page as u32, f64::from(dpi))
            .unwrap_or_else(|e| operational_error(&format!("rendering at {dpi} DPI: {e}"))),
    }
}

#[cfg(not(feature = "pdfium"))]
fn render_at_dpi(_: &Path, _: usize, _: u32, _: Option<u64>) -> libviprs::Raster {
    operational_error(
        "--dpi needs the `pdfium` feature, which was not compiled into this binary \
         (use a default-features build)",
    )
}

// ===========================================================================
// viprs geo
// ===========================================================================

/// `viprs geo`: the library's geo transform, one point at a time.
#[derive(Parser)]
pub struct GeoArgs {
    #[command(subcommand)]
    command: GeoCommand,
}

#[derive(Subcommand)]
enum GeoCommand {
    /// Map a pixel position to a geographic one. Prints `x,y`.
    PixelToGeo(PixelToGeoArgs),

    /// Map a geographic position back to a pixel. Prints `x,y`, unclamped and
    /// fractional. Exits 1 if the transform cannot be inverted.
    GeoToPixel(GeoToPixelArgs),

    /// The geographic position of the centre of a tile. Prints `x,y`.
    TileCenter(TileCenterArgs),
}

/// The transform: either origin and scale, or all six affine coefficients.
#[derive(clap::Args)]
#[command(group(
    ArgGroup::new("transform")
        .required(true)
        .args(["affine", "geo_origin"]),
))]
struct TransformArgs {
    /// Geographic position of the top-left pixel, as "x,y" (longitude,latitude).
    /// The same flag `viprs pyramid` takes. Needs `--geo-scale`.
    #[arg(
        long,
        value_name = "X,Y",
        requires = "geo_scale",
        allow_hyphen_values = true
    )]
    geo_origin: Option<String>,

    /// Geographic units per pixel, as "x,y" (use a negative y for a top-down
    /// raster). The same flag `viprs pyramid` takes. Needs `--geo-origin`.
    #[arg(
        long,
        value_name = "X,Y",
        requires = "geo_origin",
        allow_hyphen_values = true
    )]
    geo_scale: Option<String>,

    /// The full affine transform as "a,b,c,d,e,f": `x = a*px + b*py + c` and
    /// `y = d*px + e*py + f`. Use this for a rotated or skewed sheet.
    #[arg(
        long,
        value_name = "A,B,C,D,E,F",
        conflicts_with_all = ["geo_origin", "geo_scale"],
        allow_hyphen_values = true
    )]
    affine: Option<String>,
}

#[derive(Parser)]
struct PixelToGeoArgs {
    /// Pixel column (fractional allowed).
    #[arg(allow_negative_numbers = true)]
    x: f64,
    /// Pixel row (fractional allowed).
    #[arg(allow_negative_numbers = true)]
    y: f64,
    #[command(flatten)]
    transform: TransformArgs,
}

#[derive(Parser)]
struct GeoToPixelArgs {
    /// Geographic x (longitude).
    #[arg(allow_negative_numbers = true)]
    x: f64,
    /// Geographic y (latitude).
    #[arg(allow_negative_numbers = true)]
    y: f64,
    #[command(flatten)]
    transform: TransformArgs,
}

#[derive(Parser)]
struct TileCenterArgs {
    /// Tile column.
    tile_x: u32,
    /// Tile row.
    tile_y: u32,
    /// Tile size in pixels.
    #[arg(long, default_value = "256", value_parser = clap::value_parser!(u32).range(1..))]
    tile_size: u32,
    #[command(flatten)]
    transform: TransformArgs,
}

fn parse_floats(text: &str, flag: &str, want: usize) -> Vec<f64> {
    let values: Vec<f64> = text
        .split(',')
        .map(|v| {
            v.trim().parse::<f64>().unwrap_or_else(|_| {
                usage_error(&format!("--{flag} value {v:?} is not a number"), "")
            })
        })
        .collect();
    if values.len() != want {
        usage_error(
            &format!(
                "--{flag} needs {want} comma-separated numbers, got {}",
                values.len()
            ),
            "",
        );
    }
    values
}

fn build_transform(t: &TransformArgs) -> GeoTransform {
    if let Some(affine) = &t.affine {
        let v = parse_floats(affine, "affine", 6);
        return GeoTransform::new(v[0], v[1], v[2], v[3], v[4], v[5]);
    }
    // The group makes one of the two modes required, and `requires` ties the
    // origin to the scale, so both are present here.
    let origin = parse_floats(t.geo_origin.as_deref().unwrap_or_default(), "geo-origin", 2);
    let scale = parse_floats(t.geo_scale.as_deref().unwrap_or_default(), "geo-scale", 2);
    GeoTransform::from_origin_and_scale(GeoCoord::new(origin[0], origin[1]), scale[0], scale[1])
}

pub fn run_geo(args: GeoArgs) {
    match args.command {
        GeoCommand::PixelToGeo(a) => {
            let g = build_transform(&a.transform).pixel_to_geo(PixelCoord { x: a.x, y: a.y });
            println!("{},{}", g.x, g.y);
        }
        GeoCommand::GeoToPixel(a) => {
            let t = build_transform(&a.transform);
            match t.geo_to_pixel(GeoCoord::new(a.x, a.y)) {
                Some(p) => println!("{},{}", p.x, p.y),
                None => operational_error(
                    "this transform is singular, so a geographic position cannot be inverted \
                     back to a pixel",
                ),
            }
        }
        GeoCommand::TileCenter(a) => {
            let g = build_transform(&a.transform).tile_center(a.tile_x, a.tile_y, a.tile_size);
            println!("{},{}", g.x, g.y);
        }
    }
}

// ===========================================================================
// viprs plan: query flags
// ===========================================================================

/// Pixel formats `--estimate-memory` can be asked about.
#[derive(Clone, Copy, ValueEnum)]
pub enum PlanPixelFormat {
    Gray8,
    Gray16,
    Rgb8,
    Rgba8,
    Rgb16,
    Rgba16,
}

impl From<PlanPixelFormat> for PixelFormat {
    fn from(f: PlanPixelFormat) -> Self {
        match f {
            PlanPixelFormat::Gray8 => PixelFormat::Gray8,
            PlanPixelFormat::Gray16 => PixelFormat::Gray16,
            PlanPixelFormat::Rgb8 => PixelFormat::Rgb8,
            PlanPixelFormat::Rgba8 => PixelFormat::Rgba8,
            PlanPixelFormat::Rgb16 => PixelFormat::Rgb16,
            PlanPixelFormat::Rgba16 => PixelFormat::Rgba16,
        }
    }
}

/// The planner queries. At most one per run, and when one is given the command
/// prints only its answer (no table), so the output can be captured as is.
#[derive(clap::Args)]
#[command(group(
    ArgGroup::new("plan_query")
        .required(false)
        .multiple(false)
        .args([
            "estimate_memory",
            "dzi_manifest",
            "properties_sidecar",
            "tile_path",
            "tile_rect",
        ]),
))]
pub struct PlanQueryArgs {
    /// Print the streaming engine's estimated peak memory in bytes for this
    /// strip height, and nothing else.
    #[arg(long, value_name = "STRIP_HEIGHT", value_parser = clap::value_parser!(u32).range(1..))]
    estimate_memory: Option<u32>,

    /// Pixel format the memory estimate is for.
    #[arg(
        long,
        value_enum,
        default_value = "rgba8",
        requires = "estimate_memory"
    )]
    pixel_format: PlanPixelFormat,

    /// Print the Deep Zoom `.dzi` manifest for the given tile format (default
    /// `png`). Only `--layout deep-zoom` has one.
    #[arg(long, value_name = "FORMAT", num_args = 0..=1, default_missing_value = "png")]
    dzi_manifest: Option<String>,

    /// Print a layout's in-directory sidecar for the given tile extension: the
    /// relative path on the first line, then the content. Only `--layout
    /// zoomify` (ImageProperties.xml) and `--layout iiif` (info.json) have one.
    #[arg(long, value_name = "EXT")]
    properties_sidecar: Option<String>,

    /// Print the output path of one tile, given as LEVEL,COL,ROW.
    #[arg(long, value_name = "LEVEL,COL,ROW")]
    tile_path: Option<String>,

    /// File extension for `--tile-path`.
    #[arg(
        long,
        value_name = "EXT",
        default_value = "png",
        requires = "tile_path"
    )]
    tile_ext: String,

    /// Print the pixel rectangle one tile reads from, given as LEVEL,COL,ROW,
    /// as `x,y,width,height` (overlap included, edges clipped).
    #[arg(long, value_name = "LEVEL,COL,ROW")]
    tile_rect: Option<String>,
}

impl PlanQueryArgs {
    /// Whether a query was asked for, in which case the plan table is skipped.
    pub fn is_query(&self) -> bool {
        self.estimate_memory.is_some()
            || self.dzi_manifest.is_some()
            || self.properties_sidecar.is_some()
            || self.tile_path.is_some()
            || self.tile_rect.is_some()
    }
}

fn parse_tile_coord(text: &str, flag: &str) -> TileCoord {
    let parts: Vec<&str> = text.split(',').collect();
    let [level, col, row] = parts.as_slice() else {
        usage_error(&format!("--{flag} needs LEVEL,COL,ROW, got {text:?}"), "");
    };
    let n = |v: &str| {
        v.trim().parse::<u32>().unwrap_or_else(|_| {
            usage_error(&format!("--{flag} value {v:?} is not a whole number"), "")
        })
    };
    TileCoord::new(n(level), n(col), n(row))
}

fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::DeepZoom => "deep-zoom",
        Layout::Xyz => "xyz",
        Layout::Google => "google",
        Layout::Zoomify => "zoomify",
        Layout::Iiif => "iiif",
        _ => "this layout",
    }
}

/// Answer the one query that was asked for. Call only when
/// [`PlanQueryArgs::is_query`] is true.
pub fn run_plan_query(q: &PlanQueryArgs, plan: &PyramidPlan, layout: Layout) {
    if let Some(strip) = q.estimate_memory {
        let bytes = plan.estimate_streaming_peak_memory(q.pixel_format.into(), strip);
        println!("{bytes}");
    } else if let Some(format) = &q.dzi_manifest {
        match plan.dzi_manifest(format) {
            Some(xml) => println!("{xml}"),
            None => operational_error(&format!(
                "layout {} has no .dzi manifest (only deep-zoom does)",
                layout_name(layout)
            )),
        }
    } else if let Some(ext) = &q.properties_sidecar {
        match plan.properties_sidecar(ext) {
            Some((path, content)) => {
                println!("{path}");
                print!("{content}");
            }
            None => operational_error(&format!(
                "layout {} has no properties sidecar (only zoomify and iiif do)",
                layout_name(layout)
            )),
        }
    } else if let Some(spec) = &q.tile_path {
        let coord = parse_tile_coord(spec, "tile-path");
        match plan.tile_path(coord, &q.tile_ext) {
            Some(path) => println!("{path}"),
            None => operational_error(&format!("tile {spec} is out of range for this plan")),
        }
    } else if let Some(spec) = &q.tile_rect {
        let coord = parse_tile_coord(spec, "tile-rect");
        match plan.tile_rect(coord) {
            Some(r) => println!("{},{},{},{}", r.x, r.y, r.width, r.height),
            None => operational_error(&format!("tile {spec} is out of range for this plan")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libviprs::SourceError;

    fn no_stdin() -> std::io::Empty {
        std::io::empty()
    }

    fn temp_file(tag: &str, contents: &[u8]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("viprs-pgp-unit-{}-{tag}", std::process::id()));
        std::fs::write(&path, contents).expect("the temp file must be writable");
        path
    }

    #[test]
    fn a_password_file_gives_its_contents_without_the_trailing_newline() {
        let lf = temp_file("lf", b"s3cret pass\n");
        let crlf = temp_file("crlf", b"s3cret pass\r\n");
        let bare = temp_file("bare", b"s3cret pass");
        for path in [&lf, &crlf, &bare] {
            assert_eq!(
                resolve_password(None, Some(path), None, &mut no_stdin()),
                Ok(Some("s3cret pass".to_owned())),
                "{}",
                path.display()
            );
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn a_password_file_keeps_everything_but_one_trailing_newline() {
        // Spaces are legal in a password, and only the one newline an editor
        // or `echo` adds is not part of it.
        let path = temp_file("spaces", b"  two words  \n\n");
        assert_eq!(
            resolve_password(None, Some(&path), None, &mut no_stdin()),
            Ok(Some("  two words  \n".to_owned()))
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_password_file_of_dash_reads_stdin() {
        let mut stdin: &[u8] = b"from-stdin\n";
        assert_eq!(
            resolve_password(None, Some(Path::new("-")), None, &mut stdin),
            Ok(Some("from-stdin".to_owned()))
        );
    }

    #[test]
    fn the_environment_is_the_fallback_when_no_flag_is_given() {
        assert_eq!(
            resolve_password(None, None, Some("from-env".into()), &mut no_stdin()),
            Ok(Some("from-env".to_owned()))
        );
        // An empty variable is the same as an unset one.
        assert_eq!(
            resolve_password(None, None, Some("".into()), &mut no_stdin()),
            Ok(None)
        );
        assert_eq!(
            resolve_password(None, None, None, &mut no_stdin()),
            Ok(None)
        );
    }

    #[test]
    fn a_flag_or_a_file_wins_over_the_environment() {
        assert_eq!(
            resolve_password(
                Some("from-flag"),
                None,
                Some("from-env".into()),
                &mut no_stdin()
            ),
            Ok(Some("from-flag".to_owned()))
        );
        let path = temp_file("over-env", b"from-file\n");
        assert_eq!(
            resolve_password(None, Some(&path), Some("from-env".into()), &mut no_stdin()),
            Ok(Some("from-file".to_owned()))
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unreadable_password_file_is_an_error_naming_it_but_not_the_password() {
        let missing = std::env::temp_dir().join("viprs-pgp-unit-no-such-file");
        let err = resolve_password(None, Some(&missing), None, &mut no_stdin())
            .expect_err("a missing password file must not be read as no password");
        assert!(err.contains("viprs-pgp-unit-no-such-file"), "{err}");
    }

    #[test]
    fn a_typed_source_error_gets_the_password_messages() {
        let path = Path::new("locked.pdf");
        let required = SourceError::Pdf(PdfError::PasswordRequired);
        let message = password_failure_message(path, &required)
            .expect("SourceError::Pdf(PasswordRequired) should be recognised");
        assert!(message.contains("locked.pdf is encrypted and a password is needed"));

        let wrong = SourceError::Pdf(PdfError::WrongPassword);
        let message = password_failure_message(path, &wrong)
            .expect("SourceError::Pdf(WrongPassword) should be recognised");
        assert!(message.contains("wrong password for locked.pdf"));
    }

    #[test]
    fn the_info_path_s_bare_pdf_error_gets_the_password_messages_too() {
        let path = Path::new("locked.pdf");
        assert!(
            password_failure_message(path, &PdfError::PasswordRequired)
                .is_some_and(|m| m.contains("is encrypted and a password is needed"))
        );
        assert!(
            password_failure_message(path, &PdfError::WrongPassword)
                .is_some_and(|m| m.contains("wrong password"))
        );
    }

    #[test]
    fn other_pdf_failures_keep_the_library_s_wording() {
        let err = SourceError::Pdf(PdfError::PageOutOfRange { page: 9, total: 1 });
        assert_eq!(password_failure_message(Path::new("a.pdf"), &err), None);
    }
}
