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
//! **PNM sink (libviprs-cli #38).** `.ppm` / `.pnm` / `.pgm` are encoded
//! **directly here** (no core dependency — PNM is a trivial raster container):
//! a 3-band integer raster becomes binary **P6** (PPM), a 1-band integer raster
//! becomes binary **P5** (PGM), with the canonical `P6\n<w> <h>\n<maxval>\n`
//! header (maxval `255` for 8-bit, `65535` for 16-bit) followed by raw
//! **big-endian** sample bytes. Because PNM is an uncompressed, canonical
//! byte-format (no filters, no palette, no metadata blocks), two encoders that
//! agree on the pixels emit byte-identical payloads — so the differential suite
//! BYTE-compares `viprs …→.ppm` against the vips oracle (unlike `.png`, whose
//! filter/deflate choices force a decode-compare). Float / Fourier / multiband
//! (>3-band, e.g. RGBA) rasters are rejected with a clear error pointing at
//! `.png` (cast) or `.v` (lossless); see [`encode_pnm`].
//!
//! **16-bit depth on integer sinks (libviprs-cli #37).** vips's `pngsave` /
//! `tiffsave` pick their output bit depth from the raster **interpretation**,
//! not its pixel format: a `grey16` / `rgb16` raster saves 16-bit, while a
//! `b-w` / `multiband` / `srgb` raster saves 8-bit (even when its samples are
//! `ushort`). So when this module casts a **float** raster for an integer sink
//! it mirrors that rule — a `Grey16` / `Rgb16`-tagged float casts to 16-bit,
//! everything else to 8-bit (see [`cast_float_to_integer_round_even`]). The
//! honest caveat, verified against the vips 8.18.4 oracle, is documented on
//! [`cast_float_to_integer_round_even`]: libviprs' EXACT-AFTER-CAST ops
//! currently DROP the `grey16` interpretation to `multiband` on their float
//! result, so a 16-bit-input EAC op → `.png` still saves 8-bit here (matching
//! what the *raster* now says, not what vips does end-to-end). 16-bit EAC is
//! covered losslessly by the `.v` sink instead (see `cli_core_diff.rs`
//! `add_gray_expected.v`), which is why `cli_iocleanup_diff.rs` pins the 16-bit
//! `.png` path through a format-preserving op (`copy`) rather than a float EAC.
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
/// * `.png` — integer sink. A float raster is cast with **round-half-to-even
///   then clip** (to 8-bit `0..=255`, or to 16-bit `0..=65535` when the raster
///   is `Grey16`/`Rgb16`-tagged, mirroring vips — #37); 8-/16-bit integer
///   rasters pass straight through (16-bit passthrough preserved).
/// * `.tif` / `.tiff` — integer sink via core `tiff_save`, same
///   float-cast-then-encode path as `.png` (later waves — byteswap/autorot/
///   16-bit — need it, `CLI_CONTRACT.md` §2).
/// * `.ppm` / `.pnm` / `.pgm` — **PNM sink** encoded directly (#38): binary
///   **P6** for a 3-band integer raster, **P5** for a 1-band one, big-endian
///   samples, maxval `255`/`65535` by depth. Float / Fourier / multiband
///   (>3-band) rasters are rejected with a clear error (`.png`/`.v` instead).
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
        "ppm" | "pnm" | "pgm" => {
            let bytes = encode_pnm(raster)?;
            std::fs::write(path, bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(())
        }
        "" => bail!(
            "output path {} has no extension; use .png / .tif / .ppm (integer) or .v \
             (float / multiband / Fourier)",
            path.display()
        ),
        other => bail!(
            "unsupported output extension .{other}; differential sinks are .png / .tif / .ppm \
             (integer) and .v (float / multiband / Fourier)"
        ),
    }
}

