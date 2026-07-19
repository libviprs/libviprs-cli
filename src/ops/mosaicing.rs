//! Mosaicing (image-assembly) op family — a Wave-2 per-family lane
//! (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md` mosaicing section).
//!
//! The core `src/mosaicing.rs` exposes six `pub fn` that fold to three base
//! ops; those become **three** `viprs` subcommands here, each named for its
//! exact vips 8.18.4 nickname:
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `merge REF SEC OUT DIRECTION DX DY`               | `merge`         | S2 (2-image→image) | EXACT | feathered raised-cosine blend, integer truncating division |
//! | `mosaic REF SEC OUT DIRECTION XREF YREF XSEC YSEC`| `mosaic`        | S2 (2-image→image) | EXACT | discrete tie-point search then `merge` |
//! | `globalbalance IN OUT`                            | `globalbalance` | S1 (image→image)   | GOLDEN-ONLY | brightness-balance a viprs-built mosaic |
//!
//! Positional orders, the `direction` enum spellings, and the numeric input
//! bounds mirror vips 8.18.4 exactly (verified against `vips merge` / `vips
//! mosaic` / `vips globalbalance` usage): `merge`'s `DX`/`DY` accept the vips
//! `gint` range `-100000000..=1000000000`; `mosaic`'s four tie-point positions
//! accept the vips `gint` range `0..=1000000000`. Handlers keep the §3
//! `load → try_op → save` shape and call only the panic-free `try_*` core APIs,
//! so a bad input becomes exit 1 (op failure) or exit 2 (usage) rather than a
//! process abort (`CLI_CONTRACT.md` §8).
//!
//! **`merge`/`mosaic` are EXACT (`OP_MAP.md`).** They are the two families
//! whose *discrete* tie-point search (`mosaic`) and integer feather ramp
//! (`merge`) must agree with vips bit-for-bit or the whole output diverges — the
//! differential doubles as the pin on that agreement (`CLI_CONTRACT.md` §7,
//! `cli_mosaicing_diff.rs`).
//!
//! **`globalbalance` is GOLDEN-ONLY (`OP_MAP.md`).** The core reads the
//! `mosaic-join-tree` metadata blob that only viprs `merge`/`mosaic` outputs
//! carry, whereas vips's own globalbalance parses its filename-based image
//! history — the two metadata channels are mutually unreadable, so **no vips
//! cross-oracle exists**. Its differential is a regression pin against a
//! committed viprs `merge → globalbalance` pipeline fixture (and the blob
//! survives the `.v` container's JSON trailer, so the two-invocation CLI
//! pipeline works). An input without the blob is a typed exit-1 error.
//!
//! ## `--mblend` subset (`merge`)
//!
//! vips's `merge` exposes `--mblend` (max blend width, default 10). The core's
//! public `try_merge` fixes the blend width at the libvips default of 10 (its
//! rustdoc: "`mblend` is fixed at the libvips default of 10 on the public
//! surface"); there is no core API to vary it. The flag is therefore surfaced
//! for vips parity with its default of 10, but any *other* value is a typed
//! exit-1 error rather than a silently-ignored flag (the `add` 16-bit lesson:
//! never emit a wrong "success" for an input the core cannot honour). The
//! differential only ever uses the default.
//!
//! **Deliberate divergence from the oracle (adversarial-review finding 2):**
//! `vips merge --mblend N` *succeeds* and produces a valid, different blend for
//! any in-range N, whereas `viprs merge --mblend N` (N != 10) *exits 1*. This is
//! a real, intentional CLI-surface divergence — the loud-fail is correct, but it
//! means no differential can (or does) cover what vips actually does at a
//! non-default mblend, since the core cannot reproduce it. Flagged here and in
//! `PROVENANCE.md` so a parity auditor is not surprised. (Note the asymmetry with
//! `mosaic`, which omits its own `--mblend`/`--hwindow`/`--harea`/`--bandno`
//! entirely rather than rejecting non-defaults; both are safe subsets.)
//
// @doc-command:begin name=merge about="Merge two overlapping images with a feathered seam." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=merge
// @doc-command:begin name=mosaic about="Join two images by refining an approximate tie-point." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=mosaic
// @doc-command:begin name=globalbalance about="Balance the brightness of an assembled viprs mosaic." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=globalbalance

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::MergeDirection;

use super::{CommandMeta, OracleClass, Shape, io};

