//! The one way `viprs` turns an input file into a [`Raster`].
//!
//! The core's [`decode_file_with_limits`] and `decode_bytes_with_limits` do
//! the routing, SVG included: an SVG has no magic bytes, and the core sniffs
//! it by content (`libviprs::looks_like_svg`, libviprs#1170) and rasterises
//! it at the default options. The CLI used to carry its own copy of that
//! sniff (libviprs-cli#91). What is still settled here:
//!
//! * **The `.svg` / `.svgz` extension.** A file named as SVG goes straight to
//!   [`decode_svg_with_limits`], and a gzipped one is refused for what it is
//!   ([`CompressedSvg`]) rather than failing in the XML parser.
//! * **A codec this build left out.** The core reports it as a typed
//!   `FeatureNotEnabled` (or, for SVG, its "enable the `svg` feature" I/O
//!   error); this module turns that into a
//!   [`MissingFeature`](crate::features::MissingFeature) naming the feature to
//!   rebuild with, so the answer is never a generic "unsupported format".
//!
//! Every built-in (`info`, `plan`, `pyramid`) and the op harness
//! (`ops::io::load`) come through [`decode_path`] or [`decode_bytes`], so the
//! two answers are the same whichever command reads the file.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result};
use libviprs::Raster;
use libviprs::source::{DecodeLimits, decode_file_with_limits};
use libviprs::{SvgOptions, decode_svg_with_limits};

use crate::features::missing_feature;

/// The two bytes every gzip stream opens with, and so every `.svgz`.
const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];

/// A gzip-compressed SVG, which this build cannot read.
///
/// The core's renderer is `resvg` built without `usvg`'s gzip support (no
/// `flate2` in its tree), so a `.svgz` reaches the XML parser still
/// compressed and fails as a parse error. This says what is actually wrong.
#[derive(Debug)]
pub struct CompressedSvg;

impl std::fmt::Display for CompressedSvg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "this is a gzip-compressed SVG (.svgz), and the SVG renderer in this build has no \
             gzip support; gunzip it to a plain .svg first",
        )
    }
}

impl std::error::Error for CompressedSvg {}

/// Decode the image at `path` under `limits`.
///
/// # Errors
///
/// A [`MissingFeature`](crate::features::MissingFeature) when the format's
/// decoder was compiled out, otherwise the core's own decode error; either
/// way with the path as context.
pub fn decode_path(path: &Path, limits: DecodeLimits) -> Result<Raster> {
    let raster = if is_stream(path) {
        // A FIFO or `<(...)` can be read once and cannot seek, and the core's
        // file decoders seek. So read it once, here, and decode the bytes,
        // which also gets it the core's SVG sniff, as a regular file does.
        read_stream(path, &limits).and_then(|bytes| decode_bytes(&bytes, limits))
    } else if names_svg(path) {
        let bytes = read_svg(path)?;
        refuse_compressed_svg(&bytes)
            .and_then(|()| decode_svg(&bytes, SvgOptions::default(), limits))
    } else {
        decode_file_with_limits(path, limits).map_err(refusal_or_error)
    };
    raster.with_context(|| format!("failed to load image {}", path.display()))
}

/// [`decode_path`] under the core's default limits, for the built-ins that
/// take no `--max-*` flags.
///
/// # Errors
///
/// As [`decode_path`].
pub fn decode_path_default(path: &Path) -> Result<Raster> {
    decode_path(path, DecodeLimits::default())
}

/// [`decode_bytes`] under the core's default limits.
///
/// # Errors
///
/// As [`decode_bytes`].
pub fn decode_bytes_default(bytes: &[u8]) -> Result<Raster> {
    decode_bytes(bytes, DecodeLimits::default())
}

/// Decode an in-memory image (the `pyramid -` stdin path) under `limits`.
///
/// # Errors
///
/// As [`decode_path`], without the path.
pub fn decode_bytes(bytes: &[u8], limits: DecodeLimits) -> Result<Raster> {
    libviprs::source::decode_bytes_with_limits(bytes, limits).map_err(refusal_or_error)
}

