//! Shared decode/encode harness for the op families (`CLI_CONTRACT.md` §2).
//!
//! Every op handler loads its inputs through [`load`] and writes its outputs
//! through [`save`] so the decode-limit surface and the cast-on-save parity
//! rules live in exactly one place. `load` wires all five
//! [`DecodeLimits`](libviprs::source::DecodeLimits) fields as reusable
//! `--max-*` flags; `save` performs the round-half-to-even cast-on-save and
//! routes float / multiband / Fourier rasters to the native `.v` container
//! while integer rasters go to `.png`. `.jpg` is banned as a differential
//! sink.
//!
//! Only the panic-free `try_*` / fallible core APIs are called here so a bad
//! input becomes a typed error (exit 1) rather than a process abort
//! (`CLI_CONTRACT.md` §8).

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::PixelFormat;
use libviprs::Raster;
use libviprs::source::{DecodeLimits, decode_file_with_limits};

/// Long name of the `--max-width` decode-limit flag.
pub const MAX_WIDTH: &str = "max-width";
/// Long name of the `--max-height` decode-limit flag.
pub const MAX_HEIGHT: &str = "max-height";
/// Long name of the `--max-coord` decode-limit flag.
pub const MAX_COORD: &str = "max-coord";
/// Long name of the `--max-pixels` decode-limit flag.
pub const MAX_PIXELS: &str = "max-pixels";
/// Long name of the `--max-alloc-bytes` decode-limit flag.
pub const MAX_ALLOC_BYTES: &str = "max-alloc-bytes";

/// Append the five [`DecodeLimits`](libviprs::source::DecodeLimits) flags to a
/// command that decodes an image input.
///
/// Every image-loading op command funnels through this so the decode-limit
/// surface is identical across families and appears verbatim in
/// `__dump-commands` (`CLI_CONTRACT.md` §2, campaign #422-432).
pub fn with_decode_limit_args(cmd: Command) -> Command {
    cmd.arg(
        Arg::new(MAX_WIDTH)
            .long(MAX_WIDTH)
            .value_name("PX")
            .value_parser(value_parser!(u32))
            .help("Reject inputs wider than PX pixels (DecodeLimits::max_width)"),
    )
    .arg(
        Arg::new(MAX_HEIGHT)
            .long(MAX_HEIGHT)
            .value_name("PX")
            .value_parser(value_parser!(u32))
            .help("Reject inputs taller than PX pixels (DecodeLimits::max_height)"),
    )
    .arg(
        Arg::new(MAX_COORD)
            .long(MAX_COORD)
            .value_name("PX")
            .value_parser(value_parser!(u32))
            .help("Reject a single axis larger than PX pixels (DecodeLimits::max_coord)"),
    )
    .arg(
        Arg::new(MAX_PIXELS)
            .long(MAX_PIXELS)
            .value_name("N")
            .value_parser(value_parser!(u64))
            .help("Reject inputs with more than N total pixels (DecodeLimits::max_pixels)"),
    )
    .arg(
        Arg::new(MAX_ALLOC_BYTES)
            .long(MAX_ALLOC_BYTES)
            .value_name("BYTES")
            .value_parser(value_parser!(u64))
            .help("Reject a decode allocation larger than BYTES (DecodeLimits::max_alloc_bytes)"),
    )
}

/// Build a [`DecodeLimits`](libviprs::source::DecodeLimits) from the shared
/// `--max-*` flags.
///
/// Any flag the caller did not pass keeps the [`DecodeLimits::default`] ceiling
/// for that field. Robust against being called on a command that did not
/// register the flags (an absent arg id yields `None` rather than a panic), so
/// families that do not load an image can still share this helper.
pub fn decode_limits(m: &ArgMatches) -> DecodeLimits {
    let mut limits = DecodeLimits::default();
    if let Some(&v) = m.try_get_one::<u32>(MAX_WIDTH).ok().flatten() {
        limits = limits.with_max_width(v);
    }
    if let Some(&v) = m.try_get_one::<u32>(MAX_HEIGHT).ok().flatten() {
        limits = limits.with_max_height(v);
    }
    if let Some(&v) = m.try_get_one::<u32>(MAX_COORD).ok().flatten() {
        limits = limits.with_max_coord(v);
    }
    if let Some(&v) = m.try_get_one::<u64>(MAX_PIXELS).ok().flatten() {
        limits = limits.with_max_pixels(v);
    }
    if let Some(&v) = m.try_get_one::<u64>(MAX_ALLOC_BYTES).ok().flatten() {
        limits = limits.with_max_alloc_bytes(v);
    }
    limits
}

/// Decode an image file under the supplied per-decode limits.
///
/// Native `.v`, PNG, JPEG, TIFF and the other formats the core decoder
/// understands all route through [`decode_file_with_limits`]; the limits are
/// pushed down before any pixel buffer is allocated.
///
/// # Errors
///
/// Propagates the core decode error (missing file, unsupported format, a limit
/// exceeded) as an [`anyhow::Error`] carrying the input path for context.
pub fn load(path: &Path, limits: &DecodeLimits) -> Result<Raster> {
    decode_file_with_limits(path, *limits)
        .with_context(|| format!("failed to load image {}", path.display()))
}

