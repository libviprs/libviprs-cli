//! The read side of a `.pvec` (libviprs-cli#147, CV1.1 to CV1.4 of EPIC CV):
//!
//! * `viprs pvec info FILE [--json]`: the manifest, the component streams, layers, text and
//!   raster counts, and the scale a reader should use.
//! * `viprs pvec validate FILE [--json]`: open the file (block checksums and the artifact
//!   digest), then decode every entity, style, known component stream and the calibration.
//! * `viprs pvec text FILE [--json]`: every text record.
//! * `viprs pvec calibrate FILE`: set, replace or drop the scale record, rewritten in place.
//!
//! The libviprs core has no PVec read API, so these go to PVec's facade directly, at the revision
//! the core pins. As with `pvec rasters`, this file holds the arguments and every refusal the
//! command line decides by itself (exit 2), and runs in every build; the facade calls are in
//! `imp`, behind `pvec-raster`, and `pvec_raster::run` dispatches to them. A build without the
//! feature exits 1 naming it.

use std::path::PathBuf;

use clap::{ArgGroup, Args};

use crate::ops::foreign::positive_finite;

#[cfg(feature = "pvec-raster")]
pub(crate) mod imp;

/// `viprs pvec info`, `validate` and `text`: one file, text or JSON.
#[derive(Args)]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct ReadArgs {
    /// The `.pvec` to read.
    pub(crate) file: PathBuf,

    /// Print one JSON document (`"v": 1`) instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

/// `viprs pvec calibrate`.
#[derive(Args)]
#[command(group(
    ArgGroup::new("mode")
        .required(true)
        .args(["scale", "plan_scale", "measured", "remove"]),
))]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct CalibrateArgs {
    /// The `.pvec` to calibrate. It is rewritten in place: the new file goes to a temporary
    /// beside it and is renamed over it, so a reader sees the old file or the new one.
    file: PathBuf,

    /// What one drawing unit is in the world, in metres, stated by hand (provenance `manual`).
    #[arg(long, value_name = "METRES", allow_negative_numbers = true, value_parser = positive_finite)]
    scale: Option<f64>,

    /// The plan's printed scale as two whole numbers, `1:100`; needs `--paper-unit`. One
    /// drawing unit is then the paper unit times 100, in metres (provenance `plan-scale`).
    #[arg(long, value_name = "N:D", requires = "paper_unit", value_parser = ratio)]
    plan_scale: Option<(u32, u32)>,

    /// What one drawing unit is on paper, in metres (0.001 for a sheet drawn in millimetres).
    #[arg(
        long,
        value_name = "METRES",
        requires = "plan_scale",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    paper_unit: Option<f64>,

    /// Two points in drawing units whose distance apart was measured, `X0,Y0,X1,Y1`; needs
    /// `--distance` (provenance `measured`).
    #[arg(
        long,
        value_name = "X0,Y0,X1,Y1",
        requires = "distance",
        allow_hyphen_values = true,
        value_parser = segment
    )]
    measured: Option<[f64; 4]>,

    /// The measured distance between the two `--measured` points, in metres.
    #[arg(
        long,
        value_name = "METRES",
        requires = "measured",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    distance: Option<f64>,

    /// Drop the scale record. A georeference the file carries stays.
    #[arg(long)]
    remove: bool,

    /// A note stored with the scale (where it came from).
    #[arg(long, value_name = "TEXT", conflicts_with = "remove")]
    note: Option<String>,

    /// The relative uncertainty of the scale, 0 or more (0.01 is 1%).
    #[arg(
        long,
        value_name = "RATIO",
        conflicts_with = "remove",
        allow_negative_numbers = true,
        value_parser = non_negative
    )]
    uncertainty: Option<f64>,
}

pub(crate) fn non_negative(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(v),
        _ => Err("expected a finite number, 0 or more".into()),
    }
}

/// `N:D`, two whole numbers above 0.
fn ratio(text: &str) -> Result<(u32, u32), String> {
    let whole = |s: &str| s.parse::<u32>().ok().filter(|&n| n > 0);
    text.split_once(':')
        .and_then(|(n, d)| Some((whole(n)?, whole(d)?)))
        .ok_or_else(|| "expected two whole numbers above 0, as 1:100".into())
}

/// `X0,Y0,X1,Y1`, four finite numbers naming two different points.
fn segment(text: &str) -> Result<[f64; 4], String> {
    const SHAPE: &str = "expected four finite numbers, as 0,0,3,4";
    let numbers: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()
        .ok_or(SHAPE)?;
    let [x0, y0, x1, y1] = numbers[..] else {
        return Err(SHAPE.into());
    };
    if (x0, y0) == (x1, y1) {
        return Err("the two points are one point, so there is no length to divide by".into());
    }
    Ok([x0, y0, x1, y1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parsers_take_what_the_help_promises_and_nothing_else() {
        assert_eq!(positive_finite("0.001"), Ok(0.001));
        for bad in ["0", "-1", "nan", "inf", "", "metre"] {
            assert!(positive_finite(bad).is_err(), "{bad:?}");
        }
        assert_eq!(non_negative("0"), Ok(0.0));
        assert!(non_negative("-0.5").is_err());
        assert_eq!(ratio("1:100"), Ok((1, 100)));
        for bad in ["1-100", "0:100", "1:0", "1:", ":2", "1.5:100", "a:b"] {
            assert!(ratio(bad).is_err(), "{bad:?}");
        }
        assert_eq!(segment("0,0,3,4"), Ok([0.0, 0.0, 3.0, 4.0]));
        assert_eq!(segment("-1, 2,3,-4"), Ok([-1.0, 2.0, 3.0, -4.0]));
        for bad in ["0,0,3", "0,0,3,4,5", "0,0,x,4", "1,1,1,1", "0,0,nan,4"] {
            assert!(segment(bad).is_err(), "{bad:?}");
        }
    }
}
