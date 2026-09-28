//! The one way `viprs` turns an input file into a [`Raster`].
//!
//! Two things are settled here that the core's content-sniffing
//! [`decode_file_with_limits`] does not settle on its own (libviprs-cli#64):
//!
//! * **SVG.** The core sniffs raster containers by their magic bytes, and an
//!   SVG document has none, so it never reaches [`decode_svg_with_limits`]
//!   through that route and fails as an unrecognised format. It is routed
//!   here instead, by extension or by an `<svg` root near the top of the file.
//! * **A codec this build left out.** The core reports it as a typed
//!   `FeatureNotEnabled`; this module turns that into a
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

/// How far into a file to look for an `<svg` root when the extension does not
/// already say SVG. An XML declaration, a doctype and a licence comment fit
/// comfortably; a document whose root starts later than this is not one a
/// person meant to hand over without its extension.
const SVG_SNIFF_BYTES: usize = 4096;

/// Decode the image at `path` under `limits`.
///
/// # Errors
///
/// A [`MissingFeature`](crate::features::MissingFeature) when the format's
/// decoder was compiled out, otherwise the core's own decode error; either
/// way with the path as context.
pub fn decode_path(path: &Path, limits: DecodeLimits) -> Result<Raster> {
    let raster = if names_svg(path) || head_is_svg(path) {
        let bytes = read_svg(path)?;
        decode_svg(&bytes, limits)
    } else {
        decode_file_with_limits(path, limits).map_err(refusal_or_error)
    };
    raster.with_context(|| format!("failed to load image {}", path.display()))
}

/// Decode an in-memory image (the `pyramid -` stdin path) under `limits`.
///
/// # Errors
///
/// As [`decode_path`], without the path.
pub fn decode_bytes(bytes: &[u8], limits: DecodeLimits) -> Result<Raster> {
    if looks_like_svg(bytes) {
        return decode_svg(bytes, limits);
    }
    libviprs::source::decode_bytes_with_limits(bytes, limits).map_err(refusal_or_error)
}

/// A core decode error, or the [`MissingFeature`](crate::features::MissingFeature) it stands for.
fn refusal_or_error(err: libviprs::source::SourceError) -> anyhow::Error {
    match missing_feature(&err) {
        Some(missing) => missing.into(),
        None => err.into(),
    }
}

fn decode_svg(bytes: &[u8], limits: DecodeLimits) -> Result<Raster> {
    match decode_svg_with_limits(bytes, SvgOptions::default(), limits) {
        Ok(raster) => Ok(raster),
        // Without the `svg` feature the core's only answer is this one
        // `Unsupported` I/O error (`libviprs::svg`), so in that build it is
        // the refusal and nothing else. With the feature on the arm is not
        // compiled, and an `Unsupported` from the renderer stays what it is.
        #[cfg(not(feature = "svg"))]
        Err(libviprs::DecodeError::Io(e)) if e.kind() == std::io::ErrorKind::Unsupported => {
            Err(crate::features::MissingFeature {
                feature: "svg",
                format: "SVG",
            }
            .into())
        }
        Err(e) => Err(e.into()),
    }
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

fn head_is_svg(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        // Let the real decode report the open failure.
        return false;
    };
    let mut head = Vec::with_capacity(SVG_SNIFF_BYTES);
    if file
        .take(SVG_SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    looks_like_svg(&head)
}

/// Whether `bytes` open an SVG document: after an optional BOM and
/// whitespace, either an `<svg` root straight away, or an XML prologue (`<?`,
/// `<!`) with an `<svg` root somewhere in the first [`SVG_SNIFF_BYTES`].
///
/// Deliberately narrow. Every raster container the core sniffs starts with
/// binary magic, so none of them can start with `<`, and a text format that
/// is not SVG does not contain an `<svg` element.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(SVG_SNIFF_BYTES)];
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let start = head
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(head.len());
    let head = &head[start..];
    if head.starts_with(b"<svg") {
        return true;
    }
    (head.starts_with(b"<?") || head.starts_with(b"<!")) && head.windows(4).any(|w| w == b"<svg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_root_is_recognised_with_or_without_a_prologue() {
        assert!(looks_like_svg(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(looks_like_svg(b"  \n<svg/>"));
        assert!(looks_like_svg(b"\xEF\xBB\xBF<svg/>"));
        assert!(looks_like_svg(
            b"<?xml version='1.0'?>\n<!-- licence -->\n<svg/>"
        ));
        assert!(looks_like_svg(b"<!DOCTYPE svg>\n<svg/>"));
    }

    #[test]
    fn raster_magic_and_other_xml_are_not_svg() {
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!looks_like_svg(b"\xff\x0a"));
        assert!(!looks_like_svg(b"<?xml version='1.0'?><html/>"));
        assert!(!looks_like_svg(b""));
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