/// Encode a raster to `path`, choosing the sink by extension and applying the
/// cast-on-save parity rules (`CLI_CONTRACT.md` §2).
///
/// * `.jpg` / `.jpeg` — **banned** as a differential sink (lossy); returns an
///   error with a clear message.
/// * `.v` / `.vips` — native container; carries any format (float, multiband,
///   Fourier) losslessly.
/// * `.png` — integer sink. A float raster is cast to 8-bit with
///   **round-half-to-even then clip** to `0..=255`; 8-/16-bit integer rasters
///   pass straight through (16-bit passthrough preserved).
///
/// # Errors
///
/// Returns an error for a banned or unsupported extension, for a multiband /
/// float raster PNG cannot carry, or on encode / write failure.
pub fn save(raster: &Raster, path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => bail!(
            ".jpg/.jpeg is banned as a differential output sink (lossy encoding). \
             Use .png for integer rasters, or .v for float / multiband / Fourier rasters."
        ),
        "v" | "vips" => {
            let bytes = raster
                .encode_vips()
                .context("failed to encode .v container")?;
            std::fs::write(path, bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(())
        }
        "png" => {
            let cast;
            let to_encode = if raster.format().is_float() {
                cast = cast_float_to_uchar_round_even(raster)?;
                &cast
            } else {
                raster
            };
            let bytes = libviprs::sink::encode_png(to_encode).with_context(|| {
                format!(
                    "failed to PNG-encode a {:?} raster; float / multiband / Fourier rasters \
                     must be written to a .v sink",
                    to_encode.format()
                )
            })?;
            std::fs::write(path, bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(())
        }
        "" => bail!(
            "output path {} has no extension; use .png (integer) or .v \
             (float / multiband / Fourier)",
            path.display()
        ),
        other => bail!(
            "unsupported output extension .{other}; differential sinks are .png (integer) \
             and .v (float / multiband / Fourier)"
        ),
    }
}

/// Cast a float raster to its 8-bit integer counterpart with vips cast-on-save
/// semantics: **round-half-to-even** then clip to `0..=255`.
///
/// Rust's `f32::round` rounds half **away from zero** (`2.5 -> 3`), which does
/// not match vips; [`f64::round_ties_even`] gives the banker's rounding the
/// `getpoint` `.5` fixtures pin (`1.5 -> 1`, `2.5 -> 2`).
fn cast_float_to_uchar_round_even(raster: &Raster) -> Result<Raster> {
    let fmt = raster.format();
    debug_assert!(fmt.is_float(), "caller guarantees a float raster");
    let channels = fmt.channels();
    let target = PixelFormat::with_channels(channels, 1)
        .ok_or_else(|| anyhow!("cannot build an 8-bit format for {channels} channels"))?;

    let (w, h) = (raster.width() as usize, raster.height() as usize);
    let samples_per_row = w * channels;
    let src = raster.data();
    let src_stride = raster.stride();

    let mut out = Raster::zeroed(raster.width(), raster.height(), target)?;
    let out_stride = out.stride();
    let out_data = out.data_mut();

    for y in 0..h {
        for s in 0..samples_per_row {
            let soff = y * src_stride + s * 4;
            let v = f32::from_ne_bytes([src[soff], src[soff + 1], src[soff + 2], src[soff + 3]]);
            out_data[y * out_stride + s] = (v as f64).round_ties_even().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpg_sink_is_banned() {
        let r = Raster::zeroed(2, 2, PixelFormat::Gray8).unwrap();
        let err = save(&r, Path::new("out.jpg")).unwrap_err();
        assert!(err.to_string().contains("banned"), "got: {err}");
        let err2 = save(&r, Path::new("out.jpeg")).unwrap_err();
        assert!(err2.to_string().contains("banned"), "got: {err2}");
    }

    #[test]
    fn no_extension_is_rejected() {
        let r = Raster::zeroed(2, 2, PixelFormat::Gray8).unwrap();
        assert!(
            save(&r, Path::new("out"))
                .unwrap_err()
                .to_string()
                .contains("no extension")
        );
    }

    #[test]
    fn cast_on_save_rounds_half_to_even() {
        // A single-band float raster of 0.5, 1.5, 2.5, 3.5 must cast to
        // 0, 2, 2, 4 under round-half-to-even (not 1, 2, 3, 4).
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(4, 1, fmt, &[0.5, 1.5, 2.5, 3.5]).unwrap();
        let cast = cast_float_to_uchar_round_even(&r).unwrap();
        assert_eq!(cast.format(), PixelFormat::Gray8);
        assert_eq!(cast.data(), &[0, 2, 2, 4]);
    }

    #[test]
    fn cast_on_save_clips_to_range() {
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(3, 1, fmt, &[-10.0, 128.4, 999.0]).unwrap();
        let cast = cast_float_to_uchar_round_even(&r).unwrap();
        assert_eq!(cast.data(), &[0, 128, 255]);
    }
}
