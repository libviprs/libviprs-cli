//! Shared matrix-file (`.mat`) loader — vips text matrix format
//! (`CLI_CONTRACT.md` §3 vector/matrix args, §9 missing-scope loader).
//!
//! vips passes convolution masks and morphological structuring elements as
//! **file** arguments in the text matrix format:
//!
//! ```text
//! <width> <height> [<scale> [<offset>]]
//! v v v ...      (values — vips reads a FLAT whitespace token stream)
//! ...
//! ```
//!
//! The vips/core `matrix_load` reads **all** values as one flat whitespace
//! token stream and only checks that the total count equals `width * height`;
//! it does **not** require one row per line. This loader matches that: a 3×3
//! mask written as nine tokens on a single line, or wrapped across three lines,
//! both parse. It then hands out both views the op families need:
//!
//! * [`MatFile::as_f64_rows`] — `&[&[f64]]` for a convolution
//!   [`Kernel`](libviprs::Kernel) (owned by the convolution family later); and
//! * [`MatFile::as_u8_mask`] — `Vec<Vec<u8>>` for a morphological mask, whose
//!   values must be the vips `0` / `128` / `255` (must-be-zero / don't-care /
//!   must-be-set) encoding. A value outside that set is an **error** (matching
//!   vips), never a silent clamp.
//!
//! The optional header `scale` (default `1.0`) and `offset` (default `0.0`) are
//! parsed and retained: a convolution [`Kernel`](libviprs::Kernel) divides its
//! accumulated sum by `scale`, so dropping it would be irrecoverable.
//!
//! Morphology owns this module first (`CLI_CONTRACT.md` §9); the convolution
//! family reuses the same parser when it lands.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Upper bound on the `.mat` file size read into memory. A structuring element
/// or convolution mask is tiny; this cap only exists so a hostile path cannot
/// make the loader read an unbounded file (16 MiB is orders of magnitude beyond
/// any real mask).
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Upper bound on `width * height`. Guards the reshape against an attacker
/// header (e.g. `1 1000000000000000000`) declaring an absurd element count.
/// Even the largest real LUT/matrix stays far below this.
const MAX_ELEMENTS: usize = 16 * 1024 * 1024;

/// Values within this distance of a mask level (`0` / `128` / `255`) are
/// accepted as that level; anything else is rejected by [`MatFile::as_u8_mask`].
const MASK_EPS: f64 = 1e-6;

/// A parsed vips text matrix: a dense `height` × `width` grid of `f64` values
/// plus the optional header `scale` / `offset`.
///
/// `width` / `height`, the `f64` view, and `scale` / `offset` are the
/// shared-loader surface the convolution family (`conv`, `convsep`, `recomb`,
/// `buildlut`, …) consumes when it lands; morphology only needs
/// [`MatFile::as_u8_mask`] today, so those members carry `#[allow(dead_code)]`
/// until the second consumer arrives.
#[allow(dead_code)]
pub struct MatFile {
    rows: Vec<Vec<f64>>,
    width: usize,
    height: usize,
    scale: f64,
    offset: f64,
}

