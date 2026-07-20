//! Bands (channel-axis) op family — the FIRST per-family Wave-2 lane and the
//! template later family waves copy (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md` bands
//! section).
//!
//! The core `src/bands.rs` exposes 24 `pub fn` that fold to twelve base ops;
//! those become **eight** `viprs` subcommands here (the `*_const` / `band{and,
//! or,eor}` / `extract_bands` twins collapse into a single command with a vector
//! arg or an enum selector, exactly as `OP_MAP.md` prescribes). Every command is
//! oracle class **EXACT** (integer-in / integer-out, decode-compare tol 0)
//! **except `bandmean`**, which is **BOUNDED-TOL** (≤1 LSB): the core floors the
//! per-pixel integer mean (truncating division) while vips rounds to nearest, so
//! a non-divisible band sum diverges by at most one least-significant bit (a
//! documented core-vs-vips rounding difference, not a CLI bug):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `bandjoin A B [C…] OUT`        | `bandjoin`       | S2 variadic | EXACT | pairwise concat fold |
//! | `bandjoin_const IN OUT "c…"`   | `bandjoin_const` | S1 | EXACT | append one band per constant |
//! | `bandfold IN OUT --factor`     | `bandfold`       | S1 | EXACT | fold width into bands |
//! | `bandunfold IN OUT --factor`   | `bandunfold`     | S1 | EXACT | unfold bands into width |
//! | `bandmean IN OUT`              | `bandmean`       | S1 | BOUNDED-TOL ≤1 LSB | mean; core floors vs vips round-to-nearest |
//! | `bandrank A B [C…] OUT --index`| `bandrank`       | S2 variadic | EXACT | per-sample rank statistic |
//! | `bandbool IN OUT and\|or\|eor`  | `bandbool`       | S1 | EXACT | fold bands with a bitwise op |
//! | `extract_band IN OUT BAND --n` | `extract_band`   | S1 | EXACT | one band (or `--n` consecutive) |
//!
//! Positional orders, flag names, enum spellings, and input value bounds mirror
//! vips 8.18.4 exactly (verified against `vips <op>` usage): `extract_band`'s
//! `BAND` is `>= 0` and `bandrank`'s `--index` is `>= -1`, matching vips's own
//! minima (no negative-from-end / sub-`-1` extensions). Handlers keep the §3
//! `load → try_op → save` shape and call only the panic-free `try_*` core APIs, so a bad input
//! becomes exit 1 rather than an abort (`CLI_CONTRACT.md` §8). The two variadic
//! commands (`bandjoin`, `bandrank`) go through [`io::inputs_and_out`], THE
//! shared S2 idiom this lane establishes for every later variadic family.
//
// @doc-command:begin name=bandjoin about="Join the bands of two or more images into one image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandjoin
// @doc-command:begin name=bandjoin_const about="Append one constant band per element of a vector to an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandjoin_const
// @doc-command:begin name=bandfold about="Fold the x axis into bands." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandfold
// @doc-command:begin name=bandunfold about="Unfold image bands into the x axis." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandunfold
// @doc-command:begin name=bandmean about="Average an image's bands into a single-band image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandmean
// @doc-command:begin name=bandrank about="Select a per-sample rank statistic across a set of images." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandrank
// @doc-command:begin name=bandbool about="Fold an image's bands with a bitwise boolean operation." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=bandbool
// @doc-command:begin name=extract_band about="Extract one band, or --n consecutive bands, from an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=extract_band

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::Raster;

