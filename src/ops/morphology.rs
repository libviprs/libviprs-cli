//! Morphology op family — the Wave-1 **reference** family
//! (`CLI_CONTRACT.md` §10, `OP_MAP.md` morphology section).
//!
//! Five vips ops collapse into four `viprs` subcommands, covering four of the
//! six §3 command shapes in one small family:
//!
//! | command | vips | shape | oracle |
//! |---|---|---|---|
//! | `morph IN OUT MASK.mat erode\|dilate` | `morph` | S1 image→image | EXACT |
//! | `rank IN OUT WIDTH HEIGHT INDEX`      | `rank`  | S1 image→image | EXACT |
//! | `countlines IN horizontal\|vertical`  | `countlines` | S3 image→stdout-scalar | EXACT |
//! | `labelregions IN MASK_OUT`            | `labelregions` | S4 image→two-outputs | EXACT |
//!
//! Positional orders mirror vips exactly. Every handler keeps the §3
//! `load → try_op → save` shape and calls only the panic-free `try_*` core
//! APIs. This is the **annotated** reference: the SCHEMA_V2 `@doc-command` /
//! `@doc-snippet` / `@doc-flag` / `@doc-test` markers below let the docs
//! generator extract per-command examples (`libviprs-org/cli/SCHEMA_V2.md` §1).
//!
//! The four `@doc-command` blocks declare each command's slot order and import
//! base once; the handlers open the matching snippets inline.
//
// @doc-command:begin name=morph about="Erode or dilate a binary image with a morphological mask." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=morph
// @doc-command:begin name=rank about="Rank (order-statistic) filter over a sliding window." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=rank
// @doc-command:begin name=countlines about="Count the average number of lines in a one-band image and print it." \
//     slot-order=load,apply imports-base=decode_file
// @doc-command:end name=countlines
// @doc-command:begin name=labelregions about="Label 4-connected regions; write the label mask and print the segment count." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=labelregions

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use libviprs::Direction;

use super::matfile::MatFile;
use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "morph",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "rank",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "countlines",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "labelregions",
            shape: Shape::TwoOutputs,
            oracle_class: OracleClass::Exact,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads an image, so each carries the shared decode-limit flags