impl MatFile {
    /// Read and parse a `.mat` file from disk.
    ///
    /// The read is capped at [`MAX_FILE_BYTES`] so a hostile path cannot force
    /// an unbounded allocation before parsing even begins.
    ///
    /// # Errors
    ///
    /// I/O failure, non-UTF-8 content, a missing/non-numeric header, a
    /// non-numeric value, or a value count that disagrees with the declared
    /// dimensions.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("failed to open matrix file {}", path.display()))?;
        // Read at most MAX_FILE_BYTES + 1 so we can detect (and reject) an
        // oversize file rather than silently truncating it.
        let mut buf = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut buf)
            .with_context(|| format!("failed to read matrix file {}", path.display()))?;
        if buf.len() as u64 > MAX_FILE_BYTES {
            bail!(
                "matrix file {} is larger than the {MAX_FILE_BYTES}-byte cap",
                path.display()
            );
        }
        let text = String::from_utf8(buf)
            .with_context(|| format!("matrix file {} is not valid UTF-8", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid matrix file {}", path.display()))
    }

    /// Parse the vips text matrix format from an in-memory string.
    ///
    /// The header line is `width height [scale [offset]]`; every remaining
    /// non-comment token is a matrix value, read as **one flat whitespace
    /// stream** (vips does not require one row per line). The value count must
    /// equal `width * height` (a typed error otherwise — never a panic), and
    /// the grid is reshaped into `width`-sized rows.
    ///
    /// # Errors
    ///
    /// As [`MatFile::load`] (minus the I/O case).
    pub fn parse(text: &str) -> Result<Self> {
        // Skip blank lines and `#` comments; the first surviving line is the
        // `width height [scale [offset]]` header.
        let mut lines = text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));

        let header = lines.next().context("matrix file is empty")?;
        let mut hdr = header.split_whitespace();
        let width: usize = hdr
            .next()
            .context("matrix header is missing the width")?
            .parse()
            .context("matrix header width is not an integer")?;
        let height: usize = hdr
            .next()
            .context("matrix header is missing the height")?
            .parse()
            .context("matrix header height is not an integer")?;
        if width == 0 || height == 0 {
            bail!("matrix header declares a zero dimension ({width}x{height})");
        }

        // Optional trailing scale / offset on the header line (defaults per
        // vips: scale 1.0, offset 0.0). A convolution Kernel needs the scale.
        let scale: f64 = match hdr.next() {
            Some(tok) => tok.parse().context("matrix header scale is not a number")?,
            None => 1.0,
        };
        let offset: f64 = match hdr.next() {
            Some(tok) => tok
                .parse()
                .context("matrix header offset is not a number")?,
            None => 0.0,
        };

        // Reject an absurd declared element count BEFORE allocating anything,
        // so a hostile header can never trigger a capacity-overflow panic.
        let expected = width
            .checked_mul(height)
            .filter(|&n| n <= MAX_ELEMENTS)
            .with_context(|| {
                format!("matrix dimensions {width}x{height} exceed the {MAX_ELEMENTS}-element cap")
            })?;

        // Collect ALL remaining tokens as one flat stream. NO header-derived
        // pre-sizing: the Vec grows to the real token count only.
        let mut values: Vec<f64> = Vec::new();
        for line in lines {
            for tok in line.split_whitespace() {
                values.push(
                    tok.parse::<f64>()
                        .with_context(|| format!("matrix value {tok:?} is not a number"))?,
                );
            }
        }

        if values.len() != expected {
            bail!(
                "matrix declares {width}x{height} = {expected} values but has {}",
                values.len()
            );
        }

        // Reshape the flat stream into width-sized rows.
        let rows: Vec<Vec<f64>> = values.chunks(width).map(<[f64]>::to_vec).collect();

        Ok(MatFile {
            rows,
            width,
            height,
            scale,
            offset,
        })
    }

    /// Matrix width (columns).
    #[allow(dead_code)] // convolution-family surface; see the struct doc.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Matrix height (rows).
    #[allow(dead_code)] // convolution-family surface; see the struct doc.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Header scale divisor (default `1.0`), as a convolution
    /// [`Kernel`](libviprs::Kernel) `scale`.
    #[allow(dead_code)] // convolution-family surface; see the struct doc.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Header offset summand (default `0.0`).
    #[allow(dead_code)] // convolution-family surface; see the struct doc.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Borrow the grid as `&[&[f64]]` for a convolution
    /// [`Kernel`](libviprs::Kernel).
    ///
    /// The returned `Vec` owns the row slices; keep it alive for as long as the
    /// `&[&[f64]]` is used.
    #[allow(dead_code)] // convolution-family surface; see the struct doc.
    pub fn as_f64_rows(&self) -> Vec<&[f64]> {
        self.rows.iter().map(|r| r.as_slice()).collect()
    }

    /// Materialise the grid as morphological mask rows (`Vec<Vec<u8>>`).
    ///
    /// Each value must be within [`MASK_EPS`] of one of the vips mask levels
    /// `0` / `128` / `255` (must-be-zero / don't-care / must-be-set). A value
    /// outside that set — e.g. `1000` — is an **error**, matching vips, rather
    /// than a silent clamp to `255`.
    ///
    /// # Errors
    ///
    /// A value that is not (within `MASK_EPS` of) `0`, `128`, or `255`.
    pub fn as_u8_mask(&self) -> Result<Vec<Vec<u8>>> {
        self.rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|&v| {
                        for level in [0u8, 128, 255] {
                            if (v - f64::from(level)).abs() <= MASK_EPS {
                                return Ok(level);
                            }
                        }
                        bail!("mask value {v} is not a vips mask level (expected 0, 128 or 255)")
                    })
                    .collect::<Result<Vec<u8>>>()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_grid() {
        let m = MatFile::parse("3 2\n0 128 255\n255 128 0\n").unwrap();
        assert_eq!(m.width(), 3);
        assert_eq!(m.height(), 2);
        let rows = m.as_f64_rows();
        assert_eq!(rows[0], &[0.0, 128.0, 255.0]);
        assert_eq!(rows[1], &[255.0, 128.0, 0.0]);
    }

    #[test]
    fn header_scale_offset_is_retained() {
        let m = MatFile::parse("2 1 2.0 0.5\n10 20\n").unwrap();
        assert_eq!((m.width(), m.height()), (2, 1));
        assert_eq!(m.as_f64_rows()[0], &[10.0, 20.0]);
        assert_eq!(m.scale(), 2.0);
        assert_eq!(m.offset(), 0.5);
    }

    #[test]
    fn scale_offset_default_to_one_and_zero() {
        let m = MatFile::parse("2 1\n10 20\n").unwrap();
        assert_eq!(m.scale(), 1.0);
        assert_eq!(m.offset(), 0.0);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let m =
            MatFile::parse("# a cross\n\n3 3\n128 255 128\n255 255 255\n128 255 128\n").unwrap();
        assert_eq!((m.width(), m.height()), (3, 3));
    }

    #[test]
    fn flat_token_stream_on_one_line_parses() {
        // vips/core matrix_load reads a FLAT token stream: a 3x3 mask given as
        // nine tokens on a single line must parse and reshape to 3 rows.
        let m = MatFile::parse("3 3\n0 128 255 255 128 0 128 255 128\n").unwrap();
        assert_eq!((m.width(), m.height()), (3, 3));
        let rows = m.as_f64_rows();
        assert_eq!(rows[0], &[0.0, 128.0, 255.0]);
        assert_eq!(rows[1], &[255.0, 128.0, 0.0]);
        assert_eq!(rows[2], &[128.0, 255.0, 128.0]);
    }

    #[test]
    fn u8_mask_view_maps_levels() {
        let m = MatFile::parse("3 1\n0 128 255\n").unwrap();
        assert_eq!(m.as_u8_mask().unwrap(), vec![vec![0u8, 128u8, 255u8]]);
    }

    #[test]
    fn u8_mask_rejects_out_of_range_value() {
        // A mask value of 1000 must ERROR like vips, not clamp to 255.
        let m = MatFile::parse("2 1\n0 1000\n").unwrap();
        assert!(m.as_u8_mask().is_err());
    }

    #[test]
    fn hostile_header_is_an_error_not_a_panic() {
        // An attacker-controlled header declaring ~1e18 elements must be a
        // typed Err (no pre-sizing, no capacity-overflow panic / alloc abort).
        let r = MatFile::parse("1 1000000000000000000\n1\n");
        assert!(r.is_err());
    }

    #[test]
    fn dimension_mismatch_is_an_error() {
        assert!(MatFile::parse("3 2\n1 2 3\n").is_err());
        assert!(MatFile::parse("2 1\n1 2 3\n").is_err());
    }

    #[test]
    fn non_numeric_value_is_an_error() {
        assert!(MatFile::parse("2 1\n1 x\n").is_err());
    }
}
