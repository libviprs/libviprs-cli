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
//! input reads the image from stdin. The core bounds allocation with
//! `max_coord`, `max_pixels` and `max_alloc_bytes` before it reserves a frame,
//! but only its `image`-crate paths look at `max_width` / `max_height`, so the
//! loaders here check the decoded geometry against all of them afterwards
//! as well; a flag the help text promises is a flag every loader honours.
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
//! change of meaning, so each of them requires `--lossless` and refuses
//! without it. The extension route (`viprs copy in.png out.webp`) is the
//! core's own `Raster::save` table and writes lossless, as it always has.

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
    cmd.arg(Arg::new("IN").required(true).help(input))
        .arg(Arg::new("OUT").required(true).help("Output image"))
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
                    .help("Deflate level for a plain PNG (default 6)"),
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
                    .help("With --palette, hold at most 2^N colours (default 8)"),
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
                    .value_parser(value_parser!(f64))
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
        loader("jpegload", "Load a JPEG image.").arg(
            Arg::new("shrink")
                .long("shrink")
                .value_parser(["1", "2", "4", "8"])
                .default_value("1")
                .help("Shrink by this integer factor while loading"),
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
                .value_parser(value_parser!(f64))
                .default_value("72")
                .help("Render at this DPI"),
        )
        .arg(
            Arg::new("scale")
                .long("scale")
                .value_parser(value_parser!(f64))
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
        let raster = io::load(Path::new(pos(m, "IN")), &io::decode_limits(m))?;
        let bytes = encode(format, &raster, m)?;
        return std::fs::write(&out, bytes)
            .with_context(|| format!("failed to write {}", out.display()));
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
        "tiff" => {
            let compression = match pos(m, "compression") {
                "lzw" => TiffCompression::Lzw,
                "deflate" => TiffCompression::Deflate,
                _ => TiffCompression::None,
            };
            // `save_tiff` is the core's one compression-choosing entry point
            // and it writes a path, so the bytes go through a sibling temp
            // file rather than a second copy of the encoder.
            let tmp = tempfile_beside(Path::new(pos(m, "OUT")))?;
            let result = integer()?.save_tiff(&tmp, compression);
            let bytes = result.map_err(anyhow::Error::from).and_then(|()| {
                std::fs::read(&tmp).with_context(|| format!("failed to read {}", tmp.display()))
            });
            let _ = std::fs::remove_file(&tmp);
            bytes?
        }
        "webp" => {
            require_lossless(m, "webpsave")?;
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
        "jxl" => {
            require_encoder(cfg!(feature = "jxl"), "jxl", "JPEG XL")?;
            require_lossless(m, "jxlsave")?;
            integer()?.encode_jxl(libviprs::jxl::SaveOptions::default())?
        }
        "jp2k" => {
            require_encoder(cfg!(feature = "jp2k"), "jp2k", "JPEG 2000")?;
            require_lossless(m, "jp2ksave")?;
            let tile = |id: &str| {
                NonZeroU32::new(*m.get_one::<u32>(id).expect("defaulted"))
                    .ok_or_else(|| anyhow!("--{id} must be at least 1"))
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

fn require_lossless(m: &ArgMatches, command: &str) -> Result<()> {
    if m.get_flag("lossless") {
        return Ok(());
    }
    bail!(
        "{command} writes lossless only, and vips's default for it is lossy, so the mode \
         has to be asked for: pass --lossless"
    )
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

/// A path next to `out` that nothing else is using, for an encoder that only
/// writes files.
fn tempfile_beside(out: &Path) -> Result<PathBuf> {
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(dir.join(format!(".{name}.{}.viprs-tmp", std::process::id())))
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Read the whole input, from stdin for `-`, refusing more than the
/// `max_alloc_bytes` budget before it is all in memory.
fn read_input(spec: &str, limits: &DecodeLimits, ceiling: Option<u64>) -> Result<Vec<u8>> {
    let cap = ceiling
        .unwrap_or(limits.max_alloc_bytes)
        .min(limits.max_alloc_bytes);
    let mut bytes = Vec::new();
    if spec == "-" {
        std::io::stdin()
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .context("failed to read the image from stdin")?;
    } else {
        std::fs::File::open(spec)
            .with_context(|| format!("failed to open {spec}"))?
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read {spec}"))?;
    }
    if bytes.len() as u64 > cap && ceiling.is_none() {
        bail!(
            "{spec} is larger than the {} byte decode budget (--max-alloc-bytes)",
            limits.max_alloc_bytes
        );
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
                    bail!("tiffload --page needs a file: stdin gives the first page only");
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
                bail!("analyzeload needs a file: an Analyze image is a .hdr/.img pair");
            }
            source(libviprs::decode_analyze_file(Path::new(spec), limits))?
        }
        "csvload" => {
            let bytes = read_input(spec, &limits, None)?;
            let (w, h) = csv_geometry(&bytes);
            check_dims(w, h, &limits)?;
            core(Raster::csv_load(&bytes))?
        }
        "matrixload" => {
            let bytes = read_input(spec, &limits, None)?;
            if let Some((w, h)) = matrix_geometry(&bytes) {
                check_dims(w, h, &limits)?;
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

fn check_dims(w: u64, h: u64, limits: &DecodeLimits) -> Result<()> {
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
    if w * h > limits.max_pixels {
        return too("pixel count", w * h, limits.max_pixels, io::MAX_PIXELS);
    }
    Ok(())
}

/// The decoded geometry against every limit the flags promise; see the module
/// docs for why this runs after the core's own checks too.
fn check_geometry(raster: &Raster, limits: &DecodeLimits) -> Result<()> {
    check_dims(raster.width().into(), raster.height().into(), limits)
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
        let err = check_dims(4, 4, &limits).unwrap_err().to_string();
        assert!(err.contains("--max-pixels"), "{err}");
        assert!(check_dims(2, 5, &limits).is_ok());
    }

    #[test]
    fn lossless_only_savers_refuse_without_the_flag() {
        let cmd = commands()
            .into_iter()
            .find(|c| c.get_name() == "webpsave")
            .unwrap();
        let m = cmd
            .try_get_matches_from(["webpsave", "a.png", "b.webp"])
            .unwrap();
        let err = require_lossless(&m, "webpsave").unwrap_err().to_string();
        assert!(err.contains("--lossless"), "{err}");
    }
}