/// The integer bit depth an interpretation saves to on vips's integer sinks
/// (`pngsave` / `tiffsave`), in **bytes per channel** (libviprs-cli #37).
///
/// vips picks its output bit depth from the raster **interpretation**, not its
/// pixel format: `grey16` / `rgb16` save 16-bit, everything else (`b-w`,
/// `multiband`, `srgb`, …) saves 8-bit. Verified against the vips 8.18.4 oracle
/// (`vips copy grey16.v out.png` → 16-bit; the same `ushort` retagged `b-w` →
/// 8-bit). Mirroring this keeps the cast-on-save depth faithful to vips.
fn integer_sink_depth(interp: Interpretation) -> usize {
    match interp {
        Interpretation::Grey16 | Interpretation::Rgb16 => 2,
        _ => 1,
    }
}

/// Cast a float raster to its integer counterpart with vips cast-on-save
/// semantics: **round-half-to-even** then clip to the target range.
///
/// The target depth mirrors vips (see [`integer_sink_depth`]): a `Grey16` /
/// `Rgb16`-tagged float casts to 16-bit (`0..=65535`), everything else to 8-bit
/// (`0..=255`). Rust's `f32::round` rounds half **away from zero** (`2.5 -> 3`),
/// which does not match vips; [`f64::round_ties_even`] gives the banker's
/// rounding the `getpoint` `.5` fixtures pin (`1.5 -> 1`, `2.5 -> 2`).
///
/// # The 16-bit EAC caveat (libviprs-cli #37)
///
/// The 16-bit branch is faithful to vips but, via the CLI today, **only fires
/// for a raster that still carries a `Grey16`/`Rgb16` tag**. vips propagates
/// that interpretation through its EXACT-AFTER-CAST ops (`linear`, `subtract`,
/// …) onto the float result, so `vips linear grey16.png out.png` saves 16-bit.
/// libviprs' EAC ops instead reset the interpretation to `multiband` on their
/// float output (a core-level propagation gap, out of scope for this pinned-core
/// CLI cleanup), so `viprs linear grey16.png out.png` reaches this caster with a
/// `multiband` float and saves 8-bit — matching what the raster now says, not
/// what vips does end-to-end. A 16-bit-input EAC → `.png` differential is
/// therefore impractical without core changes; 16-bit EAC is instead pinned
/// losslessly via the `.v` sink (`cli_core_diff.rs` `add_gray_expected.v`), and
/// the 16-bit `.png` *save path* is exercised through a format-preserving op
/// (`copy`, which keeps `Rgb16`) in `cli_iocleanup_diff.rs`.
fn cast_float_to_integer_round_even(raster: &Raster) -> Result<Raster> {
    let fmt = raster.format();
    debug_assert!(fmt.is_float(), "caller guarantees a float raster");
    // A Fourier / complex raster is non-displayable: casting its float bands to
    // an integer produces garbage, so refuse it (it belongs in a `.v` sink)
    // rather than silently emitting an approximation (`CLI_CONTRACT.md` §2/§8).
    if raster.interpretation() == Interpretation::Fourier {
        bail!(
            "a Fourier / complex raster is not displayable and must be written to a .v sink, \
             not cast to an integer image"
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
    let bpc = integer_sink_depth(raster.interpretation());
    let maxval = if bpc == 2 { 65535.0 } else { 255.0 };
    let target = PixelFormat::with_channels(channels, bpc).ok_or_else(|| {
        anyhow!(
            "cannot build a {}-bit format for {channels} channels",
            bpc * 8
        )
    })?;

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
            let rounded = (v as f64).round_ties_even().clamp(0.0, maxval);
            let ooff = y * out_stride + s * bpc;
            if bpc == 2 {
                out_data[ooff..ooff + 2].copy_from_slice(&(rounded as u16).to_ne_bytes());
            } else {
                out_data[ooff] = rounded as u8;
            }
        }
    }
    Ok(out)
}

