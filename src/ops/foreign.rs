//! Foreign (codec load/save) family: every codec libviprs ships, reachable
//! under the name vips gives it (libviprs-cli#65, `OP_MAP.md` foreign section).
//!
//! `viprs` used to write three containers and read whatever the core's content
//! sniff happened to reach, with no way to pass a codec option. This family
//! gives each codec a `*save` and a `*load` command spelled the way vips
//! spells it, carrying the options the core can actually honour as flags:
//!
//! | save | options | load | options |
//! |---|---|---|---|
//! | `jpegsave` | `--Q`, `--subsample-mode` | `jpegload` | `--shrink` |
//! | `pngsave` | `--compression`, `--interlace`, `--palette`, `--bitdepth` | `pngload` | |
//! | `tiffsave` | `--compression` | `tiffload` | `--page`, `--max-pages` |
//! | `webpsave` | `--lossless` | `webpload` | `--page`, `--n` |
//! | `gifsave` | `--dither`, `--bitdepth`, `--interlace` | `gifload` | `--page`, `--n`, `--max-pages` |
//! | `jxlsave` | `--lossless` | `jxlload` | |
//! | `jp2ksave` | `--lossless`, `--tile-width`, `--tile-height` | `jp2kload` | |
//! | `fitssave` | | `fitsload` | |
//! | `radsave` | | `radload` | |
//! | `uhdrsave` | `--Q`, `--gainmap-scale-factor` | `uhdrload` | |
//! | `csvsave` | | `csvload` | |
//! | `matrixsave` | | `matrixload` | |
//! | `ppmsave` | | `ppmload` | |
//! | | | `heifload` (AVIF only), `svgload` (`--dpi`, `--scale`, `--unlimited`), `openexrload`, `niftiload`, `analyzeload`, `matload` | |
//!
//! Every loader takes the five shared `--max-*` decode limits, and `-` as its
//! input reads the image from stdin. Nothing here writes stdout, so an OUT of
//! `-` is refused while the arguments are parsed (exit 2) rather than creating
//! a file called `-`. The core bounds allocation with
//! `max_coord`, `max_pixels` and `max_alloc_bytes` before it reserves a frame,
//! but only its `image`-crate paths look at `max_width` / `max_height`, so the
//! loaders here check the decoded geometry against all of them afterwards
//! as well; a flag the help text promises is a flag every loader honours.
//!
//! `csvload` and `matrixload` are the two whose core decoders take no limits
//! at all, so their grid is measured from the text and priced before the
//! decode: width x height x 4 bytes, times the copies of the grid the core
//! holds while it builds the raster, against `--max-alloc-bytes`. A ragged
//! CSV is priced at the padded width, because that is what `csv_load` builds.
//!
//! # Where this departs from vips
//!
//! `pngsave --bitdepth` only means something with `--palette` here (vips also
//! reduces a plain PNG's depth), and `--compression` conflicts with
//! `--interlace` and `--palette`, because the core's interlaced and palette
//! encoders take no deflate level. `jpegload --shrink` decodes the whole image
//! and box-shrinks it afterwards, where vips shrinks inside libjpeg, so the
//! limits apply to the full-size decode. `jpegload` cannot write a `.jpg` OUT:
//! loaders save through the shared op sink, which bans `.jpg` (`jpegsave` is
//! the way to write one).
//!
//! # What is deliberately not here
//!
//! The core has an entry point for each of these and every one of them is a
//! refusal stub, so a flag for it would only move the refusal to the command
//! line: JPEG `restart-interval` (`jpegsave_buffer_restart`), tiled, BigTIFF
//! and multi-page TIFF writing (`save_tiff_tiled`, `save_bigtiff`), and the
//! `foreign_stubs` codecs (HEIF/HEIC, OpenSlide, ImageMagick, `dzsave`).
//! `decode_file_sequential` is a documented alias of `decode_file`, so an
//! `--access sequential` flag would be one no output could ever tell apart.
//!
//! # Lossless-only encoders
//!
//! `webpsave`, `jxlsave` and `jp2ksave` default to lossy in vips, and the core
//! has no encoder that matches vips's lossy modes (WebP and JPEG XL have no
//! lossy encoder at all, and the JPEG 2000 one takes a rate, not vips's `Q`).
//! Writing lossless when someone typed vips's default would be a silent
//! change of meaning, so each of them makes `--lossless` a required argument:
//! leaving it out is a usage error (exit 2, shown in the usage line) caught
//! before any input is read. The extension route (`viprs copy in.png
//! out.webp`) is the core's own `Raster::save` table and writes lossless, as
//! it always has.

use std::io::Read as _;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::source::DecodeLimits;
use libviprs::{Interpretation, JpegSubsample, Raster, TiffCompression};

use super::{CommandMeta, OracleClass, Shape, io};
use crate::features::MissingEncoder;

/// Long name of the page-walk ceiling the multi-page loaders add to the
/// shared five (`DecodeLimits::max_pages`).
const MAX_PAGES: &str = "max-pages";

/// Every command in the family, with its oracle class. The class is the one
/// `tests/cli_foreign_diff.rs` in libviprs-tests holds it to.
const METAS: &[(&str, OracleClass)] = &[
    ("jpegsave", OracleClass::BoundedTol),
    ("pngsave", OracleClass::Exact),
    ("tiffsave", OracleClass::Exact),
    ("webpsave", OracleClass::Exact),
    ("gifsave", OracleClass::BoundedTol),
    ("jxlsave", OracleClass::Exact),
    ("jp2ksave", OracleClass::Exact),
    ("fitssave", OracleClass::Exact),
    ("radsave", OracleClass::Exact),
    ("uhdrsave", OracleClass::BoundedTol),
    ("csvsave", OracleClass::Exact),
    ("matrixsave", OracleClass::Exact),
    ("ppmsave", OracleClass::Exact),
    ("jpegload", OracleClass::BoundedTol),
    ("pngload", OracleClass::Exact),
    ("tiffload", OracleClass::Exact),
    ("webpload", OracleClass::Exact),
    ("gifload", OracleClass::Exact),
    ("jxlload", OracleClass::Exact),
    ("jp2kload", OracleClass::Exact),
    ("heifload", OracleClass::Exact),
    ("svgload", OracleClass::Exact),
    ("fitsload", OracleClass::Exact),
    ("radload", OracleClass::Exact),
    ("uhdrload", OracleClass::BoundedTol),
    ("csvload", OracleClass::Exact),
    ("matrixload", OracleClass::Exact),
    ("ppmload", OracleClass::Exact),
    ("openexrload", OracleClass::Exact),
    ("niftiload", OracleClass::Exact),
    ("analyzeload", OracleClass::Exact),
    ("matload", OracleClass::Exact),
];

