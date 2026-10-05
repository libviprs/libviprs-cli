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
//! password the same calls are made with an empty one, which is how a
//! document that needs one gets reported as needing it at all. The info call
//! returns the `PdfError` itself and the extract call returns it as
//! `SourceError::Pdf`, and [`password_failure_message`] matches both rather
//! than reading text. A build without the `pdfium` feature cannot decrypt, so
//! there the library's own "not available in this build" is what gets printed.
//!
//! The password comes from `--password`, `--password-file PATH` (`-` reads
//! stdin) or the `VIPRS_PDF_PASSWORD` environment variable, in that order.
//! `--password` is kept for convenience, but anything on the command line can
//! be read by every user of the machine through `ps` or `/proc/PID/cmdline`
//! and lands in shell history, so the help points at the other two. No message
//! here ever prints the password.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use libviprs::pdf::PdfError;
use libviprs::{
    GeoCoord, GeoTransform, Layout, PixelCoord, PixelFormat, PyramidPlan, SourceError,
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

/// The environment variable a password is read from when neither
/// `--password` nor `--password-file` is given.
const PASSWORD_ENV: &str = "VIPRS_PDF_PASSWORD";

/// Where an encrypted PDF's password comes from. With neither flag,
/// `VIPRS_PDF_PASSWORD` is read; an empty or unset variable means no password.
#[derive(clap::Args)]
struct PasswordArgs {
    /// Password for an encrypted PDF. Anything on the command line is visible
    /// to every user of the machine through `ps` and /proc, and is kept in
    /// shell history, so prefer --password-file or the VIPRS_PDF_PASSWORD
    /// environment variable outside a throwaway shell.
    #[arg(long, conflicts_with = "password_file")]
    password: Option<String>,

    /// Read the password from a file, or from stdin with `-`, keeping it off
    /// the command line (and out of `ps`). One trailing newline is dropped;
    /// everything else, spaces included, is the password. With neither this
    /// nor --password, VIPRS_PDF_PASSWORD is used if set.
    #[arg(long, value_name = "PATH")]
    password_file: Option<PathBuf>,
}

impl PasswordArgs {
    /// The password to open the file with, or `None` for none. Exits 1 if
    /// `--password-file` cannot be read.
    fn resolve(&self) -> Option<String> {
        resolve_password(
            self.password.as_deref(),
            self.password_file.as_deref(),
            std::env::var_os(PASSWORD_ENV),
            &mut std::io::stdin().lock(),
        )
        .unwrap_or_else(|message| operational_error(&message))
    }
}

/// Pick the password from the flag, then the file (`-` is `stdin`), then the
/// environment value `env`. One trailing `\n` or `\r\n` is dropped from a file
/// or stdin, since an editor or `echo` adds it and it is never part of the
/// password. An empty environment value counts as unset. The error names the
/// file that could not be read and never carries the password.
fn resolve_password(
    flag: Option<&str>,
    file: Option<&Path>,
    env: Option<OsString>,
    stdin: &mut dyn Read,
) -> Result<Option<String>, String> {
    if let Some(password) = flag {
        return Ok(Some(password.to_owned()));
    }
    if let Some(path) = file {
        let mut text = String::new();
        if path == Path::new("-") {
            stdin.read_to_string(&mut text).map_err(|e| {
                format!("cannot read the password from stdin (--password-file -): {e}")
            })?;
        } else {
            text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read --password-file {}: {e}", path.display()))?;
        }
        if let Some(stripped) = text.strip_suffix('\n') {
            let stripped = stripped.strip_suffix('\r').unwrap_or(stripped);
            text.truncate(stripped.len());
        }
        return Ok(Some(text));
    }
    match env {
        Some(value) if !value.is_empty() => value
            .into_string()
            .map(Some)
            .map_err(|_| format!("{PASSWORD_ENV} is not valid UTF-8")),
        _ => Ok(None),
    }
}

#[derive(Parser)]
struct PdfInfoArgs {
    /// The PDF to describe.
    input: PathBuf,

    #[command(flatten)]
    password: PasswordArgs,
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

    /// The password, for an encrypted PDF. None of these combine with a render
    /// option, because the render path takes no password, and
    /// VIPRS_PDF_PASSWORD is not read for a render.
    #[command(flatten)]
    password: PasswordArgs,

    /// Render the page through pdfium at this DPI (one PDF point is one pixel
    /// at 72).
    #[arg(
        long,
        value_name = "DPI",
        value_parser = clap::value_parser!(u32).range(1..),
        conflicts_with_all = ["password", "password_file"]
    )]
    dpi: Option<u32>,

    /// Render over a solid fill, as `r,g,b` or `r,g,b,a` with each channel
    /// 0..=255, instead of the default white. Renders at 72 DPI, so it cannot
    /// be combined with `--dpi`.
    #[arg(
        long,
        value_name = "R,G,B[,A]",
        conflicts_with_all = ["dpi", "render_budget", "password", "password_file"]
    )]
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

