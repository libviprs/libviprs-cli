//! Shared matrix-file (`.mat`) loader — vips text matrix format
//! (`CLI_CONTRACT.md` §3 vector/matrix args, §9 missing-scope loader).
//!
//! vips passes convolution masks and morphological structuring elements as
//! **file** arguments in the text matrix format:
//!
//! ```text
//! <width> <height> [<scale> [<offset>]]
//! v v v ...      (width values)
//! ...            (height rows)
//! ```
//!
//! The optional trailing `scale`/`offset` on the header line is tolerated and
//! ignored (matching [`libviprs::Raster::matrix_load`]). This loader parses the
//! grid once and hands out both views the op families need:
//!
//! * [`MatFile::as_f64_rows`] — `&[&[f64]]` for a convolution
//!   [`Kernel`](libviprs::Kernel) (owned by the convolution family later); and
//! * [`MatFile::as_u8_mask`] — `Vec<Vec<u8>>` for a morphological mask, whose
//!   values are the vips `0` / `128` / `255` (must-be-zero / don't-care /
//!   must-be-set) encoding. The core `try_erode` / `try_dilate` validate the
//!   `0/128/255` invariant, so this loader only rounds to the nearest byte.
//!
//! Morphology owns this module first (`CLI_CONTRACT.md` §9); the convolution
//! family reuses the same parser when it lands.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// A parsed vips text matrix: a dense `height` × `width` grid of `f64` values.
///
/// `width` / `height` and the `f64` view are the shared-loader surface the
/// convolution family (`conv`, `convsep`, `recomb`, `buildlut`, …) consumes
/// when it lands; morphology only needs [`MatFile::as_u8_mask`] today, so those
/// members carry `#[allow(dead_code)]` until the second consumer arrives.
#[allow(dead_code)]
pub struct MatFile {
    rows: Vec<Vec<f64>>,
    width: usize,
    height: usize,
}

impl MatFile {
    /// Read and parse a `.mat` file from disk.
    ///
    /// # Errors
    ///
    /// I/O failure, non-UTF-8 content, a missing/non-numeric header, a
    /// non-numeric value, or a value count that disagrees with the declared
    /// dimensions.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read matrix file {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid matrix file {}", path.display()))
    }

    /// Parse the vips text matrix format from an in-memory string.
    ///
    /// # Errors
    ///
    /// As [`MatFile::load`] (minus the I/O case).
    pub fn parse(text: &str) -> Result<Self> {
        // Skip blank lines and `#` comments; the first surviving line is the
        // `width height [scale offset]` header.
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

        let mut rows: Vec<Vec<f64>> = Vec::with_capacity(height);
        for line in lines {
            let mut row = Vec::with_capacity(width);
            for tok in line.split_whitespace() {
                row.push(
                    tok.parse::<f64>()
                        .with_context(|| format!("matrix value {tok:?} is not a number"))?,
                );
            }
            rows.push(row);
        }

        if rows.len() != height {
            bail!(
                "matrix declares {height} rows but has {} value rows",
                rows.len()
            );
        }
        for (i, row) in rows.iter().enumerate() {
            if row.len() != width {
                bail!("matrix row {i} has {} values, expected {width}", row.len());
            }
        }

        Ok(MatFile {
            rows,
            width,
            height,
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
    /// Each value is rounded half-to-even to the nearest byte and clipped to
    /// `0..=255`; the core morphology ops then validate the vips `0/128/255`
    /// encoding and reject anything else with a typed error.
    pub fn as_u8_mask(&self) -> Vec<Vec<u8>> {
        self.rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|&v| v.round_ties_even().clamp(0.0, 255.0) as u8)
                    .collect()
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
    fn header_scale_offset_is_ignored() {
        let m = MatFile::parse("2 1 2.0 0.5\n10 20\n").unwrap();
        assert_eq!((m.width(), m.height()), (2, 1));
        assert_eq!(m.as_f64_rows()[0], &[10.0, 20.0]);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let m =
            MatFile::parse("# a cross\n\n3 3\n128 255 128\n255 255 255\n128 255 128\n").unwrap();
        assert_eq!((m.width(), m.height()), (3, 3));
    }

    #[test]
    fn u8_mask_view_rounds_to_bytes() {
        let m = MatFile::parse("2 1\n0 255\n").unwrap();
        assert_eq!(m.as_u8_mask(), vec![vec![0u8, 255u8]]);
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