/// via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("morph")
                .about("Erode or dilate a binary image with a morphological mask.")
                .arg(Arg::new("IN").required(true).help("Input image (8-bit)"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("MASK")
                        .required(true)
                        .value_name("MASK.mat")
                        .help("Structuring element as a vips text matrix (values 0/128/255)"),
                )
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("erode|dilate")
                        .value_parser(["erode", "dilate"])
                        .help("Morphological operation"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("rank")
                .about("Rank (order-statistic) filter over a sliding window.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (unsigned 8/16-bit)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("WIDTH")
                        .required(true)
                        .value_parser(clap::value_parser!(u32))
                        .help("Window width in pixels"),
                )
                .arg(
                    Arg::new("HEIGHT")
                        .required(true)
                        .value_parser(clap::value_parser!(u32))
                        .help("Window height in pixels"),
                )
                .arg(
                    Arg::new("INDEX")
                        .required(true)
                        .value_parser(clap::value_parser!(u32))
                        .help(
                            "Sorted-window index (0 = min, width*height-1 = max, middle = median)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("countlines")
                .about("Count the average number of lines in a one-band image and print it.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (one band, unsigned 8/16-bit)"),
                )
                .arg(
                    Arg::new("DIRECTION")
                        .required(true)
                        .value_name("horizontal|vertical")
                        .value_parser(["horizontal", "vertical"])
                        .help("Line direction to count"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("labelregions")
                .about(
                    "Label 4-connected regions; write the label mask and print the segment count.",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("MASK_OUT")
                        .required(true)
                        .help("Output path for the Gray16 label mask"),
                ),
        ),
    ]
}

/// Dispatch a matched morphology subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "morph" => run_morph(m),
        "rank" => run_rank(m),
        "countlines" => run_countlines(m),
        "labelregions" => run_labelregions(m),
        other => bail!("morphology family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// `morph IN OUT MASK.mat erode|dilate` — S1 image→image, mask via matfile.
fn run_morph(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let mask_path = PathBuf::from(pos(m, "MASK"));
    let op = pos(m, "OP");

    // @doc-snippet:begin command=morph slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=morph slot=load

    let mask = MatFile::load(&mask_path)?;
    let mask_rows = mask.as_u8_mask()?;
    let mask_refs: Vec<&[u8]> = mask_rows.iter().map(|r| r.as_slice()).collect();

    // @doc-snippet:begin command=morph slot=apply
    // @doc-test: morphology.rs::erode_square_exact:659 repo=libviprs
    let out = match op {
        "erode" => raster.try_erode(&mask_refs)?, // @doc-flag: mask command=morph kind=param param_name=mask
        "dilate" => raster.try_dilate(&mask_refs)?,
        other => bail!("unknown morph operation {other:?} (expected erode|dilate)"),
    };
    // @doc-snippet:end command=morph slot=apply

    // @doc-snippet:begin command=morph slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=morph slot=save
    Ok(())
}

/// `rank IN OUT WIDTH HEIGHT INDEX` — S1 image→image order statistic.
fn run_rank(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");
    let index = *m.get_one::<u32>("INDEX").expect("required");

    // @doc-snippet:begin command=rank slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=rank slot=load

    // @doc-snippet:begin command=rank slot=apply
    // @doc-test: morphology.rs::rank_order_statistics_exact:794 repo=libviprs
    let out = raster.try_rank(width, height, index)?;
    // @doc-snippet:end command=rank slot=apply

    // @doc-snippet:begin command=rank slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=rank slot=save
    Ok(())
}

/// `countlines IN horizontal|vertical` — S3 image→stdout-scalar.
fn run_countlines(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let direction = match pos(m, "DIRECTION") {
        "horizontal" => Direction::Horizontal,
        "vertical" => Direction::Vertical,
        other => bail!("unknown direction {other:?} (expected horizontal|vertical)"),
    };

    // @doc-snippet:begin command=countlines slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=countlines slot=load

    // @doc-snippet:begin command=countlines slot=apply
    // @doc-test: morphology.rs::countlines_single_horizontal:858 repo=libviprs
    let nolines = raster.try_countlines(direction)?;
    // @doc-snippet:end command=countlines slot=apply

    println!("{}", fmt_vips_double(nolines));
    Ok(())
}

/// `labelregions IN MASK_OUT` — S4 image→two-outputs (mask + printed count).
fn run_labelregions(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let mask_out = PathBuf::from(pos(m, "MASK_OUT"));

    // @doc-snippet:begin command=labelregions slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=labelregions slot=load

    // @doc-snippet:begin command=labelregions slot=apply
    // @doc-test: morphology.rs::label_regions_circle:935 repo=libviprs
    let (mask, segments) = raster.try_label_regions()?;
    // @doc-snippet:end command=labelregions slot=apply

    // @doc-snippet:begin command=labelregions slot=save imports=save_file
    io::save(&mask, &mask_out)?;
    // @doc-snippet:end command=labelregions slot=save
    println!("{segments}");
    Ok(())
}

/// Format a scalar in the vips numeric print format (`avg` prints
/// `100.000000`, `CLI_CONTRACT.md` §3). The differential harness float-parses
/// the value with an epsilon rather than comparing text; per-op numeric-format
/// parity is a §9 missing-scope item.
fn fmt_vips_double(v: f64) -> String {
    format!("{v:.6}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn morph_parses_positionals_in_vips_order() {
        let m = commands()
            .into_iter()
            .find(|c| c.get_name() == "morph")
            .unwrap()
            .try_get_matches_from(["morph", "in.png", "out.png", "cross.mat", "erode"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "MASK"), "cross.mat");
        assert_eq!(pos(&m, "OP"), "erode");
    }

    #[test]
    fn morph_rejects_bad_op() {
        let err = commands()
            .into_iter()
            .find(|c| c.get_name() == "morph")
            .unwrap()
            .try_get_matches_from(["morph", "in.png", "out.png", "cross.mat", "open"]);
        assert!(err.is_err(), "erode|dilate must be enforced by clap");
    }

    #[test]
    fn fmt_vips_double_matches_contract_example() {
        assert_eq!(fmt_vips_double(1.0), "1.000000");
        assert_eq!(fmt_vips_double(0.5), "0.500000");
    }
}