/// The message for a missing or a wrong password, if `err` is one of those.
///
/// `pdf_info_with_password` returns the [`PdfError`] itself and the extract
/// calls return it as [`SourceError::Pdf`], so those are the two shapes looked
/// for. Neither message carries the password.
fn password_failure_message(
    path: &Path,
    err: &(dyn std::error::Error + 'static),
) -> Option<String> {
    let pdf = match err.downcast_ref::<SourceError>() {
        Some(SourceError::Pdf(pdf)) => Some(pdf),
        _ => err.downcast_ref::<PdfError>(),
    };
    match pdf? {
        PdfError::PasswordRequired => Some(format!(
            "{} is encrypted and a password is needed (pass --password-file, set \
             {PASSWORD_ENV}, or pass --password)",
            path.display()
        )),
        PdfError::WrongPassword => Some(format!(
            "wrong password for {} (the password given does not open it)",
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

fn require_file(path: &Path) {
    if !path.exists() {
        operational_error(&format!("file not found: {}", path.display()));
    }
}

fn run_pdf_info(args: PdfInfoArgs) {
    require_file(&args.input);
    // An empty password is how the library is asked "is this one locked?".
    let password = args.password.resolve();
    let info = pdf_info_with_password(&args.input, password.as_deref().unwrap_or(""));
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

/// Parse `--background` into the channel slice the library takes: 3 or 4
/// finite channels, each 0..=255.
fn parse_background(text: &str) -> Vec<f64> {
    const HINT: &str =
        "write it as r,g,b or r,g,b,a with each channel 0..=255, for example 255,0,0";
    let channels = parse_finite_list(text, "background", HINT);
    if !(3..=4).contains(&channels.len()) {
        usage_error(
            &format!(
                "--background needs 3 (r,g,b) or 4 (r,g,b,a) channels, got {}",
                channels.len()
            ),
            HINT,
        );
    }
    if let Some(c) = channels.iter().find(|c| !(0.0..=255.0).contains(*c)) {
        usage_error(
            &format!("--background channel {c} is outside 0..=255"),
            HINT,
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
        render_at_dpi(&args.input, args.page, page_u32, dpi, args.render_budget)
    } else {
        // With no password this asks with an empty one, so a file that needs
        // one is reported as needing it instead of failing on its streams.
        let password = args.password.resolve();
        let pw = password.as_deref().unwrap_or("");
        extract_page_image_with_password(&args.input, page_u32, pw).unwrap_or_else(|e| {
            pdf_failure(&format!("extracting page {}", args.page), &args.input, &e)
        })
    };

    if let Err(e) = ops::io::save(&raster, &args.output) {
        // An output extension nothing here writes is decided by the path
        // alone, so it's a usage mistake on this built-in too (#78).
        if ops::is_usage_error(&e) {
            usage_error(&format!("{e:#}"), "");
        }
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
fn render_at_dpi(
    path: &Path,
    page: usize,
    page_u32: u32,
    dpi: u32,
    budget: Option<u64>,
) -> libviprs::Raster {
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
        None => libviprs::extract_page_image_dpi(path, page_u32, f64::from(dpi))
            .unwrap_or_else(|e| operational_error(&format!("rendering at {dpi} DPI: {e}"))),
    }
}

#[cfg(not(feature = "pdfium"))]
fn render_at_dpi(_: &Path, _: usize, _: u32, _: u32, _: Option<u64>) -> libviprs::Raster {
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
    #[arg(allow_negative_numbers = true, value_parser = finite_f64)]
    x: f64,
    /// Pixel row (fractional allowed).
    #[arg(allow_negative_numbers = true, value_parser = finite_f64)]
    y: f64,
    #[command(flatten)]
    transform: TransformArgs,
}

#[derive(Parser)]
struct GeoToPixelArgs {
    /// Geographic x (longitude).
    #[arg(allow_negative_numbers = true, value_parser = finite_f64)]
    x: f64,
    /// Geographic y (latitude).
    #[arg(allow_negative_numbers = true, value_parser = finite_f64)]
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

/// A geo coordinate positional: any finite number. NaN and the infinities
/// parse as `f64` but cannot be mapped anywhere, so clap refuses them (exit 2).
fn finite_f64(text: &str) -> Result<f64, String> {
    match text.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        Ok(v) => Err(format!("{v} is not a finite number")),
        Err(e) => Err(e.to_string()),
    }
}

/// The flags that take a number list and so allow values starting with `-`.
const LIST_FLAGS: &[&str] = &["--geo-origin", "--geo-scale", "--affine"];

/// The first number-list flag in `args` whose value is another flag, as
/// `(flag, value)`.
///
/// `--geo-origin`, `--geo-scale` and `--affine` allow hyphen values so that
/// `-122.4,37.7` parses, which also lets clap hand them whatever comes next,
/// so a forgotten value takes the following flag as the value and that flag
/// never applies. Nothing that starts with `--` is a number, so treating such
/// a value as a mistake refuses nothing real. This runs on the raw arguments
/// rather than as a clap value parser because clap parses an option's value
/// late: an extra positional after the swallowed flag is reported first, as
/// an "unexpected argument" that names neither flag.
pub(crate) fn swallowed_flag(args: &[std::ffi::OsString]) -> Option<(String, String)> {
    args.windows(2).find_map(|pair| {
        let flag = pair[0].to_str()?;
        let value = pair[1].to_str()?;
        (LIST_FLAGS.contains(&flag) && value.starts_with("--"))
            .then(|| (flag.to_owned(), value.to_owned()))
    })
}

/// Exit 2 when a number-list flag swallowed the next flag (see
/// [`swallowed_flag`]), before clap or anything else reads the arguments.
pub(crate) fn refuse_swallowed_flag(args: &[std::ffi::OsString]) {
    if let Some((flag, value)) = swallowed_flag(args) {
        usage_error(
            &format!("{flag} got {value:?} as its value, and that looks like a flag, not a value"),
            &format!(
                "give {flag} its value, or join the two with = ({flag}=VALUE), which also takes \
                 a value that starts with -"
            ),
        );
    }
}

/// Parse `--{flag}` as comma-separated finite numbers, exiting 2 (with
/// `hint`) for anything that is not one. NaN, `inf` and values like `1e400`
/// that overflow to infinity are refused, since nothing downstream can use
/// them. The one number-list parser behind `--affine`, `--geo-origin`,
/// `--geo-scale` (here and on `viprs pyramid`) and `--background`.
fn parse_finite_list(text: &str, flag: &str, hint: &str) -> Vec<f64> {
    text.split(',')
        .map(|v| match v.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => n,
            Ok(_) => usage_error(
                &format!("--{flag} value {v:?} is not a finite number"),
                hint,
            ),
            Err(_) => usage_error(&format!("--{flag} value {v:?} is not a number"), hint),
        })
        .collect()
}

/// [`parse_finite_list`] for exactly `want` numbers.
pub(crate) fn parse_floats(text: &str, flag: &str, want: usize) -> Vec<f64> {
    let names = match want {
        2 => "x,y",
        6 => "a,b,c,d,e,f",
        _ => "",
    };
    let hint = if names.is_empty() {
        String::new()
    } else {
        format!("write it as {names}")
    };
    let values = parse_finite_list(text, flag, &hint);
    if values.len() != want {
        usage_error(
            &format!(
                "--{flag} needs {want} comma-separated numbers, got {}",
                values.len()
            ),
            &hint,
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

/// `viprs plan --layout`. It takes the three layouts `pyramid --layout`
/// writes, plus `zoomify` and `iiif`, which only `plan` offers: it can answer
/// their sidecar and tile-path questions, while `pyramid` has no cells for
/// writing them and an archive cannot hold them. Kept apart from the
/// pyramid's own enum so adding a plan layout never widens `pyramid --layout`.
#[derive(Clone, Copy, ValueEnum)]
pub enum PlanLayoutArg {
    DeepZoom,
    Xyz,
    Google,
    Zoomify,
    Iiif,
}

impl From<PlanLayoutArg> for Layout {
    fn from(arg: PlanLayoutArg) -> Self {
        match arg {
            PlanLayoutArg::DeepZoom => Layout::DeepZoom,
            PlanLayoutArg::Xyz => Layout::Xyz,
            PlanLayoutArg::Google => Layout::Google,
            PlanLayoutArg::Zoomify => Layout::Zoomify,
            PlanLayoutArg::Iiif => Layout::Iiif,
        }
    }
}

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
    ///
    /// Give the format with `=` (`--dzi-manifest=jpg`): a bare
    /// `--dzi-manifest` means `png` and never takes the next word, so it
    /// cannot swallow the input.
    #[arg(
        long,
        value_name = "FORMAT",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "png"
    )]
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

    fn os(args: &[&str]) -> Vec<std::ffi::OsString> {
        args.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn a_list_flag_followed_by_a_flag_is_a_swallow() {
        for flag in ["--geo-origin", "--geo-scale", "--affine"] {
            assert_eq!(
                swallowed_flag(&os(&["viprs", "geo", flag, "--centre", "1"])),
                Some((flag.to_owned(), "--centre".to_owned()))
            );
        }
    }

    #[test]
    fn negative_values_and_joined_values_are_not_a_swallow() {
        assert_eq!(
            swallowed_flag(&os(&[
                "viprs",
                "--geo-origin",
                "-122.4,37.7",
                "--geo-scale=-1,-1",
                "--affine",
                "-1,0,0,0,-1,0",
                "--render",
            ])),
            None
        );
        // Another flag's value that happens to look like one is not ours.
        assert_eq!(swallowed_flag(&os(&["viprs", "--page", "--centre"])), None);
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