/// Static command metadata for the family (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    METAS
        .iter()
        .map(|&(name, oracle_class)| CommandMeta {
            name,
            shape: Shape::ImageToImage,
            oracle_class,
        })
        .collect()
}

fn in_out(cmd: Command, input: &'static str) -> Command {
    cmd.arg(Arg::new("IN").required(true).help(input)).arg(
        Arg::new("OUT")
            .required(true)
            .value_parser(output_path)
            .help("Output file (not -: nothing here writes stdout)"),
    )
}

/// OUT is always a file. A loader's IN takes `-` for stdin, so `-` as OUT
/// reads like stdout and would quietly create a file called `-` instead.
fn output_path(s: &str) -> std::result::Result<String, String> {
    if s == "-" {
        Err(
            "OUT cannot be -: these commands write a file and never stdout \
             (only a loader's IN reads - as stdin)"
                .to_owned(),
        )
    } else {
        Ok(s.to_owned())
    }
}

/// A finite number above zero, for `svgload --dpi` and `--scale`: the core
/// rounds NaN, infinities and negatives to a zero-sized render, so they are
/// refused while parsing instead (exit 2).
fn positive_finite(s: &str) -> std::result::Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => Ok(v),
        Ok(_) => Err("must be a finite number above 0".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

/// A finite number from 0 to 1 inclusive, for `gifsave --dither`.
fn unit_interval(s: &str) -> std::result::Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if (0.0..=1.0).contains(&v) => Ok(v),
        Ok(_) => Err("must be a number from 0 to 1".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

fn saver(name: &'static str, about: &'static str) -> Command {
    io::with_decode_limit_args(in_out(Command::new(name).about(about), "Input image"))
}

fn loader(name: &'static str, about: &'static str) -> Command {
    io::with_decode_limit_args(in_out(
        Command::new(name).about(about),
        "Input file, or - to read it from stdin",
    ))
}

fn flag(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(id).action(ArgAction::SetTrue).help(help)
}

fn lossless() -> Arg {
    flag(
        "lossless",
        "Write lossless (required: vips's default is lossy, which this build cannot encode)",
    )
    .required(true)
}

/// `--page` and `--n` for the multi-page loaders, and `--max-pages` for the
/// two whose core decoder walks its page chain under `DecodeLimits::max_pages`
/// (TIFF's IFD walk and GIF's frame scan). The WebP decoder does not consult
/// it, so `webpload` does not offer a flag that would do nothing.
fn page_args(cmd: Command, with_n: bool, with_max_pages: bool) -> Command {
    let mut cmd = cmd.arg(
        Arg::new("page")
            .long("page")
            .value_name("N")
            .value_parser(value_parser!(u32))
            .help("First page to load, counting from 0"),
    );
    if with_max_pages {
        cmd = cmd.arg(
            Arg::new(MAX_PAGES)
                .long(MAX_PAGES)
                .value_name("N")
                .value_parser(value_parser!(u32).range(1..))
                .help("Reject a file declaring more than N pages (DecodeLimits::max_pages)"),
        );
    }
    if with_n {
        cmd = cmd.arg(
            Arg::new("n")
                .long("n")
                .value_name("N")
                .allow_negative_numbers(true)
                .value_parser(value_parser!(i32))
                .help("Number of pages to load, -1 for every page from --page on"),
        );
    }
    cmd
}

/// The clap commands this family contributes.
pub fn commands() -> Vec<Command> {
    vec![
        saver("jpegsave", "Save an image as JPEG.")
            .arg(
                Arg::new("Q")
                    .long("Q")
                    .value_name("1-100")
                    .value_parser(value_parser!(u8).range(1..=100))
                    .default_value("75")
                    .help("Quality factor"),
            )
            .arg(
                Arg::new("subsample-mode")
                    .long("subsample-mode")
                    .value_parser(["auto", "on", "off"])
                    .default_value("auto")
                    .help("Chroma subsampling: auto (4:2:0 below Q 90), on (4:2:0), off (4:4:4)"),
            ),
        saver("pngsave", "Save an image as PNG.")
            .arg(
                Arg::new("compression")
                    .long("compression")
                    .value_name("0-9")
                    .value_parser(value_parser!(u8).range(0..=9))
                    .conflicts_with_all(["interlace", "palette"])
                    .help(
                        "Deflate level for a plain PNG (default 6); unlike vips it conflicts \
                         with --interlace and --palette, whose encoders take no level",
                    ),
            )
            .arg(flag("interlace", "Write an Adam7-interlaced PNG"))
            .arg(
                flag("palette", "Quantise to an indexed (palette) PNG").conflicts_with("interlace"),
            )
            .arg(
                Arg::new("bitdepth")
                    .long("bitdepth")
                    .value_name("1|2|4|8")
                    .value_parser(["1", "2", "4", "8"])
                    .requires("palette")
                    .help(
                        "With --palette, hold at most 2^N colours (default 8); unlike vips it \
                         needs --palette",
                    ),
            ),
        saver("tiffsave", "Save an image as a single-page TIFF.").arg(
            Arg::new("compression")
                .long("compression")
                .value_parser(["none", "lzw", "deflate"])
                .default_value("none")
                .help("Strip compression"),
        ),
        saver("webpsave", "Save an image as lossless WebP.").arg(lossless()),
        saver("gifsave", "Save an image as GIF.")
            .arg(
                Arg::new("dither")
                    .long("dither")
                    .value_name("0-1")
                    .value_parser(unit_interval)
                    .default_value("1")
                    .help("Amount of dithering during palette quantisation"),
            )
            .arg(
                Arg::new("bitdepth")
                    .long("bitdepth")
                    .value_name("1-8")
                    .value_parser(value_parser!(u8).range(1..=8))
                    .default_value("8")
                    .help("Bits per pixel: the palette holds at most 2^N colours (255 at 8)"),
            )
            .arg(flag("interlace", "Write the frame interlaced")),
        saver(
            "jxlsave",
            "Save an image as lossless JPEG XL (needs the `jxl` feature).",
        )
        .arg(lossless()),
        saver(
            "jp2ksave",
            "Save an image as lossless JPEG 2000 (needs the `jp2k` feature).",
        )
        .arg(lossless())
        .arg(
            Arg::new("tile-width")
                .long("tile-width")
                .value_name("PX")
                .value_parser(value_parser!(u32).range(1..))
                .default_value("512")
                .help("Tile width in pixels"),
        )
        .arg(
            Arg::new("tile-height")
                .long("tile-height")
                .value_name("PX")
                .value_parser(value_parser!(u32).range(1..))
                .default_value("512")
                .help("Tile height in pixels"),
        ),
        saver("fitssave", "Save an image as FITS."),
        saver(
            "radsave",
            "Save a three-band float image as Radiance HDR (.hdr).",
        ),
        saver(
            "uhdrsave",
            "Save an HDR image as Ultra HDR JPEG (converted to scRGB first).",
        )
        .arg(
            Arg::new("Q")
                .long("Q")
                .value_name("1-100")
                .value_parser(value_parser!(u8).range(1..=100))
                .default_value("75")
                .help("JPEG quality of both halves"),
        )
        .arg(
            Arg::new("gainmap-scale-factor")
                .long("gainmap-scale-factor")
                .value_name("N")
                .value_parser(value_parser!(u32).range(1..=128))
                .default_value("2")
                .help("How much smaller than the base the gain map is, per axis"),
        ),
        saver("csvsave", "Save a one-band image as tab-separated values."),
        saver("matrixsave", "Save a one-band image as a vips text matrix."),
        saver(
            "ppmsave",
            "Save a one- or three-band integer image as binary PGM/PPM.",
        ),
        loader(
            "jpegload",
            "Load a JPEG image (OUT cannot be .jpg: the shared op sink bans it, use jpegsave).",
        )
        .arg(
            Arg::new("shrink")
                .long("shrink")
                .value_parser(["1", "2", "4", "8"])
                .default_value("1")
                .help(
                    "Shrink by this integer factor after a full-size decode (vips shrinks \
                     inside the decoder; the --max-* limits see the full size here)",
                ),
        ),
        loader("pngload", "Load a PNG image."),
        page_args(
            loader("tiffload", "Load one page of a TIFF image."),
            false,
            true,
        ),
        page_args(loader("webpload", "Load WebP frames."), true, false),
        page_args(loader("gifload", "Load GIF frames."), true, true),
        loader("jxlload", "Load a JPEG XL image (needs the `jxl` feature)."),
        loader(
            "jp2kload",
            "Load a JPEG 2000 image (needs the `jp2k` feature).",
        ),
        loader(
            "heifload",
            "Load an AVIF image (needs the `avif` feature; HEIC has no decoder).",
        ),
        loader(
            "svgload",
            "Render an SVG document (needs the `svg` feature).",
        )
        .arg(
            Arg::new("dpi")
                .long("dpi")
                .value_parser(positive_finite)
                .default_value("72")
                .help("Render at this DPI"),
        )
        .arg(
            Arg::new("scale")
                .long("scale")
                .value_parser(positive_finite)
                .default_value("1")
                .help("Scale the rendered output by this factor"),
        )
        .arg(flag(
            "unlimited",
            "Lift the 10 MB SVG input ceiling (never the --max-* limits)",
        )),
        loader("fitsload", "Load a FITS image."),
        loader("radload", "Load a Radiance HDR image as three-band float."),
        loader("uhdrload", "Load the SDR base of an Ultra HDR JPEG."),
        loader(
            "csvload",
            "Load comma- or tab-separated values as a float image.",
        ),
        loader("matrixload", "Load a vips text matrix as a float image."),
        loader("ppmload", "Load a binary or ASCII PBM/PGM/PPM image."),
        loader("openexrload", "Load an OpenEXR image."),
        loader("niftiload", "Load a NIfTI image."),
        loader(
            "analyzeload",
            "Load an Analyze 7.5 image from its .hdr (the .img must sit beside it).",
        ),
        loader("matload", "Load a MATLAB level-5 .mat variable."),
    ]
}

/// Handler for a matched command of this family.
///
/// # Errors
///
/// A load, encode or write failure, a limit exceeded, or a codec this build
/// was compiled without.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    let out = PathBuf::from(pos(m, "OUT"));
    if let Some(format) = name.strip_suffix("save") {
        // A build without the encoder refuses before the input is read, so
        // nothing gets decoded only to be thrown away.
        require_saver(format)?;
        let raster = io::load(Path::new(pos(m, "IN")), &io::decode_limits(m))?;
        return save(format, &raster, m, &out);
    }
    let raster = decode(name, m)?;
    check_geometry(&raster, &io::decode_limits(m))?;
    io::save(&raster, &out)
}

fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .unwrap_or_default()
}