/// vips `gint` minimum for `merge`'s `DX`/`DY` (`vips merge` usage).
const MERGE_DISP_MIN: i64 = -100_000_000;
/// vips `gint` maximum for `merge`'s `DX`/`DY` and `mosaic`'s positions.
const GINT_MAX: i64 = 1_000_000_000;
/// vips `gint` minimum for `mosaic`'s tie-point positions (`vips mosaic` usage).
const POS_MIN: i64 = 0;
/// The libvips default max blend width the core public surface fixes (`mblend`).
const DEFAULT_MBLEND: i32 = 10;
/// vips `mblend` bounds (`vips merge` usage: min 0, max 10000).
const MBLEND_MIN: i64 = 0;
const MBLEND_MAX: i64 = 10_000;

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "merge",
            // 2-image→image (§3.2): REF + SEC + OUT, not a variadic fold, but
            // the same N-image→image shape class in the OP_MAP.
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "mosaic",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // GOLDEN-ONLY: no vips cross-oracle (the core reads the viprs
            // join-tree blob; vips reads its own image history). See the header.
            name: "globalbalance",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::GoldenOnly,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`]. `merge`/`mosaic` use
/// **fixed named positionals** (not the variadic [`io::inputs_and_out`] idiom):
/// vips places `OUT` as the *third* positional, `merge REF SEC OUT …`, so a
/// trailing-`num_args` split would mis-parse the vips order.
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("merge")
                .about("Merge two overlapping images with a feathered seam.")
                // DX/DY are signed displacements (a left-right join has a
                // negative DX = overlap − REF.width), so a leading `-` is a value,
                // not an unknown flag.
                .allow_negative_numbers(true)
                .arg(
                    Arg::new("REF")
                        .required(true)
                        .help("Reference image (placed at the origin)"),
                )
                .arg(
                    Arg::new("SEC")
                        .required(true)
                        .help("Secondary image (displaced by DX/DY)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("DIRECTION")
                        .required(true)
                        .value_name("horizontal|vertical")
                        .value_parser(["horizontal", "vertical"])
                        .help("Join direction (REF on the left/top respectively)"),
                )
                .arg(
                    Arg::new("DX")
                        .required(true)
                        // vips `gint` range for the horizontal displacement.
                        .value_parser(value_parser!(i32).range(MERGE_DISP_MIN..=GINT_MAX))
                        .help("Horizontal displacement from SEC to REF"),
                )
                .arg(
                    Arg::new("DY")
                        .required(true)
                        .value_parser(value_parser!(i32).range(MERGE_DISP_MIN..=GINT_MAX))
                        .help("Vertical displacement from SEC to REF"),
                )
                .arg(
                    Arg::new("mblend")
                        .long("mblend")
                        .value_name("N")
                        .default_value("10")
                        // vips's own bounds; the core, however, only supports the
                        // default 10 (see the module header) — a non-default value
                        // is a typed exit-1 error, not a silently-ignored flag.
                        .value_parser(value_parser!(i32).range(MBLEND_MIN..=MBLEND_MAX))
                        .help(
                            "Maximum blend width (vips default 10; the core fixes this at 10, \
                             so any other value is rejected)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            // `mosaic` intentionally omits vips's four OPTIONAL flags
            // (`--hwindow` / `--harea` / `--mblend` / `--bandno`): the core
            // `try_mosaic` pins each to the vips default (hwindow 5, harea 15,
            // mblend 10, bandno 0) and exposes no API to vary them, so surfacing
            // them could only reject a non-default value — a safe, documented
            // subset of the vips surface, not a parity gap (finding 1).
            Command::new("mosaic")
                .about("Join two images by refining an approximate tie-point.")
                .arg(
                    Arg::new("REF")
                        .required(true)
                        .help("Reference image (placed at the origin)"),
                )
                .arg(Arg::new("SEC").required(true).help("Secondary image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("DIRECTION")
                        .required(true)
                        .value_name("horizontal|vertical")
                        .value_parser(["horizontal", "vertical"])
                        .help("Join direction (REF on the left/top respectively)"),
                )
                .arg(pos_arg("XREF", "Reference tie-point x (in REF)"))
                .arg(pos_arg("YREF", "Reference tie-point y (in REF)"))
                .arg(pos_arg("XSEC", "Secondary tie-point x (in SEC)"))
                .arg(pos_arg("YSEC", "Secondary tie-point y (in SEC)")),
        ),
        io::with_decode_limit_args(
            Command::new("globalbalance")
                .about("Balance the brightness of an assembled viprs mosaic.")
                .arg(Arg::new("IN").required(true).help(
                    "A mosaic built by `viprs merge`/`viprs mosaic` (carries join-tree metadata)",
                ))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output image (always float — write .v)"),
                ),
        ),
    ]
}

/// A required tie-point-position positional with the vips `gint` bounds
/// (`0..=1000000000`), shared by `mosaic`'s four positions.
fn pos_arg(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .required(true)
        .value_parser(value_parser!(i32).range(POS_MIN..=GINT_MAX))
        .help(help)
}

/// Dispatch a matched mosaicing subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "merge" => run_merge(m),
        "mosaic" => run_mosaic(m),
        "globalbalance" => run_globalbalance(m),
        other => bail!("mosaicing family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Map the `horizontal|vertical` positional (clap-enforced) to the core enum.
fn direction(m: &ArgMatches) -> Result<MergeDirection> {
    match pos(m, "DIRECTION") {
        "horizontal" => Ok(MergeDirection::Horizontal),
        "vertical" => Ok(MergeDirection::Vertical),
        other => bail!("unknown direction {other:?} (expected horizontal|vertical)"),
    }
}

/// `merge REF SEC OUT DIRECTION DX DY [--mblend N]` — S2 feathered blend.
fn run_merge(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let ref_path = PathBuf::from(pos(m, "REF"));
    let sec_path = PathBuf::from(pos(m, "SEC"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let dir = direction(m)?;
    let dx = *m.get_one::<i32>("DX").expect("required positional");
    let dy = *m.get_one::<i32>("DY").expect("required positional");

    // The core public surface fixes mblend at the vips default 10 and offers no
    // API to vary it. Reject any other value with a typed exit-1 error rather
    // than silently ignoring it (never emit a wrong "success"; `add` 16-bit).
    let mblend = *m.get_one::<i32>("mblend").expect("clap default 10");
    if mblend != DEFAULT_MBLEND {
        bail!(
            "--mblend {mblend} is not supported: the core fixes the blend width at the \
             vips default {DEFAULT_MBLEND}"
        );
    }

    // @doc-snippet:begin command=merge slot=load imports=decode_file
    let reference = io::load(&ref_path, &limits)?;
    let secondary = io::load(&sec_path, &limits)?;
    // @doc-snippet:end command=merge slot=load

    // @doc-snippet:begin command=merge slot=apply
    // @doc-test: mosaicing.rs::lrmerge_blend_band_exact:1934 repo=libviprs
    let out = reference.try_merge(&secondary, dir, dx, dy)?;
    // @doc-snippet:end command=merge slot=apply

    // @doc-snippet:begin command=merge slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=merge slot=save
    Ok(())
}

/// `mosaic REF SEC OUT DIRECTION XREF YREF XSEC YSEC` — S2 tie-point join.
fn run_mosaic(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let ref_path = PathBuf::from(pos(m, "REF"));
    let sec_path = PathBuf::from(pos(m, "SEC"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let dir = direction(m)?;
    let xref = *m.get_one::<i32>("XREF").expect("required positional");
    let yref = *m.get_one::<i32>("YREF").expect("required positional");
    let xsec = *m.get_one::<i32>("XSEC").expect("required positional");
    let ysec = *m.get_one::<i32>("YSEC").expect("required positional");

    // @doc-snippet:begin command=mosaic slot=load imports=decode_file
    let reference = io::load(&ref_path, &limits)?;
    let secondary = io::load(&sec_path, &limits)?;
    // @doc-snippet:end command=mosaic slot=load

    // @doc-snippet:begin command=mosaic slot=apply
    // @doc-test: mosaicing.rs::lrmosaic_recovers_true_offset:2075 repo=libviprs
    // The discrete tie-point search runs entirely inside try_mosaic (the search
    // failure modes surface as typed errors → exit 1, never a panic).
    let out = reference.try_mosaic(&secondary, dir, xref, yref, xsec, ysec)?;
    // @doc-snippet:end command=mosaic slot=apply

    // @doc-snippet:begin command=mosaic slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=mosaic slot=save
    Ok(())
}

/// `globalbalance IN OUT` — S1; balance a viprs-built mosaic (GOLDEN-ONLY).
fn run_globalbalance(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=globalbalance slot=load imports=decode_file
    let mosaic = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=globalbalance slot=load

    // @doc-snippet:begin command=globalbalance slot=apply
    // @doc-test: mosaicing.rs::global_balance_two_image_mosaic:2141 repo=libviprs
    // Errors (no / corrupt join-tree metadata, unsolvable factor system) become
    // exit 1; the output is always float, so callers write a `.v` sink.
    let out = mosaic.try_global_balance()?;
    // @doc-snippet:end command=globalbalance slot=apply

    // @doc-snippet:begin command=globalbalance slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=globalbalance slot=save
    Ok(())
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
        assert_eq!(
            meta_names.len(),
            3,
            "the mosaicing family has three commands"
        );
    }

    #[test]
    fn merge_parses_positionals_in_vips_order() {
        let m = cmd("merge")
            .try_get_matches_from([
                "merge",
                "ref.png",
                "sec.png",
                "out.png",
                "horizontal",
                "-28",
                "0",
            ])
            .unwrap();
        assert_eq!(pos(&m, "REF"), "ref.png");
        assert_eq!(pos(&m, "SEC"), "sec.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "DIRECTION"), "horizontal");
        assert_eq!(*m.get_one::<i32>("DX").unwrap(), -28);
        assert_eq!(*m.get_one::<i32>("DY").unwrap(), 0);
        // Default mblend is the vips default 10.
        assert_eq!(*m.get_one::<i32>("mblend").unwrap(), 10);
    }

    #[test]
    fn merge_rejects_bad_direction() {
        assert!(
            cmd("merge")
                .try_get_matches_from([
                    "merge", "ref.png", "sec.png", "out.png", "diagonal", "0", "0",
                ])
                .is_err(),
            "horizontal|vertical must be enforced by clap"
        );
    }

    #[test]
    fn merge_rejects_dx_below_vips_minimum() {
        // vips's DX minimum is -100000000; anything lower is rejected at parse
        // time (strict bounds parity — no sub-minimum extension).
        assert!(
            cmd("merge")
                .try_get_matches_from([
                    "merge",
                    "ref.png",
                    "sec.png",
                    "out.png",
                    "horizontal",
                    "-100000001",
                    "0",
                ])
                .is_err(),
            "a DX below the vips minimum must be rejected"
        );
    }

    #[test]
    fn merge_nondefault_mblend_is_exit1_not_ignored() {
        // clap accepts any in-range mblend, but the handler rejects a non-default
        // value with a typed error (exit 1), never a silent success.
        let m = cmd("merge")
            .try_get_matches_from([
                "merge",
                "ref.png",
                "sec.png",
                "out.png",
                "horizontal",
                "0",
                "0",
                "--mblend",
                "5",
            ])
            .unwrap();
        let err = run_merge(&m).unwrap_err();
        assert!(
            err.to_string().contains("not supported"),
            "expected a mblend-unsupported error, got: {err}"
        );
    }

    #[test]
    fn merge_mblend_out_of_vips_range_is_rejected() {
        // vips's mblend max is 10000.
        assert!(
            cmd("merge")
                .try_get_matches_from([
                    "merge",
                    "ref.png",
                    "sec.png",
                    "out.png",
                    "horizontal",
                    "0",
                    "0",
                    "--mblend",
                    "10001",
                ])
                .is_err(),
            "an mblend above the vips maximum must be rejected"
        );
    }

    #[test]
    fn mosaic_parses_positionals_in_vips_order() {
        let m = cmd("mosaic")
            .try_get_matches_from([
                "mosaic", "ref.png", "sec.png", "out.png", "vertical", "8", "20", "2", "20",
            ])
            .unwrap();
        assert_eq!(pos(&m, "REF"), "ref.png");
        assert_eq!(pos(&m, "SEC"), "sec.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "DIRECTION"), "vertical");
        assert_eq!(*m.get_one::<i32>("XREF").unwrap(), 8);
        assert_eq!(*m.get_one::<i32>("YREF").unwrap(), 20);
        assert_eq!(*m.get_one::<i32>("XSEC").unwrap(), 2);
        assert_eq!(*m.get_one::<i32>("YSEC").unwrap(), 20);
    }

    #[test]
    fn mosaic_rejects_negative_tie_point() {
        // vips's tie-point minimum is 0; negatives are rejected at parse time.
        assert!(
            cmd("mosaic")
                .try_get_matches_from([
                    "mosaic",
                    "ref.png",
                    "sec.png",
                    "out.png",
                    "horizontal",
                    "-1",
                    "0",
                    "0",
                    "0",
                ])
                .is_err(),
            "a negative tie-point position must be rejected"
        );
    }

    #[test]
    fn globalbalance_parses_in_and_out() {
        let m = cmd("globalbalance")
            .try_get_matches_from(["globalbalance", "mosaic.v", "balanced.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "mosaic.v");
        assert_eq!(pos(&m, "OUT"), "balanced.v");
    }

    #[test]
    fn direction_maps_to_core_enum() {
        let m = cmd("merge")
            .try_get_matches_from(["merge", "r", "s", "o", "vertical", "0", "0"])
            .unwrap();
        assert_eq!(direction(&m).unwrap(), MergeDirection::Vertical);
        let m = cmd("merge")
            .try_get_matches_from(["merge", "r", "s", "o", "horizontal", "0", "0"])
            .unwrap();
        assert_eq!(direction(&m).unwrap(), MergeDirection::Horizontal);
    }
}
