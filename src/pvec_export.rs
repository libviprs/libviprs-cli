//! `viprs pvec export FILE OUT --format gml|gpkg|mvt|pmtiles|step` (libviprs-cli#154, CV2.5 of EPIC CV):
//! a `.pvec` through PVec's interchange writers, with the fidelity report that says what each one did
//! to the geometry.
//!
//! The GML, GeoPackage and STEP writers take every entity of the file. `mvt` is one display-only vector
//! tile (`--tile Z/X/Y`, default `0/0/0`), and `pmtiles` packs every tile of a zoom range into an
//! archive. For both the drawing's declared bounds are fitted into a square map with the top-left of
//! the bounds at the top-left of the map, so tile `0/0/0` is the whole drawing.
//!
//! As with the other `pvec` commands, this file holds the arguments and every refusal the command line
//! decides by itself (exit 2), and runs in every build; the writers are in `imp`, behind `pvec-export`,
//! and a build without the feature exits 1 naming it. Exports are deterministic: no clock is read, so
//! the same file and flags give the same bytes.

use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::ops::foreign::positive_finite;
use crate::paths::same_file;
use crate::usage_error;

#[cfg(feature = "pvec-export")]
pub(crate) mod imp;

/// What to write.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// GML 3.2.1 `MultiCurve`, the curves as GML constructs where one exists.
    Gml,
    /// A GeoPackage 1.4 with one feature per entity.
    Gpkg,
    /// One Mapbox Vector Tile, display only.
    Mvt,
    /// A PMTiles v3 archive of vector tiles, display only.
    Pmtiles,
    /// STEP AP242 (ISO 10303-21), exact curves only.
    Step,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Gml => "gml",
            Format::Gpkg => "gpkg",
            Format::Mvt => "mvt",
            Format::Pmtiles => "pmtiles",
            Format::Step => "step",
        }
    }
}

/// `viprs pvec export`.
#[derive(Args)]
#[cfg_attr(not(feature = "pvec-export"), allow(dead_code))]
pub(crate) struct ExportArgs {
    /// The `.pvec` to export.
    pub(crate) file: PathBuf,

    /// Where the export goes. Written beside itself and renamed over, so a failed export leaves
    /// nothing behind.
    pub(crate) out: PathBuf,

    /// What to write.
    #[arg(long, value_enum)]
    pub(crate) format: Format,

    /// The most a curve approximated as line segments may deviate, in the units of the output
    /// (source units unless `--affine` changes them). Needed by `gml` and `gpkg`.
    #[arg(
        long,
        value_name = "UNITS",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(crate) tolerance: Option<f64>,

    /// The CRS name written as `srsName` (`gml` only). Nothing here knows what it means: no CRS
    /// database is read and nothing is reprojected. The default, `urn:pvec:local`, is the drawing's
    /// own units.
    #[arg(long, value_name = "NAME")]
    pub(crate) srs: Option<String>,

    /// Source to output coordinates, `A,B,C,D,TX,TY` with `x' = A x + B y + TX` and `y' = C x + D y +
    /// TY` (`gml` and `step`). The default is the identity.
    #[arg(
        long,
        value_name = "A,B,C,D,TX,TY",
        allow_hyphen_values = true,
        value_parser = affine
    )]
    pub(crate) affine: Option<[f64; 6]>,

    /// The tile `mvt` writes, `Z/X/Y` with the origin at the top-left; the default is `0/0/0`.
    #[arg(long, value_name = "Z/X/Y", allow_hyphen_values = true, value_parser = tile)]
    pub(crate) tile: Option<(u8, u32, u32)>,

    /// The zoom range `pmtiles` writes, `MIN-MAX` (at most 6); the default is `0-0`.
    #[arg(long, value_name = "MIN-MAX", value_parser = zooms)]
    pub(crate) zooms: Option<(u8, u8)>,

    /// The chord budget of `mvt` and `pmtiles` in screen pixels (the tile is 256 pixels wide).
    #[arg(
        long,
        value_name = "PIXELS",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(crate) error_px: Option<f64>,

    /// Write PVec's fidelity report here: Markdown when the name ends in `.md`, JSON otherwise.
    #[arg(long, value_name = "PATH")]
    pub(crate) report: Option<PathBuf>,
}

/// `A,B,C,D,TX,TY`: six finite numbers whose linear part can be undone.
fn affine(text: &str) -> Result<[f64; 6], String> {
    const SHAPE: &str = "expected six finite numbers, as 0.001,0,0,0.001,0,0";
    let n: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()
        .ok_or(SHAPE)?;
    let [a, b, c, d, tx, ty] = n[..] else {
        return Err(SHAPE.into());
    };
    if a * d - b * c == 0.0 {
        return Err("the linear part has no inverse (A*D - B*C is 0)".into());
    }
    Ok([a, b, c, d, tx, ty])
}