/// The encoder a `*save` command needs, checked before anything is read.
fn require_saver(format: &str) -> Result<()> {
    match format {
        "jxl" => require_encoder(cfg!(feature = "jxl"), "jxl", "JPEG XL"),
        "jp2k" => require_encoder(cfg!(feature = "jp2k"), "jp2k", "JPEG 2000"),
        _ => Ok(()),
    }
}

fn save(format: &str, raster: &Raster, m: &ArgMatches, out: &Path) -> Result<()> {
    if format == "tiff" {
        // `save_tiff` is the core's one compression-choosing entry point and
        // it writes the path itself, so it gets OUT directly: no temp file
        // beside it to plant a symlink at, and no encode, read back and write
        // again to get the bytes out.
        let compression = match pos(m, "compression") {
            "lzw" => TiffCompression::Lzw,
            "deflate" => TiffCompression::Deflate,
            _ => TiffCompression::None,
        };
        return io::to_integer_encodable(raster)?
            .save_tiff(out, compression)
            .with_context(|| format!("failed to write {}", out.display()));
    }
    let bytes = encode(format, raster, m)?;
    std::fs::write(out, bytes).with_context(|| format!("failed to write {}", out.display()))
}

fn encode(format: &str, raster: &Raster, m: &ArgMatches) -> Result<Vec<u8>> {
    let integer = || io::to_integer_encodable(raster);
    Ok(match format {
        "jpeg" => {
            let q = *m.get_one::<u8>("Q").expect("defaulted");
            let mode = match pos(m, "subsample-mode") {
                "on" => JpegSubsample::On,
                "off" => JpegSubsample::Off,
                _ => JpegSubsample::Auto,
            };
            integer()?.encode_jpeg_options(q, mode)?
        }
        "png" => {
            let r = integer()?;
            if m.get_flag("interlace") {
                r.encode_png_interlaced()?
            } else if m.get_flag("palette") {
                let bits: u32 = pos(m, "bitdepth").parse().unwrap_or(8);
                r.encode_png_palette(1 << bits)?
            } else {
                r.encode_png(m.get_one::<u8>("compression").copied().unwrap_or(6))?
            }
        }
        "webp" => {
            // --lossless is a required argument, so clap has already refused
            // a command line without it.
            integer()?.encode_webp(
                libviprs::webp::SaveOptions::default().with_keep(libviprs::webp::Keep::None),
            )?
        }
        "gif" => {
            let options = libviprs::gif::SaveOptions::default()
                .with_dither(*m.get_one::<f64>("dither").expect("defaulted"))
                .with_bitdepth(*m.get_one::<u8>("bitdepth").expect("defaulted"))
                .with_interlaced(m.get_flag("interlace"));
            integer()?.encode_gif(options)?
        }
        "jxl" => integer()?.encode_jxl(libviprs::jxl::SaveOptions::default())?,
        "jp2k" => {
            let tile = |id: &str| {
                NonZeroU32::new(*m.get_one::<u32>(id).expect("defaulted"))
                    .ok_or_else(|| usage_err!("--{id} must be at least 1"))
            };
            let options = libviprs::jp2k::SaveOptions::default()
                .with_tile_width(tile("tile-width")?)
                .with_tile_height(tile("tile-height")?);
            integer()?.encode_jp2k(options)?
        }
        "fits" => raster.encode_fits()?,
        "rad" => raster.encode_radiance(libviprs::radiance::SaveOptions::default())?,
        "uhdr" => {
            let options = libviprs::uhdr::SaveOptions::default()
                .with_quality(*m.get_one::<u8>("Q").expect("defaulted"))
                .with_gain_map_shrink(
                    *m.get_one::<u32>("gainmap-scale-factor").expect("defaulted"),
                );
            let scrgb = to_scrgb(raster)?;
            libviprs::uhdr::encode_uhdr(&scrgb, &options)?
        }
        "csv" => raster.csv_save()?,
        "matrix" => raster.matrix_save()?,
        "ppm" => io::encode_pnm(raster)?,
        other => bail!("no encoder for {other}save"),
    })
}

