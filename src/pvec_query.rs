//! `viprs pvec pick` and `viprs pvec tessellate` (libviprs-cli#154, CV2.5 of EPIC CV): the facade's
//! `PickIndex` and `Tessellate` over the entities of a `.pvec`.
//!
//! * `viprs pvec pick FILE (--point X,Y --tolerance T | --rect X0,Y0,X1,Y1 [--mode M] | --nearest X,Y
//!   [--tolerance D]) [--json]`: the entity under a point, the entities a rectangle selects, or the
//!   nearest entity, all in source units.
//! * `viprs pvec tessellate FILE (--error E | --error-px N --units-per-pixel U) [--max-segments N]
//!   [--json]`: every curve as a polyline inside an error budget, like PVec's own `pvec tessellate`.
//!
//! As with the other read commands, this file holds the arguments and every refusal the command
//! line decides by itself (exit 2) and runs in every build; the facade calls are in `imp`, behind
//! `pvec-raster`, and `pvec_raster::run` dispatches to them. A build without the feature exits 1
//! naming it.

use std::path::PathBuf;

use clap::{ArgGroup, Args, ValueEnum};

use crate::ops::foreign::positive_finite;
use crate::pvec_read::non_negative;

#[cfg(feature = "pvec-raster")]
pub(crate) mod imp;

/// How a rectangle selects entities.
#[derive(Clone, Copy, ValueEnum)]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) enum Mode {
    /// Entities whose bounds meet the rectangle, touching included.
    Intersects,
    /// Entities whose bounds lie inside the rectangle.
    Contains,
}

/// `viprs pvec pick`.
#[derive(Args)]
#[command(group(
    ArgGroup::new("query")
        .required(true)
        .args(["point", "rect", "nearest"]),
))]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct PickArgs {
    /// The `.pvec` to query.
    pub(crate) file: PathBuf,

    /// The entity within `--tolerance` of this point, `X,Y` in source units: the nearest one.
    #[arg(
        long,
        value_name = "X,Y",
        requires = "tolerance",
        allow_hyphen_values = true,
        value_parser = pair
    )]
    pub(crate) point: Option<[f64; 2]>,

    /// The entities a rectangle selects, `X0,Y0,X1,Y1` in source units, one id per line.
    #[arg(
        long,
        value_name = "X0,Y0,X1,Y1",
        allow_hyphen_values = true,
        value_parser = rectangle
    )]
    pub(crate) rect: Option<[f64; 4]>,

    /// What a rectangle selects: bounds that meet it (the default) or lie inside it.
    #[arg(long, value_enum, conflicts_with_all = ["point", "nearest"])]
    pub(crate) mode: Option<Mode>,

    /// The nearest entity to this point, `X,Y` in source units, wherever it is (or within
    /// `--tolerance`).
    #[arg(
        long,
        value_name = "X,Y",
        allow_hyphen_values = true,
        value_parser = pair
    )]
    pub(crate) nearest: Option<[f64; 2]>,

    /// The pick radius in source units for `--point`, which needs it; the largest distance for
    /// `--nearest`, which is unlimited without it.
    #[arg(
        long,
        value_name = "DISTANCE",
        conflicts_with = "rect",
        allow_negative_numbers = true,
        value_parser = non_negative
    )]
    pub(crate) tolerance: Option<f64>,

    /// Print one JSON document (`"v": 1`) instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

/// `viprs pvec tessellate`.
#[derive(Args)]
#[command(group(
    ArgGroup::new("budget")
        .required(true)
        .args(["error", "error_px"]),
))]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct TessellateArgs {
    /// The `.pvec` to tessellate.
    pub(crate) file: PathBuf,

    /// The largest deviation from the exact curve, in source units.
    #[arg(
        long,
        value_name = "UNITS",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(crate) error: Option<f64>,

    /// The largest deviation on screen, in pixels; needs `--units-per-pixel`.
    #[arg(
        long,
        value_name = "PIXELS",
        requires = "units_per_pixel",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(crate) error_px: Option<f64>,

    /// Source units one screen pixel covers, which turns `--error-px` into a budget.
    #[arg(
        long,
        value_name = "UNITS",
        conflicts_with = "error",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(crate) units_per_pixel: Option<f64>,

    /// The most segments one curve may have; a budget it cannot reach in that many is exit 1.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 65_536,
        allow_negative_numbers = true,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub(crate) max_segments: u64,

    /// Print one JSON document (`"v": 1`) instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

/// Four or two finite numbers separated by commas.
fn numbers(text: &str, count: usize, shape: &str) -> Result<Vec<f64>, String> {
    let parsed: Option<Vec<f64>> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect();
    parsed
        .filter(|v| v.len() == count)
        .ok_or_else(|| format!("expected {shape}"))
}

/// `X,Y`, two finite numbers.
fn pair(text: &str) -> Result<[f64; 2], String> {
    let n = numbers(text, 2, "two finite numbers, as 100,250")?;
    Ok([n[0], n[1]])
}

/// `X0,Y0,X1,Y1`, four finite numbers with the minimum corner first.
fn rectangle(text: &str) -> Result<[f64; 4], String> {
    let n = numbers(text, 4, "four finite numbers, as 0,0,400,300")?;
    if n[0] > n[2] || n[1] > n[3] {
        return Err("the first corner must be the minimum one (X0 <= X1 and Y0 <= Y1)".into());
    }
    Ok([n[0], n[1], n[2], n[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parsers_take_what_the_help_promises_and_nothing_else() {
        assert_eq!(pair("1, -2.5"), Ok([1.0, -2.5]));
        for bad in ["1", "1,2,3", "x,2", "nan,2", "1,inf", ""] {
            assert!(pair(bad).is_err(), "{bad:?}");
        }
        assert_eq!(rectangle("0,0,4,3"), Ok([0.0, 0.0, 4.0, 3.0]));
        assert_eq!(rectangle("2,2,2,2"), Ok([2.0, 2.0, 2.0, 2.0]));
        for bad in ["0,0,1", "5,5,1,1", "0,5,1,1", "a,0,1,1"] {
            assert!(rectangle(bad).is_err(), "{bad:?}");
        }
    }
}
