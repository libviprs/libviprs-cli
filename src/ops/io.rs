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
//! **Interpretation-aware save (libviprs-cli #36).** An integer sink whose
//! raster carries a non-RGB colour space (`lab`, `xyz`, `scrgb`, …) is
//! converted to sRGB via the core colourspace route before encoding — exactly
//! as vips's foreign savers do — rather than casting the raw colour channels to
//! garbage; see [`to_integer_encodable`]. Non-displayable rasters that must be
//! kept losslessly go to a `.v` sink instead.
//!
//! Only the panic-free `try_*` / fallible core APIs are called here so a bad
//! input becomes a typed error (exit 1) rather than a process abort
//! (`CLI_CONTRACT.md` §8).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::Interpretation;
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

/// **THE S2 idiom** (`CLI_CONTRACT.md` §3.2, the N-image→image / variadic
/// shape): split a single trailing multi-value positional into its inputs and
/// its output path.
///
/// clap 4.5 makes the naive two-positional encoding (`A B [C…]` variadic
/// *followed by* a separate `OUT`) illegal: a `num_args(1..)`/`num_args(2..)`
/// positional is greedy and there is no unambiguous place for a second trailing
/// positional to begin, so clap rejects the command at build time. The legal —
/// and canonical — encoding is therefore **one** trailing positional declared
/// `num_args(2..)` (at least two values: one or more inputs plus the output).
/// This helper reproduces the vips `<op> A B [C…] OUT` order by peeling the
/// **last** collected value off as `OUT` and returning the rest, in order, as
/// the inputs.
///
/// Every variadic family reuses this: `bands` (`bandjoin`, `bandrank`) is the
/// first; later N-image→image commands (`arithmetic add`, `conversion
/// arrayjoin`, …) declare the identical positional and call this rather than
/// re-deriving the split.
///
/// # Precondition
///
/// `id` must name a positional that was registered on the command as a
/// **`String`-typed `num_args(2..)`** argument (the S2 encoding above). A caller
/// that passes an unregistered id — or one registered with a different value
/// type — gets a typed error (via [`ArgMatches::try_get_many`]) rather than the
/// clap downcast **panic** `get_many` would raise, so a wiring mistake in one of
/// the 14 families that reuse this idiom surfaces as exit 1, not an abort
/// (`CLI_CONTRACT.md` §8).
///
/// # Errors
///
/// Errors if `id` is not a registered `String` positional, or if fewer than two
/// values are present. `num_args(2..)` already enforces the count at parse time,
/// so the count guard only catches a caller that wired a looser positional (and
/// keeps the split total, never panicking on an empty slice).
pub fn inputs_and_out(m: &ArgMatches, id: &str) -> Result<(Vec<PathBuf>, PathBuf)> {
    let mut vals: Vec<PathBuf> = m
        .try_get_many::<String>(id)
        .map_err(|e| {
            anyhow!(
                "internal error: {id:?} is not a registered String num_args(2..) \
                 positional ({e})"
            )
        })?
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .collect();
    if vals.len() < 2 {
        bail!(
            "the {id} argument needs at least two values (one or more inputs \
             followed by the output path), got {}",
            vals.len()
        );
    }
    let out = vals.pop().expect("length checked to be >= 2 above");
    Ok((vals, out))
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
/// * `.tif` / `.tiff` — integer sink via core `tiff_save`, same
///   float-cast-then-encode path as `.png` (later waves — byteswap/autorot/
///   16-bit — need it, `CLI_CONTRACT.md` §2).
/// * `.ppm` — **deferred**: core ships no PPM/PNM encoder yet, so a clear
///   typed error points at `.png` / `.tif` (`CLI_CONTRACT.md` §2 names `.ppm`
///   but there is nothing to call).
///
/// # Errors
///
/// Returns an error for a banned, deferred, or unsupported extension, for a
/// multiband / float raster the integer sinks cannot carry, or on encode /
/// write failure.
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
            let prepared = to_integer_encodable(raster)?;
            let to_encode = prepared.as_ref();
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
        "tif" | "tiff" => {
            let prepared = to_integer_encodable(raster)?;
            let to_encode = prepared.as_ref();
            // `Raster::tiff_save` is infallible and returns an EMPTY buffer for
            // a raster with no TIFF form (float / multiband); surface that as a
            // typed error pointing at `.v` rather than writing a 0-byte file.
            let bytes = to_encode.tiff_save();
            if bytes.is_empty() {
                bail!(
                    "failed to TIFF-encode a {:?} raster; float / multiband / Fourier rasters \
                     must be written to a .v sink",
                    to_encode.format()
                );
            }
            std::fs::write(path, bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(())
        }
        "ppm" => bail!(
            "PPM/PNM output is not implemented yet (deferred: the core crate ships no PPM \
             encoder). Use .png or .tif for integer rasters, or .v for float / multiband / \
             Fourier rasters."
        ),
        "" => bail!(
            "output path {} has no extension; use .png / .tif (integer) or .v \
             (float / multiband / Fourier)",
            path.display()
        ),
        other => bail!(
            "unsupported output extension .{other}; differential sinks are .png / .tif \
             (integer) and .v (float / multiband / Fourier)"
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
    // A Fourier / complex raster is non-displayable: casting its float bands to
    // 8-bit produces garbage, so refuse it (it belongs in a `.v` sink) rather
    // than silently emitting an 8-bit approximation (`CLI_CONTRACT.md` §2/§8).
    if raster.interpretation() == Interpretation::Fourier {
        bail!(
            "a Fourier / complex raster is not displayable and must be written to a .v sink, \
             not cast to an 8-bit integer image"
        );
    }
    // Float samples are 4-byte native-endian f32; the per-channel reader below
    // depends on that width.
    debug_assert_eq!(
        fmt.bytes_per_channel(),
        4,
        "float rasters must carry 4-byte f32 samples"
    );
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

/// Whether an interpretation is a **non-displayable colour space** that must be
/// converted to sRGB before an integer sink can carry it (`CLI_CONTRACT.md` §2,
/// libviprs-cli #36).
///
/// The device / already-integer interpretations — `srgb`, plain `rgb`, `rgb16`,
/// `b-w`, `grey16`, plus the tag-only `multiband` / `matrix` / `histogram` /
/// `labq` — encode straight to PNG/TIFF the way vips writes them. Every real
/// colour space (`lab`, `xyz`, `scrgb`, `lch`, `cmc`, `labs`, `yxy`, `oklab`,
/// `oklch`, `cmyk`, `hsv`) is not directly displayable: vips's foreign savers
/// run `vips_colourspace(…, sRGB)` before an integer encode, and so must we, or
/// the raw channels (Lab's signed `a`/`b`, XYZ's 0..100 range, …) would be
/// cast to garbage.
fn needs_srgb_conversion(interp: Interpretation) -> bool {
    matches!(
        interp,
        Interpretation::Lab
            | Interpretation::Xyz
            | Interpretation::ScRgb
            | Interpretation::Lch
            | Interpretation::Cmc
            | Interpretation::Labs
            | Interpretation::Yxy
            | Interpretation::OkLab
            | Interpretation::OkLch
            | Interpretation::Cmyk
            | Interpretation::Hsv
    )
}

/// Prepare a raster for an **integer sink** (`.png` / `.tif`), applying the
/// `CLI_CONTRACT.md` §2 cast-on-save parity rules (libviprs-cli #36):
///
/// 1. A **non-RGB colour space** (Lab/XYZ/scRGB/…) is converted to sRGB via the
///    core colourspace route exactly as vips would before an integer encode —
///    NOT cast channel-for-channel — so `viprs colourspace in.png out.png lab`
///    writes the same PNG pixels vips does (a round trip back through sRGB).
/// 2. Any other **float** raster (e.g. a plain `b-w` ΔE float, or an
///    already-sRGB-tagged float) is cast to 8-bit with round-half-to-even then
///    clip.
/// 3. An **integer** raster with a device interpretation passes straight
///    through (16-bit passthrough preserved).
///
/// Float / non-displayable rasters the caller would rather keep losslessly
/// belong in a `.v` sink; this path is only reached once an integer sink was
/// explicitly requested.
fn to_integer_encodable(raster: &Raster) -> Result<std::borrow::Cow<'_, Raster>> {
    use std::borrow::Cow;
    if needs_srgb_conversion(raster.interpretation()) {
        // Interpretation-aware conversion (#36): a Fourier / complex raster is
        // still refused by the float caster below, but a genuine colour space
        // converts to sRGB the way vips's savers do.
        let srgb = raster.try_colourspace(Interpretation::Srgb).map_err(|e| {
            anyhow!(
                "interpretation-aware save: cannot convert a {:?} raster to sRGB for an \
                 integer sink ({e}); write it to a .v sink to keep the raw colour data",
                raster.interpretation()
            )
        })?;
        Ok(Cow::Owned(srgb))
    } else if raster.format().is_float() {
        Ok(Cow::Owned(cast_float_to_uchar_round_even(raster)?))
    } else {
        Ok(Cow::Borrowed(raster))
    }
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
    fn tif_sink_writes_an_integer_raster() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("viprs_io_tif_{}.tif", std::process::id()));
        let r = Raster::zeroed(3, 2, PixelFormat::Gray8).unwrap();
        save(&r, &path).expect("a Gray8 raster must TIFF-encode");
        let meta = std::fs::metadata(&path).expect("the .tif file must exist");
        assert!(meta.len() > 0, "the .tif file must be non-empty");
        let _ = std::fs::remove_file(&path);

        // .tiff is the same integer sink.
        let path2 = dir.join(format!("viprs_io_tiff_{}.tiff", std::process::id()));
        save(&r, &path2).expect(".tiff must also encode");
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn ppm_sink_is_deferred() {
        let r = Raster::zeroed(2, 2, PixelFormat::Gray8).unwrap();
        let err = save(&r, Path::new("out.ppm")).unwrap_err().to_string();
        assert!(err.contains("deferred"), "got: {err}");
    }

    #[test]
    fn fourier_raster_is_rejected_by_the_caster() {
        // A float raster tagged Fourier must refuse the 8-bit cast.
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(2, 1, fmt, &[0.0, 1.0])
            .unwrap()
            .copy()
            .interpretation(Interpretation::Fourier)
            .build();
        let err = cast_float_to_uchar_round_even(&r).unwrap_err().to_string();
        assert!(err.contains("Fourier"), "got: {err}");
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
    fn inputs_and_out_splits_last_positional_as_out() {
        // The S2 idiom: one trailing `num_args(2..)` positional, split into
        // (inputs, out) with the LAST value peeled off as the output.
        let m = Command::new("bandjoin")
            .arg(
                Arg::new("INPUTS")
                    .required(true)
                    .num_args(2..)
                    .value_name("A B [C…] OUT"),
            )
            .try_get_matches_from(["bandjoin", "a.png", "b.png", "c.png", "out.png"])
            .unwrap();
        let (inputs, out) = inputs_and_out(&m, "INPUTS").unwrap();
        assert_eq!(
            inputs,
            vec![
                PathBuf::from("a.png"),
                PathBuf::from("b.png"),
                PathBuf::from("c.png"),
            ]
        );
        assert_eq!(out, PathBuf::from("out.png"));
    }

    #[test]
    fn inputs_and_out_minimum_two_values() {
        // The minimum legal S2 invocation: one input + the output.
        let m = Command::new("bandjoin")
            .arg(Arg::new("INPUTS").required(true).num_args(2..))
            .try_get_matches_from(["bandjoin", "in.png", "out.png"])
            .unwrap();
        let (inputs, out) = inputs_and_out(&m, "INPUTS").unwrap();
        assert_eq!(inputs, vec![PathBuf::from("in.png")]);
        assert_eq!(out, PathBuf::from("out.png"));
    }

    #[test]
    fn integer_sink_converts_a_non_rgb_colour_space_to_srgb() {
        // #36: a Lab float raster written to an integer sink is colourspace
        // -converted to 8-bit sRGB (3-band uchar) the way vips would, NOT cast
        // channel-for-channel (which would garble Lab's 0..100 L and signed a/b).
        let fmt = PixelFormat::with_channels(3, 4).unwrap();
        let lab = Raster::from_f32_samples(1, 1, fmt, &[50.0, 20.0, -30.0])
            .unwrap()
            .copy()
            .interpretation(Interpretation::Lab)
            .build();
        let prepared = to_integer_encodable(&lab).unwrap();
        assert!(
            !prepared.format().is_float(),
            "the prepared raster must be integer, got {:?}",
            prepared.format()
        );
        assert_eq!(prepared.format().bytes_per_channel(), 1, "8-bit sRGB");
        assert_eq!(prepared.format().channels(), 3, "3-band sRGB");
        assert_eq!(prepared.interpretation(), Interpretation::Srgb);
    }

    #[test]
    fn integer_sink_passes_a_device_raster_through_unchanged() {
        // A plain Gray8 (device interpretation) borrows through with no cast.
        let r = Raster::zeroed(2, 2, PixelFormat::Gray8).unwrap();
        let prepared = to_integer_encodable(&r).unwrap();
        assert!(matches!(prepared, std::borrow::Cow::Borrowed(_)));
        assert_eq!(prepared.format(), PixelFormat::Gray8);
    }

    #[test]
    fn cast_on_save_clips_to_range() {
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(3, 1, fmt, &[-10.0, 128.4, 999.0]).unwrap();
        let cast = cast_float_to_uchar_round_even(&r).unwrap();
        assert_eq!(cast.data(), &[0, 128, 255]);
    }
}