/// A core decode error, or the [`MissingFeature`](crate::features::MissingFeature) it stands for.
fn refusal_or_error(err: libviprs::source::SourceError) -> anyhow::Error {
    match missing_feature(&err) {
        Some(missing) => missing.into(),
        None => err.into(),
    }
}

/// Refuse a gzip-compressed SVG with [`CompressedSvg`], before anything asks
/// whether this build has the `svg` feature: rebuilding with `svg` would not
/// help, so naming that feature would send the person the wrong way. Shared
/// with `svgload` (libviprs-cli#65), so every route into the renderer gives
/// the same answer.
pub(crate) fn refuse_compressed_svg(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(GZIP_MAGIC) {
        Err(CompressedSvg.into())
    } else {
        Ok(())
    }
}

/// Render an SVG document with `options`, turning a build without the `svg`
/// feature into its refusal. Shared with `svgload` (libviprs-cli#65), which
/// is the one caller that passes options.
pub(crate) fn decode_svg(
    bytes: &[u8],
    options: SvgOptions,
    limits: DecodeLimits,
) -> Result<Raster> {
    decode_svg_with_limits(bytes, options, limits).map_err(refusal_or_error)
}

/// Read an SVG document, refusing one over the core's input ceiling before
/// reading it rather than after.
fn read_svg(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let cap = libviprs::svg::MAX_INPUT_BYTES as u64;
    let mut bytes = Vec::new();
    // One byte past the ceiling is enough for the core to see that the
    // document is over it and refuse with its own typed error.
    file.take(cap + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn names_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// Whether `path` is something that exists but is neither a regular file nor
/// a directory: a FIFO, a `<(...)` process substitution, a character device.
fn is_stream(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.is_file() && !m.is_dir())
}

/// Read a stream input once, refusing one larger than the decode may
/// allocate rather than buffering it without end.
fn read_stream(path: &Path, limits: &DecodeLimits) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limits.max_alloc_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limits.max_alloc_bytes {
        anyhow::bail!(
            "the input stream is larger than the {} bytes this decode may allocate",
            limits.max_alloc_bytes
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch path for one test, under the system temp dir.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("viprs-input-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A PNG comfortably over the core's SVG sniff window, so a sniff that eats the
    /// head of a stream eats part of the image rather than all of it.
    fn noisy_png() -> Vec<u8> {
        let (w, h) = (64u32, 64u32);
        let mut state = 0x2545_f491_u32;
        let pixels: Vec<u8> = (0..w * h * 3)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let raster = Raster::new(w, h, libviprs::PixelFormat::Rgb8, pixels).unwrap();
        let png = raster.encode_png(6).unwrap();
        assert!(
            png.len() > libviprs::SVG_SNIFF_BYTES,
            "the fixture must outgrow the sniff"
        );
        png
    }

    /// A FIFO (or `<(...)` process substitution) can be read once. The SVG
    /// sniff used to open it and take the first 4 KB, and the decoder then
    /// opened it again and got the rest, or blocked waiting for a writer that
    /// had gone. Only regular files are sniffed now, and a stream is read once
    /// and decoded from its bytes, because the core's file decoders seek.
    #[cfg(unix)]
    #[test]
    fn a_fifo_input_keeps_its_first_bytes() {
        let dir = scratch("fifo");
        let fifo = dir.join("in.png");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must be runnable");
        assert!(made.success());

        let png = noisy_png();
        let writer_path = fifo.clone();
        std::thread::spawn(move || {
            use std::io::Write as _;
            // The FIFO may be opened more than once by a broken reader, so
            // keep offering the whole image until nobody opens it.
            for _ in 0..2 {
                if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&writer_path) {
                    let _ = f.write_all(&png);
                }
            }
        });

        let (tx, rx) = std::sync::mpsc::channel();
        let reader_path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(
                decode_path(&reader_path, DecodeLimits::default()).map(|r| (r.width(), r.height())),
            );
        });
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("decoding a FIFO must not hang");
        assert_eq!(got.map_err(|e| format!("{e:#}")), Ok((64, 64)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `.svgz` is gzip, and the SVG renderer in this build has no gzip
    /// support (usvg is built without flate2), so it gets a refusal that says
    /// so instead of an XML parse error.
    #[test]
    fn an_svgz_is_refused_saying_why() {
        let dir = scratch("svgz");
        let svgz = dir.join("in.svgz");
        std::fs::write(&svgz, b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03junk").unwrap();
        let err = decode_path(&svgz, DecodeLimits::default()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("gzip") && msg.contains("gunzip"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same document under a plain `.svg` name is still gzip.
    #[test]
    fn a_gzipped_document_named_svg_is_refused_saying_why() {
        let dir = scratch("svg-gz");
        let svg = dir.join("in.svg");
        std::fs::write(&svg, b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03junk").unwrap();
        let err = decode_path(&svg, DecodeLimits::default()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("gzip") && msg.contains("gunzip"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The error `decode_path` gives `bytes` saved as `name`, in a scratch
    /// directory whose path says nothing about SVG.
    fn refusal_for(tag: &str, name: &str, bytes: &[u8]) -> anyhow::Error {
        let dir = scratch(tag);
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let err = decode_path(&path, DecodeLimits::default()).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        err
    }

    /// Asserts `err` came from the raster route, not the SVG renderer or the
    /// missing-`svg` refusal.
    fn assert_not_routed_to_svg(err: &anyhow::Error) {
        let msg = format!("{err:#}");
        assert!(
            err.downcast_ref::<crate::features::MissingFeature>()
                .is_none(),
            "refused as a missing svg feature: {msg}"
        );
        assert!(!msg.to_lowercase().contains("svg"), "{msg}");
    }

    /// Core's sniff (libviprs#1170) wants the root element itself to be
    /// `svg`, so a root that only starts with those letters is not one. The
    /// CLI's own sniff claimed it (libviprs-cli#91).
    #[test]
    fn a_root_that_only_starts_with_svg_is_not_routed_to_the_renderer() {
        let err = refusal_for("lookalike-root", "doc.xml", b"<svgish/>");
        assert_not_routed_to_svg(&err);
    }

    /// The same for an `<svg` that only appears inside a comment.
    #[test]
    fn an_svg_inside_a_comment_is_not_routed_to_the_renderer() {
        let err = refusal_for(
            "commented-root",
            "doc.xml",
            b"<?xml version='1.0'?>\n<!-- <svg> -->\n<html/>",
        );
        assert_not_routed_to_svg(&err);
    }

    /// The content sniff is core's now; the CLI keeps only the extension
    /// route and the `.svgz` refusal.
    #[test]
    fn the_svg_content_sniff_is_cores() {
        // Spelled in pieces so this test does not match itself.
        let src = include_str!("input.rs");
        for name in [
            ["fn looks_like", "_svg("].concat(),
            ["const SVG_SNIFF", "_BYTES"].concat(),
            ["fn head_is", "_svg("].concat(),
        ] {
            assert!(
                !src.contains(&name),
                "src/input.rs still has `{name}`; core sniffs SVG (libviprs-cli#91)"
            );
        }
    }

    /// Without the feature an SVG is the typed refusal, never the generic
    /// decode error the content sniff would give it.
    #[cfg(not(feature = "svg"))]
    #[test]
    fn without_the_svg_feature_an_svg_is_a_missing_feature() {
        let err = decode_bytes(
            b"<svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'/>",
            DecodeLimits::default(),
        )
        .unwrap_err();
        let missing = err
            .downcast_ref::<crate::features::MissingFeature>()
            .unwrap_or_else(|| panic!("expected MissingFeature, got {err:#}"));
        assert_eq!(missing.feature, "svg");
    }

    #[cfg(feature = "svg")]
    #[test]
    fn with_the_svg_feature_an_svg_decodes() {
        let r = decode_bytes(
            b"<svg xmlns='http://www.w3.org/2000/svg' width='3' height='2'><rect width='3' height='2' fill='red'/></svg>",
            DecodeLimits::default(),
        )
        .unwrap();
        assert_eq!((r.width(), r.height()), (3, 2));
    }
}