/// `Z/X/Y` inside the `2^Z` square.
fn tile(text: &str) -> Result<(u8, u32, u32), String> {
    const SHAPE: &str = "expected Z/X/Y, three whole numbers, as 3/6/1";
    let parts: Vec<&str> = text.split('/').collect();
    let [z, x, y] = parts[..] else {
        return Err(SHAPE.into());
    };
    let (z, x, y) = (
        z.parse::<u8>().map_err(|_| SHAPE)?,
        x.parse::<u32>().map_err(|_| SHAPE)?,
        y.parse::<u32>().map_err(|_| SHAPE)?,
    );
    if z > 22 {
        return Err(format!("zoom {z} is past 22"));
    }
    if u64::from(x) >= 1 << z || u64::from(y) >= 1 << z {
        return Err(format!(
            "tile {z}/{x}/{y} is outside the {0}x{0} tiles of zoom {z}",
            1u64 << z
        ));
    }
    Ok((z, x, y))
}

/// `MIN-MAX`, two whole numbers, at most 6 (4^6 tiles is already 4096 tiles of tessellation).
fn zooms(text: &str) -> Result<(u8, u8), String> {
    let range = text
        .split_once('-')
        .and_then(|(lo, hi)| Some((lo.parse::<u8>().ok()?, hi.parse::<u8>().ok()?)))
        .ok_or("expected MIN-MAX, two whole numbers, as 0-3")?;
    if range.0 > range.1 {
        return Err(format!("{} is above {}", range.0, range.1));
    }
    if range.1 > 6 {
        return Err(format!("zoom {} is past 6", range.1));
    }
    Ok(range)
}

/// Every refusal that needs no file: a flag the format does not read, one it needs, and an output
/// that is the input.
pub(crate) fn check(a: &ExportArgs) {
    let name = a.format.name();
    let only = |flag: &str, set: bool, formats: &[Format]| {
        if set && !formats.contains(&a.format) {
            let list: Vec<&str> = formats.iter().map(|f| f.name()).collect();
            usage_error(
                &format!("{flag} does not apply to --format {name}"),
                &format!("it is read by --format {}", list.join(" and ")),
            );
        }
    };
    only(
        "--tolerance",
        a.tolerance.is_some(),
        &[Format::Gml, Format::Gpkg],
    );
    only("--srs", a.srs.is_some(), &[Format::Gml]);
    only("--affine", a.affine.is_some(), &[Format::Gml, Format::Step]);
    only("--tile", a.tile.is_some(), &[Format::Mvt]);
    only("--zooms", a.zooms.is_some(), &[Format::Pmtiles]);
    only(
        "--error-px",
        a.error_px.is_some(),
        &[Format::Mvt, Format::Pmtiles],
    );
    if matches!(a.format, Format::Gml | Format::Gpkg) && a.tolerance.is_none() {
        usage_error(
            &format!("--tolerance is required for --format {name}"),
            "it is the most a curve written as line segments may deviate, in the output's units",
        );
    }
    if same_file(&a.file, &a.out) {
        usage_error(
            "OUT is the input file",
            "an export never writes over the drawing it reads; give another path",
        );
    }
    if let Some(report) = &a.report {
        if same_file(&a.file, report) {
            usage_error(
                "--report is the input file",
                "the report would overwrite the drawing; give another path",
            );
        }
        if same_file(&a.out, report) {
            usage_error(
                "--report is OUT",
                "the report and the export would replace each other; give another path",
            );
        }
    }
}

/// `viprs pvec export`.
pub(crate) fn run(a: ExportArgs) {
    check(&a);
    #[cfg(feature = "pvec-export")]
    imp::run(&a);
    #[cfg(not(feature = "pvec-export"))]
    {
        let _ = a;
        crate::operational_error(
            "`viprs pvec export` needs a build with the `pvec-export` feature (rebuild with \
             `--features pvec-export`); `viprs features` lists what this binary has",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parsers_take_what_the_help_promises_and_nothing_else() {
        assert_eq!(tile("3/6/1"), Ok((3, 6, 1)));
        assert_eq!(tile("0/0/0"), Ok((0, 0, 0)));
        for bad in ["1/2", "a/b/c", "0/1/0", "2/4/0", "40/0/0", "-1/0/0", ""] {
            assert!(tile(bad).is_err(), "{bad:?}");
        }
        assert_eq!(zooms("0-3"), Ok((0, 3)));
        for bad in ["3-1", "0-9", "x-2", "1", "0..2", ""] {
            assert!(zooms(bad).is_err(), "{bad:?}");
        }
        assert_eq!(affine("1,0,0,-1,5,6"), Ok([1.0, 0.0, 0.0, -1.0, 5.0, 6.0]));
        for bad in ["1,0,0,1,0", "1,0,0,1,0,x", "0,0,0,0,0,0", "1,2,2,4,0,0", ""] {
            assert!(affine(bad).is_err(), "{bad:?}");
        }
    }
}