/// `uhdrsave` takes linear-light scRGB. A three-band float already tagged
/// scRGB goes straight through; anything else is converted the way vips's
/// saver converts it, through the colourspace route.
fn to_scrgb(raster: &Raster) -> Result<std::borrow::Cow<'_, Raster>> {
    let f = raster.format();
    if raster.interpretation() == Interpretation::ScRgb && f.is_float() && f.channels() == 3 {
        return Ok(std::borrow::Cow::Borrowed(raster));
    }
    raster
        .try_colourspace(Interpretation::ScRgb)
        .map(std::borrow::Cow::Owned)
        .map_err(|e| anyhow!("uhdrsave needs an scRGB image and this one does not convert: {e}"))
}

fn require_encoder(compiled: bool, feature: &'static str, format: &'static str) -> Result<()> {
    if compiled {
        Ok(())
    } else {
        Err(MissingEncoder { feature, format }.into())
    }
}

fn require_decoder(compiled: bool, feature: &'static str, format: &'static str) -> Result<()> {
    if compiled {
        Ok(())
    } else {
        Err(crate::features::MissingFeature { feature, format }.into())
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Read the whole input, from stdin for `-`, refusing more than the
/// `max_alloc_bytes` budget before it is all in memory.
///
/// `ceiling` is a format's own input ceiling (SVG's 10 MB). When that is the
/// lower of the two, one byte past it is handed on so the core refuses the
/// document with its own typed error. When the budget is the lower one the
/// input is refused here: a truncated document is never handed on.
fn read_input(spec: &str, limits: &DecodeLimits, ceiling: Option<u64>) -> Result<Vec<u8>> {
    let budget = limits.max_alloc_bytes;
    let cap = ceiling.map_or(budget, |c| c.min(budget));
    let mut bytes = Vec::new();
    if spec == "-" {
        std::io::stdin()
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .context("failed to read the image from stdin")?;
    } else {
        let file = std::fs::File::open(spec).with_context(|| format!("failed to open {spec}"))?;
        // Sized from the file, so a large input is read without the doubling
        // a growing Vec does on the way, and never past the cap.
        if let Ok(meta) = file.metadata() {
            let hint = meta.len().min(cap.saturating_add(1));
            bytes.reserve(usize::try_from(hint).unwrap_or(0));
        }
        file.take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read {spec}"))?;
    }
    let over_ceiling_only = ceiling.is_some_and(|c| c < budget);
    if bytes.len() as u64 > cap && !over_ceiling_only {
        bail!("{spec} is larger than the {budget} byte decode budget (--max-alloc-bytes)");
    }
    Ok(bytes)
}

/// Refuse bytes that do not open with any of `magics`, the way vips's own
/// `*load` refuses a file of another format rather than sniffing past it.
fn expect_magic(bytes: &[u8], magics: &[&[u8]], format: &str, spec: &str) -> Result<()> {
    if magics.iter().any(|m| bytes.starts_with(m)) {
        Ok(())
    } else {
        bail!("{spec} is not a {format} file")
    }
}

fn core<E: Into<anyhow::Error>>(r: std::result::Result<Raster, E>) -> Result<Raster> {
    r.map_err(Into::into)
}

/// A core decode error, turned into the missing-feature refusal when that
/// is what it is.
fn source(r: std::result::Result<Raster, libviprs::source::SourceError>) -> Result<Raster> {
    r.map_err(|e| match crate::features::missing_feature(&e) {
        Some(missing) => missing.into(),
        None => e.into(),
    })
}

fn decode(name: &str, m: &ArgMatches) -> Result<Raster> {
    let spec = pos(m, "IN");
    let mut limits = io::decode_limits(m);
    if let Some(&pages) = m.try_get_one::<u32>(MAX_PAGES).ok().flatten() {
        limits = limits.with_max_pages(pages);
    }
    let page = m
        .try_get_one::<u32>("page")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(0);
    let n = m
        .try_get_one::<i32>("n")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(1);
    let raster = match name {
        "jpegload" => {
            let bytes = read_input(spec, &limits, None)?;
            expect_magic(&bytes, &[b"\xFF\xD8\xFF"], "JPEG", spec)?;
            let full = crate::input::decode_bytes(&bytes, limits)?;
            let shrink: u32 = pos(m, "shrink").parse().unwrap_or(1);
            if shrink > 1 {
                let s = f64::from(shrink);
                full.try_shrink(s, s)
                    .map_err(|e| anyhow!("shrink-on-load failed: {e}"))?
            } else {
                full
            }
        }
        "pngload" => {
            let bytes = read_input(spec, &limits, None)?;
            expect_magic(&bytes, &[b"\x89PNG\r\n\x1a\n"], "PNG", spec)?;
            crate::input::decode_bytes(&bytes, limits)?
        }
        "ppmload" => {
            let bytes = read_input(spec, &limits, None)?;
            expect_magic(
                &bytes,
                &[b"P1", b"P2", b"P3", b"P4", b"P5", b"P6"],
                "PBM/PGM/PPM",
                spec,
            )?;
            crate::input::decode_bytes(&bytes, limits)?
        }
        "tiffload" => {
            if spec == "-" {
                if page != 0 {
                    usage_bail!("tiffload --page needs a file: stdin gives the first page only");
                }
                let bytes = read_input(spec, &limits, None)?;
                expect_magic(&bytes, &[b"II*\0", b"MM\0*"], "TIFF", spec)?;
                core(Raster::tiff_load_with_limits(&bytes, limits))?
            } else {
                core(libviprs::decode_tiff_page_with_limits(
                    Path::new(spec),
                    page,
                    limits,
                ))?
            }
        }
        "gifload" => {
            let bytes = read_input(spec, &limits, None)?;
            let options = libviprs::gif::LoadOptions::default()
                .with_page(page)
                .with_n(n);
            source(libviprs::decode_gif_with(&bytes, limits, options))?
        }
        "webpload" => {
            let bytes = read_input(spec, &limits, None)?;
            let options = libviprs::webp::LoadOptions::default()
                .with_page(page)
                .with_n(n);
            source(libviprs::decode_webp_with(&bytes, limits, options))?
        }
        "jxlload" => {
            require_decoder(cfg!(feature = "jxl"), "jxl", "JPEG XL")?;
            source(libviprs::decode_jxl(
                &read_input(spec, &limits, None)?,
                limits,
            ))?
        }
        "jp2kload" => {
            require_decoder(cfg!(feature = "jp2k"), "jp2k", "JPEG 2000")?;
            source(libviprs::decode_jp2k(
                &read_input(spec, &limits, None)?,
                limits,
            ))?
        }
        "heifload" => {
            require_decoder(cfg!(feature = "avif"), "avif", "AVIF")?;
            let bytes = read_input(spec, &limits, None)?;
            // `ftyp` at 4 and the `avif` brand at 8: anything else in the HEIF
            // family is HEVC, which libviprs has no decoder for.
            if bytes.get(4..12) != Some(b"ftypavif".as_slice()) {
                bail!(
                    "{spec} is not an AVIF file; heifload reads AVIF only, because HEIF/HEIC \
                     (HEVC) has no decoder in libviprs"
                );
            }
            source(libviprs::decode_avif(&bytes, limits))?
        }
        "svgload" => {
            let unlimited = m.get_flag("unlimited");
            // One byte past the core's ceiling is enough for it to refuse
            // with its own typed error; --unlimited reads the whole document.
            let ceiling = (!unlimited).then_some(libviprs::svg::MAX_INPUT_BYTES as u64 + 1);
            let bytes = read_input(spec, &limits, ceiling)?;
            // The same order `input::decode_path` keeps: a gzipped document
            // is refused for what it is before the feature check, which
            // would otherwise tell a build without `svg` to rebuild with it,
            // and the renderer in a build with it would fail it as XML.
            crate::input::refuse_compressed_svg(&bytes)?;
            require_decoder(cfg!(feature = "svg"), "svg", "SVG")?;
            let options = libviprs::SvgOptions::default()
                .with_dpi(*m.get_one::<f64>("dpi").expect("defaulted"))
                .with_scale(*m.get_one::<f64>("scale").expect("defaulted"))
                .with_unlimited(unlimited);
            crate::input::decode_svg(&bytes, options, limits)?
        }
        "fitsload" => source(libviprs::decode_fits(
            &read_input(spec, &limits, None)?,
            limits,
        ))?,
        "radload" => source(libviprs::decode_radiance(
            &read_input(spec, &limits, None)?,
            limits,
        ))?,
        "openexrload" => source(libviprs::decode_exr(
            &read_input(spec, &limits, None)?,
            limits,
        ))?,
        "niftiload" => source(libviprs::decode_nifti(
            &read_input(spec, &limits, None)?,
            limits,
        ))?,
        "matload" => source(libviprs::decode_mat(
            &read_input(spec, &limits, None)?,
            limits,
        ))?,
        "uhdrload" => source(libviprs::uhdr::decode_uhdr(
            &read_input(spec, &limits, None)?,
            limits,
        ))?,
        "analyzeload" => {
            if spec == "-" {
                usage_bail!("analyzeload needs a file: an Analyze image is a .hdr/.img pair");
            }
            source(libviprs::decode_analyze_file(Path::new(spec), limits))?
        }
        "csvload" => {
            let bytes = read_input(spec, &limits, None)?;
            let (w, h) = csv_geometry(&bytes);
            check_dims(w, h, Some(CSV_GRID_COPIES), &limits)?;
            core(Raster::csv_load(&bytes))?
        }
        "matrixload" => {
            let bytes = read_input(spec, &limits, None)?;
            if let Some((w, h)) = matrix_geometry(&bytes) {
                check_dims(w, h, Some(MATRIX_GRID_COPIES), &limits)?;
            }
            core(Raster::matrix_load(&bytes))?
        }
        other => bail!("no loader called {other}"),
    };
    Ok(raster)
}

/// The grid `Raster::csv_load` will build, read the way it reads it: the
/// first non-empty row's comma- or TAB-separated field count is the width,
/// the non-empty rows are the height. Counted before the decode so a
/// declared-huge grid is refused before it is allocated.
fn csv_geometry(bytes: &[u8]) -> (u64, u64) {
    let text = String::from_utf8_lossy(bytes);
    let mut rows = text.lines().filter(|l| !l.trim().is_empty());
    let width = rows
        .next()
        .map_or(0, |l| l.split([',', '\t']).count() as u64);
    (width, 1 + rows.count() as u64)
}

/// The `width height` a text matrix declares on its first line.
fn matrix_geometry(bytes: &[u8]) -> Option<(u64, u64)> {
    let text = std::str::from_utf8(bytes.get(..bytes.len().min(256))?).ok()?;
    let mut it = text.lines().next()?.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// Copies of a `csvload` grid alive at once while `Raster::csv_load` builds
/// it: the padded row vectors, the flattened samples (a growing Vec, which can
/// sit at up to twice the grid while the rows drain into it), and the raster's
/// own buffer. The core reserves none of them fallibly or under a limit, which
/// is on the core tracking issue; until it does, this is priced here.
const CSV_GRID_COPIES: u64 = 3;

/// The same for `Raster::matrix_load`: the parsed samples (a growing Vec, so
/// up to twice the grid) and the raster's buffer.
const MATRIX_GRID_COPIES: u64 = 2;

/// Bytes per sample of the one-band float raster both text loaders build.
const F32_BYTES: u64 = 4;

/// `w` x `h` against every geometry limit, and with `copies` the float grid
/// a text loader builds (`w` x `h` x 4 bytes, `copies` times) against
/// `max_alloc_bytes`. The products saturate rather than wrap, so a header
/// claiming absurd dimensions is refused, never priced at a wrapped-around
/// small number.
fn check_dims(w: u64, h: u64, copies: Option<u64>, limits: &DecodeLimits) -> Result<()> {
    let too = |what: &str, got: u64, max: u64, flag: &str| {
        Err(anyhow!(
            "image {what} {got} exceeds the decode limit of {max} (--{flag})"
        ))
    };
    if w > u64::from(limits.max_width) {
        return too("width", w, limits.max_width.into(), io::MAX_WIDTH);
    }
    if h > u64::from(limits.max_height) {
        return too("height", h, limits.max_height.into(), io::MAX_HEIGHT);
    }
    if w.max(h) > u64::from(limits.max_coord) {
        return too("axis", w.max(h), limits.max_coord.into(), io::MAX_COORD);
    }
    let pixels = w.saturating_mul(h);
    if pixels > limits.max_pixels {
        return too("pixel count", pixels, limits.max_pixels, io::MAX_PIXELS);
    }
    if let Some(copies) = copies {
        let bytes = pixels.saturating_mul(F32_BYTES).saturating_mul(copies);
        if bytes > limits.max_alloc_bytes {
            return too(
                "byte size",
                bytes,
                limits.max_alloc_bytes,
                io::MAX_ALLOC_BYTES,
            );
        }
    }
    Ok(())
}

/// The decoded geometry against every limit the flags promise; see the module
/// docs for why this runs after the core's own checks too.
fn check_geometry(raster: &Raster, limits: &DecodeLimits) -> Result<()> {
    check_dims(raster.width().into(), raster.height().into(), None, limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_an_image_to_image_with_a_meta() {
        let names: Vec<String> = commands()
            .iter()
            .map(|c| c.get_name().to_string())
            .collect();
        let metas: Vec<&str> = metas().iter().map(|m| m.name).collect();
        assert_eq!(names, metas);
        for name in &names {
            assert!(
                name.ends_with("save") || name.ends_with("load"),
                "{name} is not spelled the way vips spells a codec"
            );
        }
    }

    #[test]
    fn every_loader_takes_the_decode_limits() {
        for cmd in commands() {
            let longs: Vec<&str> = cmd.get_arguments().filter_map(|a| a.get_long()).collect();
            for limit in [
                io::MAX_WIDTH,
                io::MAX_HEIGHT,
                io::MAX_COORD,
                io::MAX_PIXELS,
                io::MAX_ALLOC_BYTES,
            ] {
                assert!(
                    longs.contains(&limit),
                    "{} has no --{limit}",
                    cmd.get_name()
                );
            }
        }
    }

    #[test]
    fn csv_geometry_reads_the_first_row_and_counts_the_rest() {
        assert_eq!(csv_geometry(b"1\t2\t3\n4\t5\t6\n\n7,8,9\n"), (3, 3));
        assert_eq!(csv_geometry(b""), (0, 1));
    }

    #[test]
    fn matrix_geometry_reads_the_header() {
        assert_eq!(matrix_geometry(b"3 2 1 0\n1 2 3\n4 5 6\n"), Some((3, 2)));
        assert_eq!(matrix_geometry(b"nonsense\n"), None);
    }

    #[test]
    fn check_dims_names_the_flag_it_tripped() {
        let limits = DecodeLimits::default().with_max_pixels(10);
        let err = check_dims(4, 4, None, &limits).unwrap_err().to_string();
        assert!(err.contains("--max-pixels"), "{err}");
        assert!(check_dims(2, 5, None, &limits).is_ok());
    }

    #[test]
    fn check_dims_prices_the_grid_copies_without_wrapping() {
        // The ragged CSV from the review, one 65535-field row and 16383
        // one-field rows: inside every geometry default, about 4.3 GB a copy.
        let limits = DecodeLimits::default();
        assert!(check_dims(65_535, 16_384, None, &limits).is_ok());
        let err = check_dims(65_535, 16_384, Some(CSV_GRID_COPIES), &limits)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--max-alloc-bytes"), "{err}");
        // Dimensions whose byte count overflows u64 still refuse cleanly.
        let wide = DecodeLimits::default()
            .with_max_width(u32::MAX)
            .with_max_height(u32::MAX)
            .with_max_coord(u32::MAX)
            .with_max_pixels(u64::MAX)
            .with_max_alloc_bytes(u64::MAX - 1);
        let max = u64::from(u32::MAX);
        let err = check_dims(max, max, Some(CSV_GRID_COPIES), &wide)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--max-alloc-bytes"), "{err}");
    }

    fn command(name: &str) -> Command {
        commands()
            .into_iter()
            .find(|c| c.get_name() == name)
            .unwrap_or_else(|| panic!("no command {name}"))
    }

    fn parse(args: &[&str]) -> std::result::Result<ArgMatches, clap::Error> {
        command(args[0]).try_get_matches_from(args)
    }

    /// A scratch directory of its own per test, removed when it drops.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("viprs-foreign-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path.to_string_lossy().into_owned()
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A 2x2 RGB binary PPM, small enough to load through any limit.
    const PPM_2X2: &[u8] = b"P6\n2 2\n255\n\x00\x10\x20\x30\x40\x50\x60\x70\x80\x90\xa0\xb0";

    #[test]
    fn csvload_prices_the_grid_against_max_alloc_bytes_before_decoding() {
        // 20x20 floats is 1600 bytes a copy, so a 1000 byte budget cannot hold
        // even one of the copies csv_load makes, while every axis and pixel
        // limit is far away.
        let scratch = Scratch::new("csv-price");
        let row = vec!["1"; 20].join(",");
        let csv = vec![row; 20].join("\n");
        let input = scratch.file("grid.csv", csv.as_bytes());
        let out = scratch.path("grid.v");
        let m = parse(&["csvload", &input, &out, "--max-alloc-bytes", "1000"]).unwrap();
        let err = run("csvload", &m).expect_err("a grid over budget loaded");
        let err = format!("{err:#}");
        assert!(err.contains("--max-alloc-bytes"), "{err}");
    }

    #[test]
    fn csvload_prices_a_ragged_grid_at_the_padded_width() {
        // One wide first row and short rows after it: csv_load pads every row
        // to the first one's width, so the grid is 400 wide however few bytes
        // the short rows take.
        let scratch = Scratch::new("csv-ragged");
        let mut csv = vec!["0"; 400].join(",");
        csv.push_str(&"\n1".repeat(50));
        let input = scratch.file("ragged.csv", csv.as_bytes());
        let out = scratch.path("ragged.v");
        let m = parse(&["csvload", &input, &out, "--max-alloc-bytes", "100000"]).unwrap();
        let err = format!("{:#}", run("csvload", &m).expect_err("ragged bomb loaded"));
        assert!(err.contains("--max-alloc-bytes"), "{err}");
    }

    #[test]
    fn matrixload_prices_the_declared_grid_against_max_alloc_bytes() {
        let scratch = Scratch::new("matrix-price");
        let mut text = String::from("20 20\n");
        for _ in 0..20 {
            text.push_str(&vec!["1"; 20].join(" "));
            text.push('\n');
        }
        let input = scratch.file("grid.mat.txt", text.as_bytes());
        let out = scratch.path("grid.v");
        let m = parse(&["matrixload", &input, &out, "--max-alloc-bytes", "1000"]).unwrap();
        let err = format!("{:#}", run("matrixload", &m).expect_err("over budget"));
        assert!(err.contains("--max-alloc-bytes"), "{err}");
    }

    #[test]
    fn svg_input_over_the_alloc_budget_is_refused_not_truncated() {
        // Under the 10 MB SVG ceiling but over a 100 byte budget: the reader
        // used to stop at the budget and hand the parser half a document.
        let scratch = Scratch::new("svg-budget");
        let doc = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\">{}</svg>",
            "<rect width=\"1\" height=\"1\"/>".repeat(20)
        );
        let input = scratch.file("doc.svg", doc.as_bytes());
        let limits = DecodeLimits::default().with_max_alloc_bytes(100);
        let ceiling = Some(libviprs::svg::MAX_INPUT_BYTES as u64 + 1);
        let err = read_input(&input, &limits, ceiling)
            .expect_err("a document over budget was read in part")
            .to_string();
        assert!(err.contains("--max-alloc-bytes"), "{err}");
    }

    #[test]
    fn svg_input_under_both_ceilings_is_read_whole() {
        let scratch = Scratch::new("svg-whole");
        let input = scratch.file("doc.svg", &[b'x'; 300]);
        let limits = DecodeLimits::default().with_max_alloc_bytes(1000);
        let ceiling = Some(libviprs::svg::MAX_INPUT_BYTES as u64 + 1);
        assert_eq!(read_input(&input, &limits, ceiling).unwrap().len(), 300);
    }

    #[test]
    fn no_command_takes_stdout_as_its_output() {
        for cmd in commands() {
            let name = cmd.get_name().to_string();
            let mut args = vec![name.clone(), "in.png".into(), "-".into()];
            if name == "webpsave" || name == "jxlsave" || name == "jp2ksave" {
                args.push("--lossless".into());
            }
            let err = cmd
                .try_get_matches_from(&args)
                .expect_err(&format!("{name} took - as OUT"));
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn loaders_still_take_stdin_as_their_input() {
        assert!(parse(&["pngload", "-", "out.png"]).is_ok());
    }

    #[test]
    fn lossless_only_savers_need_the_flag_to_parse() {
        for name in ["webpsave", "jxlsave", "jp2ksave"] {
            let err = parse(&[name, "a.png", "b.out"]).expect_err(name);
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "{name}: {err}"
            );
            assert!(err.to_string().contains("--lossless"), "{name}: {err}");
            assert!(parse(&[name, "a.png", "b.out", "--lossless"]).is_ok());
        }
    }

    #[test]
    fn svgload_refuses_a_dpi_or_scale_that_is_not_a_finite_positive_number() {
        for bad in ["NaN", "inf", "-inf", "-1", "0"] {
            for flag in ["--dpi", "--scale"] {
                let arg = format!("{flag}={bad}");
                let err = parse(&["svgload", "a.svg", "b.png", &arg])
                    .expect_err(&format!("svgload took {arg}"));
                assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "{arg}");
            }
        }
        assert!(parse(&["svgload", "a.svg", "b.png", "--dpi=144", "--scale=0.5"]).is_ok());
    }

    #[test]
    fn gifsave_dither_is_held_to_its_advertised_range() {
        for bad in ["1.5", "-0.1", "NaN", "inf"] {
            let arg = format!("--dither={bad}");
            let err = parse(&["gifsave", "a.png", "b.gif", &arg])
                .expect_err(&format!("gifsave took {arg}"));
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "{arg}");
        }
        for good in ["0", "0.5", "1"] {
            assert!(parse(&["gifsave", "a.png", "b.gif", &format!("--dither={good}")]).is_ok());
        }
    }

    #[cfg(unix)]
    #[test]
    fn tiffsave_never_writes_through_a_name_beside_the_output() {
        // The old route wrote `.OUT.<pid>.viprs-tmp` with File::create, which
        // follows a symlink planted at that name. Plant one and check the file
        // it points at comes through untouched.
        let scratch = Scratch::new("tiff-symlink");
        let input = scratch.file("in.ppm", PPM_2X2);
        let victim = scratch.file("victim.txt", b"keep me");
        let out = scratch.path("out.tif");
        let planted = scratch
            .0
            .join(format!(".out.tif.{}.viprs-tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        let m = parse(&["tiffsave", &input, &out]).unwrap();
        run("tiffsave", &m).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        let written = std::fs::read(&out).unwrap();
        assert!(written.starts_with(b"II*\0") || written.starts_with(b"MM\0*"));
    }

    #[cfg(unix)]
    #[test]
    fn tiffsave_leaves_nothing_but_its_output_beside_it() {
        let scratch = Scratch::new("tiff-clean");
        let input = scratch.file("in.ppm", PPM_2X2);
        let out = scratch.path("out.tif");
        let m = parse(&["tiffsave", &input, &out, "--compression", "lzw"]).unwrap();
        run("tiffsave", &m).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&scratch.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["in.ppm", "out.tif"]);
    }

    #[cfg(not(feature = "jxl"))]
    #[test]
    fn a_missing_encoder_is_reported_before_the_input_is_read() {
        let scratch = Scratch::new("enc-first");
        let missing = scratch.path("not-there.png");
        let out = scratch.path("out.jxl");
        let m = parse(&["jxlsave", &missing, &out, "--lossless"]).unwrap();
        let err = run("jxlsave", &m).expect_err("jxlsave ran without the jxl feature");
        assert!(
            err.downcast_ref::<MissingEncoder>().is_some(),
            "the input was read first: {err:#}"
        );
    }

    /// A real gzip stream around `data`: one stored deflate block, so no
    /// compressor is needed, with the CRC-32 and length trailer a gunzip
    /// checks. A `.svgz` in the wild is exactly this shape with a compressed
    /// block instead of a stored one.
    fn gzip_stored(data: &[u8]) -> Vec<u8> {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        let crc = !crc;
        let len = u16::try_from(data.len()).expect("fixture fits one stored block");
        let mut out = vec![0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 0x03];
        out.push(0x01);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        out
    }

    const TINY_SVG: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg' width='3' height='2'>\
        <rect width='3' height='2' fill='red'/></svg>";

    /// `svgload` on a gzipped document gets the same refusal every other
    /// command gets from `input::decode_path` (libviprs-cli#64): the renderer
    /// has no gzip support, so say that, in a build with or without `svg`.
    /// Rebuilding with `svg` would not help, so a build without it must not
    /// send the person off to do that.
    #[test]
    fn svgload_refuses_a_gzipped_svgz_the_way_every_loader_does() {
        let scratch = Scratch::new("svgz-svgload");
        let gz = gzip_stored(TINY_SVG);
        for name in ["in.svgz", "in.svg"] {
            let input = scratch.file(name, &gz);
            let out = scratch.path("out.png");
            let m = parse(&["svgload", &input, &out]).unwrap();
            let err = run("svgload", &m).expect_err(&format!("svgload decoded a gzipped {name}"));
            assert!(
                err.downcast_ref::<crate::input::CompressedSvg>().is_some(),
                "{name}: expected the .svgz refusal, got {err:#}"
            );
            assert!(
                !std::path::Path::new(&out).exists(),
                "{name}: a refused load wrote {out}"
            );
        }
    }

    /// The fixture is a real gzip of a document that renders, so the refusal
    /// above is about the gzip and nothing else.
    #[cfg(feature = "svg")]
    #[test]
    fn the_svgz_fixture_holds_a_document_that_renders() {
        let scratch = Scratch::new("svgz-plain");
        let input = scratch.file("in.svg", TINY_SVG);
        let out = scratch.path("out.png");
        let m = parse(&["svgload", &input, &out]).unwrap();
        run("svgload", &m).unwrap();
        assert!(std::path::Path::new(&out).exists());
        let gz = gzip_stored(TINY_SVG);
        assert_eq!(&gz[..2], b"\x1f\x8b");
        assert_eq!(&gz[15..15 + TINY_SVG.len()], TINY_SVG);
    }
}