use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "bandjoin",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "bandjoin_const",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "bandfold",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "bandunfold",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // EXACT (tol 0): core issue #482 made the per-pixel integer mean
            // round-to-nearest, matching vips 8.18.4 bit-for-bit (previously the
            // core floored via truncating division, diverging ≤1 LSB on a
            // non-divisible band sum). Verified against the oracle 2026-07-19.
            name: "bandmean",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "bandrank",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "bandbool",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "extract_band",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`]. The two variadic
/// commands declare a single trailing `num_args(2..)` positional — the legal
/// clap encoding of the vips `A B [C…] OUT` order (see [`io::inputs_and_out`]).
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("bandjoin")
                .about("Join the bands of two or more images into one image.")
                .arg(
                    Arg::new("INPUTS")
                        .required(true)
                        .num_args(2..)
                        .value_name("A B [C…] OUT")
                        .help("Two or more input images followed by the output path"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("bandjoin_const")
                .about("Append one constant band per element of a vector to an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("C")
                        .required(true)
                        .value_name("c…")
                        .help("Space-separated constants, one appended band each (e.g. \"10 20\")"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("bandfold")
                .about("Fold the x axis into bands.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("factor")
                        .long("factor")
                        .value_name("N")
                        .default_value("0")
                        .value_parser(value_parser!(u32))
                        .help("Fold by this factor (0 = fold the whole width into bands)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("bandunfold")
                .about("Unfold image bands into the x axis.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("factor")
                        .long("factor")
                        .value_name("N")
                        .default_value("0")
                        .value_parser(value_parser!(u32))
                        .help("Unfold by this factor (0 = unfold every band into the width)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("bandmean")
                .about("Average an image's bands into a single-band image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("bandrank")
                .about("Select a per-sample rank statistic across a set of images.")
                .arg(
                    Arg::new("INPUTS")
                        .required(true)
                        .num_args(2..)
                        .value_name("A B [C…] OUT")
                        .help("Two or more input images followed by the output path"),
                )
                .arg(
                    Arg::new("index")
                        .long("index")
                        .value_name("I")
                        .default_value("-1")
                        // vips's own minimum is -1 (= median); reject anything
                        // below it rather than silently folding sub-`-1` values
                        // into the median (B5 strict parity).
                        .value_parser(value_parser!(i64).range(-1..))
                        .help(
                            "Sorted-list index to select (>= -1: 0 = min, count-1 = max, \
                             -1 = median, the default)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("bandbool")
                .about("Fold an image's bands with a bitwise boolean operation.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("and|or|eor")
                        .value_parser(["and", "or", "eor"])
                        .help(
                            "Bitwise operation to fold the bands with \
                             (and|or|eor; vips's lshift/rshift are intentionally \
                             excluded — not core-backed)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("extract_band")
                .about("Extract one band, or --n consecutive bands, from an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("BAND")
                        .required(true)
                        // vips's minimum is 0; reject negatives rather than
                        // offering a negative-from-end extension vips lacks
                        // (B5 strict parity).
                        .value_parser(value_parser!(i64).range(0..))
                        .help("First band to extract (>= 0)"),
                )
                .arg(
                    Arg::new("n")
                        .long("n")
                        .value_name("N")
                        .default_value("1")
                        .value_parser(value_parser!(u32))
                        .help("Number of consecutive bands to extract"),
                ),
        ),
    ]
}

/// Dispatch a matched bands subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "bandjoin" => run_bandjoin(m),
        "bandjoin_const" => run_bandjoin_const(m),
        "bandfold" => run_bandfold(m),
        "bandunfold" => run_bandunfold(m),
        "bandmean" => run_bandmean(m),
        "bandrank" => run_bandrank(m),
        "bandbool" => run_bandbool(m),
        "extract_band" => run_extract_band(m),
        other => bail!("bands family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Parse a vips-style space-separated numeric vector (e.g. `"10 20 30"`).
fn parse_f64_vec(s: &str) -> Result<Vec<f64>> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| {
            t.parse::<f64>()
                .map_err(|e| anyhow!("constant {t:?} is not a number: {e}"))
        })
        .collect::<Result<_>>()?;
    if v.is_empty() {
        bail!("expected at least one constant (a space-separated vector like \"10 20\")");
    }
    Ok(v)
}

/// `bandjoin A B [C…] OUT` — S2 variadic; pairwise-concat fold over the inputs.
fn run_bandjoin(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let (inputs, out_path) = io::inputs_and_out(m, "INPUTS")?;

    // @doc-snippet:begin command=bandjoin slot=load imports=decode_file
    let mut acc = io::load(&inputs[0], &limits)?;
    // @doc-snippet:end command=bandjoin slot=load

    // @doc-snippet:begin command=bandjoin slot=apply
    // @doc-test: bands.rs::ported_bandjoin_flow:1196 repo=libviprs
    for path in &inputs[1..] {
        let next = io::load(path, &limits)?;
        acc = acc.try_bandjoin(&next)?;
    }
    // @doc-snippet:end command=bandjoin slot=apply

    // @doc-snippet:begin command=bandjoin slot=save imports=save_file
    io::save(&acc, &out_path)?;
    // @doc-snippet:end command=bandjoin slot=save
    Ok(())
}

/// `bandjoin_const IN OUT "c…"` — S1; append one constant band per element.
fn run_bandjoin_const(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let consts = parse_f64_vec(pos(m, "C"))?;

    // @doc-snippet:begin command=bandjoin_const slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=bandjoin_const slot=load

    // @doc-snippet:begin command=bandjoin_const slot=apply
    // @doc-test: bands.rs::bandjoin_vec_appends_multiple_bands:792 repo=libviprs
    let out = raster.try_bandjoin_vec(&consts)?;
    // @doc-snippet:end command=bandjoin_const slot=apply

    // @doc-snippet:begin command=bandjoin_const slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=bandjoin_const slot=save
    Ok(())
}

/// `bandfold IN OUT --factor N` — S1; fold the x axis into bands.
fn run_bandfold(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let factor = factor_arg(m);

    // @doc-snippet:begin command=bandfold slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=bandfold slot=load

    // @doc-snippet:begin command=bandfold slot=apply
    // @doc-test: bands.rs::bandfold_factor_two:829 repo=libviprs
    let out = raster.try_bandfold(factor)?;
    // @doc-snippet:end command=bandfold slot=apply

    // @doc-snippet:begin command=bandfold slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=bandfold slot=save
    Ok(())
}

/// `bandunfold IN OUT --factor N` — S1; unfold bands into the x axis.
fn run_bandunfold(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let factor = factor_arg(m);

    // @doc-snippet:begin command=bandunfold slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=bandunfold slot=load

    // @doc-snippet:begin command=bandunfold slot=apply
    // @doc-test: bands.rs::bandunfold_factor_two:882 repo=libviprs
    let out = raster.try_bandunfold(factor)?;
    // @doc-snippet:end command=bandunfold slot=apply

    // @doc-snippet:begin command=bandunfold slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=bandunfold slot=save
    Ok(())
}

/// `bandmean IN OUT` — S1; per-pixel mean across bands.
fn run_bandmean(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=bandmean slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=bandmean slot=load

    // @doc-snippet:begin command=bandmean slot=apply
    // @doc-test: bands.rs::bandmean_floors_integer_mean:922 repo=libviprs
    let out = raster.try_bandmean()?;
    // @doc-snippet:end command=bandmean slot=apply

    // @doc-snippet:begin command=bandmean slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=bandmean slot=save
    Ok(())
}

/// `bandrank A B [C…] OUT --index I` — S2 variadic; per-sample rank statistic.
fn run_bandrank(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let (inputs, out_path) = io::inputs_and_out(m, "INPUTS")?;
    // vips default index is -1 (= median). With the `-1..` value_parser the only
    // negative clap admits is -1, which maps to the core's `None` (median); a
    // non-negative index selects that sorted position. A positive index beyond
    // `u32::MAX` is a typed exit-1 error — NOT a `u32::try_from` panic/abort
    // (exit 101), per CLI_CONTRACT.md §8.
    let index_raw = *m.get_one::<i64>("index").expect("clap default -1");
    let index: Option<u32> = if index_raw < 0 {
        None
    } else {
        Some(
            u32::try_from(index_raw)
                .map_err(|_| anyhow!("--index {index_raw} out of range (max {})", u32::MAX))?,
        )
    };

    // @doc-snippet:begin command=bandrank slot=load imports=decode_file
    let rasters: Vec<Raster> = inputs
        .iter()
        .map(|p| io::load(p, &limits))
        .collect::<Result<_>>()?;
    // @doc-snippet:end command=bandrank slot=load

    // @doc-snippet:begin command=bandrank slot=apply
    // @doc-test: bands.rs::bandrank_median_of_three:977 repo=libviprs
    let (first, rest) = rasters
        .split_first()
        .expect("inputs_and_out guarantees at least one input");
    let others: Vec<&Raster> = rest.iter().collect();
    let out = first.try_bandrank(&others, index)?;
    // @doc-snippet:end command=bandrank slot=apply

    // @doc-snippet:begin command=bandrank slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=bandrank slot=save
    Ok(())
}

/// `bandbool IN OUT and|or|eor` — S1; fold bands with a bitwise op.
fn run_bandbool(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let op = pos(m, "OP");

    // @doc-snippet:begin command=bandbool slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=bandbool slot=load

    // @doc-snippet:begin command=bandbool slot=apply
    // @doc-test: bands.rs::bandbool_family_reduces_bands:1047 repo=libviprs
    let out = match op {
        "and" => raster.try_bandand()?,
        "or" => raster.try_bandor()?,
        "eor" => raster.try_bandeor()?,
        other => bail!("unknown bandbool operation {other:?} (expected and|or|eor)"),
    };
    // @doc-snippet:end command=bandbool slot=apply

    // @doc-snippet:begin command=bandbool slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=bandbool slot=save
    Ok(())
}

/// `extract_band IN OUT BAND --n N` — S1; extract one or `--n` consecutive bands.
fn run_extract_band(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let band = *m.get_one::<i64>("BAND").expect("required positional");
    let n = *m.get_one::<u32>("n").expect("clap default 1");

    // @doc-snippet:begin command=extract_band slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=extract_band slot=load

    // @doc-snippet:begin command=extract_band slot=apply
    // @doc-test: bands.rs::extract_bands_middle_slice:1143 repo=libviprs
    // A single `extract_band BAND` is `--n 1`; both go through the range API so
    // the `--n` case is one code path (mirrors vips's single `extract_band` op).
    let out = raster.try_extract_bands(band, n)?;
    // @doc-snippet:end command=extract_band slot=apply

    // @doc-snippet:begin command=extract_band slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=extract_band slot=save
    Ok(())
}

/// Map the `--factor` flag (vips default `0` = whole) to the core's
/// `Option<u32>` (`None` = whole width / all bands).
fn factor_arg(m: &ArgMatches) -> Option<u32> {
    match *m.get_one::<u32>("factor").expect("clap default 0") {
        0 => None,
        v => Some(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str) -> Command {
        commands()
            .into_iter()
            .find(|c| c.get_name() == name)
            .unwrap_or_else(|| panic!("no command {name}"))
    }

    #[test]
    fn commands_and_metas_agree() {
        let cmd_names: Vec<String> = commands()
            .iter()
            .map(|c| c.get_name().to_string())
            .collect();
        let meta_names: Vec<&str> = metas().iter().map(|m| m.name).collect();
        for name in &meta_names {
            assert!(
                cmd_names.iter().any(|c| c == name),
                "meta {name} has no command"
            );
        }
        assert_eq!(cmd_names.len(), meta_names.len());
        assert_eq!(meta_names.len(), 8, "the bands family has eight commands");
    }

    #[test]
    fn bandjoin_splits_variadic_into_inputs_and_out() {
        // S2: `A B C OUT` -> inputs [A,B,C], out OUT.
        let m = cmd("bandjoin")
            .try_get_matches_from(["bandjoin", "a.png", "b.png", "c.png", "out.png"])
            .unwrap();
        let (inputs, out) = io::inputs_and_out(&m, "INPUTS").unwrap();
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
    fn bandjoin_requires_at_least_two_positionals() {
        // One value cannot be split into an input + an output.
        assert!(
            cmd("bandjoin")
                .try_get_matches_from(["bandjoin", "only.png"])
                .is_err(),
            "num_args(2..) must reject a single positional"
        );
    }

    #[test]
    fn bandrank_parses_inputs_and_index_flag() {
        let m = cmd("bandrank")
            .try_get_matches_from([
                "bandrank", "a.png", "b.png", "c.png", "out.png", "--index", "0",
            ])
            .unwrap();
        let (inputs, out) = io::inputs_and_out(&m, "INPUTS").unwrap();
        assert_eq!(inputs.len(), 3);
        assert_eq!(out, PathBuf::from("out.png"));
        assert_eq!(*m.get_one::<i64>("index").unwrap(), 0);
    }

    #[test]
    fn bandrank_index_defaults_to_median() {
        let m = cmd("bandrank")
            .try_get_matches_from(["bandrank", "a.png", "b.png", "out.png"])
            .unwrap();
        // Default -1 maps to the core's median (None).
        assert_eq!(*m.get_one::<i64>("index").unwrap(), -1);
    }

    #[test]
    fn bandrank_huge_index_is_error_not_panic() {
        // A positive --index beyond u32::MAX parses (the `-1..` bound only floors
        // the value) but must convert to a typed exit-1 error, NEVER a
        // u32::try_from panic/abort (exit 101) — CLI_CONTRACT.md §8. The index is
        // range-checked before any image is loaded, so dummy paths are fine.
        let m = cmd("bandrank")
            .try_get_matches_from([
                "bandrank",
                "a.png",
                "b.png",
                "out.png",
                "--index",
                "4294967296", // u32::MAX + 1
            ])
            .unwrap();
        let err = run_bandrank(&m).unwrap_err();
        assert!(
            err.to_string().contains("out of range"),
            "expected a typed 'out of range' error, got: {err}"
        );
    }

    #[test]
    fn bandrank_index_below_minus_one_is_rejected() {
        // vips's minimum index is -1; the `-1..` value_parser rejects anything
        // lower at parse time (B5 strict parity — no sub-`-1` extension).
        assert!(
            cmd("bandrank")
                .try_get_matches_from(["bandrank", "a.png", "out.png", "--index", "-2"])
                .is_err(),
            "an --index below -1 must be rejected"
        );
    }

    #[test]
    fn extract_band_rejects_a_negative_band() {
        // vips's minimum band is 0; the `0..` value_parser rejects negatives at
        // parse time (B5 strict parity — no negative-from-end extension).
        assert!(
            cmd("extract_band")
                .try_get_matches_from(["extract_band", "in.png", "out.png", "-1"])
                .is_err(),
            "a negative BAND must be rejected"
        );
    }

    #[test]
    fn bandbool_enforces_the_enum() {
        assert!(
            cmd("bandbool")
                .clone()
                .try_get_matches_from(["bandbool", "in.png", "out.png", "and"])
                .is_ok()
        );
        assert!(
            cmd("bandbool")
                .try_get_matches_from(["bandbool", "in.png", "out.png", "lshift"])
                .is_err(),
            "only and|or|eor are core-backed; vips's lshift/rshift are rejected"
        );
    }

    #[test]
    fn extract_band_parses_band_and_n() {
        let m = cmd("extract_band")
            .try_get_matches_from(["extract_band", "in.png", "out.png", "1", "--n", "2"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(*m.get_one::<i64>("BAND").unwrap(), 1);
        assert_eq!(*m.get_one::<u32>("n").unwrap(), 2);
    }

    #[test]
    fn extract_band_n_defaults_to_one() {
        let m = cmd("extract_band")
            .try_get_matches_from(["extract_band", "in.png", "out.png", "0"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("n").unwrap(), 1);
    }

    #[test]
    fn factor_zero_maps_to_whole() {
        let m = cmd("bandfold")
            .try_get_matches_from(["bandfold", "in.png", "out.png"])
            .unwrap();
        // vips default 0 -> core None (fold whole width).
        assert_eq!(factor_arg(&m), None);
        let m = cmd("bandfold")
            .try_get_matches_from(["bandfold", "in.png", "out.png", "--factor", "3"])
            .unwrap();
        assert_eq!(factor_arg(&m), Some(3));
    }

    #[test]
    fn parse_f64_vec_reads_a_space_separated_vector() {
        assert_eq!(parse_f64_vec("10 20 30").unwrap(), vec![10.0, 20.0, 30.0]);
        assert!(parse_f64_vec("   ").is_err(), "an empty vector is an error");
        assert!(parse_f64_vec("10 x").is_err(), "a non-number is an error");
    }
}