/// Encode an **integer** raster as binary PNM (libviprs-cli #38): P6 (PPM) for a
/// 3-band raster, P5 (PGM) for a 1-band raster.
///
/// The header is the canonical `P6\n<w> <h>\n<maxval>\n` (magic chosen by band
/// count), `maxval` is `255` for 8-bit and `65535` for 16-bit samples, and the
/// payload is the raw samples in **big-endian** order (the PNM byte order;
/// samples live in the raster as native-endian, so 16-bit samples are byte-
/// swapped on a little-endian host). PNM carries no metadata and no compression,
/// so this payload is byte-identical to any conformant encoder's given the same
/// pixels — the property `cli_iocleanup_diff.rs` byte-compares against vips.
///
/// # Errors
///
/// Rejects — with a clear message pointing at `.png` (cast) or `.v` (lossless):
/// a **float** raster (PNM is integer-only; a caller wanting cast-on-save uses
/// `.png`), a **Fourier** raster (non-displayable), and any band count other
/// than 1 or 3 (PNM has no 2-band or alpha/RGBA form, and no multiband form).
fn encode_pnm(raster: &Raster) -> Result<Vec<u8>> {
    let fmt = raster.format();
    if fmt.is_float() {
        bail!(
            "PNM (.ppm/.pnm/.pgm) is an integer-only sink; a {fmt:?} float raster cannot be \
             PNM-encoded. Use .png to cast-on-save to 8/16-bit, or .v to keep the float data."
        );
    }
    if raster.interpretation() == Interpretation::Fourier {
        bail!(
            "a Fourier / complex raster is not displayable and must be written to a .v sink, \
             not PNM-encoded."
        );
    }
    let channels = fmt.channels();
    let magic = match channels {
        1 => "P5",
        3 => "P6",
        n => bail!(
            "PNM encodes only 1-band (P5/PGM) or 3-band (P6/PPM) rasters; this raster has {n} \
             bands. Use .png (1/3/4-band) or .v (any bands) instead."
        ),
    };
    let bpc = fmt.bytes_per_channel();
    let maxval: u32 = match bpc {
        1 => 255,
        2 => 65535,
        // is_float() ruled out the 4-byte case above; this is unreachable for a
        // real PixelFormat but keeps the mapping total.
        other => bail!("PNM cannot encode a {}-bit sample depth", other * 8),
    };

    let (w, h) = (raster.width() as usize, raster.height() as usize);
    let row_bytes = w * channels * bpc;
    let src = raster.data();
    let stride = raster.stride();

    let header = format!(
        "{magic}\n{} {}\n{maxval}\n",
        raster.width(),
        raster.height()
    );
    let mut out = Vec::with_capacity(header.len() + h * row_bytes);
    out.extend_from_slice(header.as_bytes());
    for y in 0..h {
        let row = &src[y * stride..y * stride + row_bytes];
        if bpc == 1 {
            out.extend_from_slice(row);
        } else {
            // Native-endian u16 samples -> big-endian PNM bytes.
            for sample in row.chunks_exact(2) {
                let v = u16::from_ne_bytes([sample[0], sample[1]]);
                out.extend_from_slice(&v.to_be_bytes());
            }
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
///    already-sRGB-tagged float) is cast with round-half-to-even then clip —
///    to 8-bit, or to 16-bit when the raster is `Grey16`/`Rgb16`-tagged (#37,
///    see [`cast_float_to_integer_round_even`]).
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
        Ok(Cow::Owned(cast_float_to_integer_round_even(raster)?))
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
    fn ppm_sink_round_trips_rgb8_to_exact_p6_bytes() {
        // #38: a known 2x1 Rgb8 raster must PNM-encode to the EXACT canonical P6
        // bytes — `P6\n2 1\n255\n` header then the raw RGB samples. This is the
        // byte-exactness the differential leans on.
        let r = Raster::new(2, 1, PixelFormat::Rgb8, vec![10, 20, 30, 40, 50, 60]).unwrap();
        let bytes = encode_pnm(&r).unwrap();
        let mut expected = b"P6\n2 1\n255\n".to_vec();
        expected.extend_from_slice(&[10, 20, 30, 40, 50, 60]);
        assert_eq!(bytes, expected);

        // Written through the extension-dispatched sink it lands on disk verbatim.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("viprs_io_ppm_{}.ppm", std::process::id()));
        save(&r, &path).expect("Rgb8 must PNM-encode");
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pgm_sink_round_trips_gray8_to_exact_p5_bytes() {
        // #38: a 1-band raster encodes as P5 (PGM), magic chosen by band count.
        let r = Raster::new(3, 1, PixelFormat::Gray8, vec![0, 128, 255]).unwrap();
        let bytes = encode_pnm(&r).unwrap();
        let mut expected = b"P5\n3 1\n255\n".to_vec();
        expected.extend_from_slice(&[0, 128, 255]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn ppm_sink_encodes_16bit_big_endian() {
        // #38: a Gray16 raster is P5, maxval 65535, big-endian samples. Native
        // storage is little-endian on the test host, so the encoder byte-swaps.
        let r = Raster::new(
            2,
            1,
            PixelFormat::Gray16,
            vec![0x00, 0x01, 0xff, 0x0a], // native-endian u16: 0x0100=256, 0x0aff=2815
        )
        .unwrap();
        let bytes = encode_pnm(&r).unwrap();
        let mut expected = b"P5\n2 1\n65535\n".to_vec();
        expected.extend_from_slice(&[0x01, 0x00, 0x0a, 0xff]); // big-endian
        assert_eq!(bytes, expected);
    }

    #[test]
    fn ppm_sink_rejects_float_and_rgba() {
        // Float → clear error pointing at .png / .v.
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let f = Raster::from_f32_samples(2, 1, fmt, &[1.0, 2.0]).unwrap();
        let err = save(&f, Path::new("out.ppm")).unwrap_err().to_string();
        assert!(err.contains("integer-only"), "got: {err}");

        // 4-band (RGBA) has no PNM form.
        let rgba = Raster::zeroed(2, 1, PixelFormat::Rgba8).unwrap();
        let err2 = encode_pnm(&rgba).unwrap_err().to_string();
        assert!(
            err2.contains("1-band") || err2.contains("bands"),
            "got: {err2}"
        );
    }

    #[test]
    fn fourier_raster_is_rejected_by_the_caster() {
        // A float raster tagged Fourier must refuse the integer cast.
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(2, 1, fmt, &[0.0, 1.0])
            .unwrap()
            .copy()
            .interpretation(Interpretation::Fourier)
            .build();
        let err = cast_float_to_integer_round_even(&r)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Fourier"), "got: {err}");
    }

    #[test]
    fn cast_on_save_rounds_half_to_even() {
        // A single-band float raster of 0.5, 1.5, 2.5, 3.5 must cast to
        // 0, 2, 2, 4 under round-half-to-even (not 1, 2, 3, 4).
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(4, 1, fmt, &[0.5, 1.5, 2.5, 3.5]).unwrap();
        let cast = cast_float_to_integer_round_even(&r).unwrap();
        assert_eq!(cast.format(), PixelFormat::Gray8);
        assert_eq!(cast.data(), &[0, 2, 2, 4]);
    }

    #[test]
    fn cast_on_save_grey16_float_targets_16bit() {
        // #37: a float raster tagged Grey16 casts to 16-bit (Gray16), mirroring
        // vips's interpretation-driven pngsave depth — not 8-bit. 32896.0 must
        // survive as the ushort 32896, not be clamped to 255.
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(1, 1, fmt, &[32896.0])
            .unwrap()
            .copy()
            .interpretation(Interpretation::Grey16)
            .build();
        let cast = cast_float_to_integer_round_even(&r).unwrap();
        assert_eq!(cast.format(), PixelFormat::Gray16);
        assert_eq!(cast.data(), &32896u16.to_ne_bytes());
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
        let cast = cast_float_to_integer_round_even(&r).unwrap();
        assert_eq!(cast.data(), &[0, 128, 255]);
    }
}
