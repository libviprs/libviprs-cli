//! Composite (alpha-blending) op family — a Wave-2 per-family lane
//! (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md` composite section).
//!
//! The core `src/composite.rs` exposes a single fallible operation
//! ([`Raster::try_composite2`]) plus its panicking `composite` / `composite2`
//! aliases; it blends exactly **two** images with **one** blend mode. Both vips
//! nicknames map onto it, so this family contributes **two** `viprs`
//! subcommands, each oracle class **BOUNDED-TOL** (≤1 LSB — the blend is done
//! premultiplied in `f64` then rounded to the integer container, so core and
//! vips agree to within one least-significant bit):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `composite BASE OVERLAY OUT MODE`  | `composite`  | S2 | BOUNDED-TOL ≤1 LSB | vips array form, restricted to two inputs / one mode |
//! | `composite2 BASE OVERLAY OUT MODE` | `composite2` | S2 | BOUNDED-TOL ≤1 LSB | vips pair form |
//!
//! **Mode surface.** `MODE` is one of the **25** vips 8.18.4 `VipsBlendMode`
//! spellings (`vips composite2` `allowed enums`, verified against the oracle):
//! the Porter-Duff operators (`clear`…`saturate`) and the PDF separable blends
//! (`multiply`…`exclusion`), with vips's exact hyphenated spellings
//! (`dest-over`, `colour-dodge`, `hard-light`, …). The core additionally
//! implements four non-separable PDF modes (`hue`, `saturation`, `colour`,
//! `luminosity`) for the ported conversion suite, but vips 8.18.4's blend-mode
//! enum stops at `exclusion`, so those are **core-only extensions with no vips
//! oracle** and are deliberately NOT exposed on this parity surface (mirroring
//! the bands lane, which likewise omits the non-core-backed `lshift`/`rshift`
//! from `bandbool`).
//!
//! **Subset.** The core op composites exactly two images at the origin with
//! unpremultiplied inputs in their own colour space; vips's `--x`, `--y`,
//! `--compositing-space`, and `--premultiplied` options have no core equivalent
//! (they keep their vips defaults `0`, `0`, `srgb`, `false`), so they are not
//! exposed — the references are generated at those defaults. `composite`'s vips
//! image ARRAY is restricted to two inputs (`BASE OVERLAY`): the fixed
//! positional encoding rejects a third input with clap's usage exit 2, matching
//! the OP_MAP subset note.
//!
//! Both handlers keep the §3 `load → try_op → save` shape and call only the
//! panic-free [`Raster::try_composite2`] core API, so a bad input (mismatched
//! geometry, a band count the op cannot blend) becomes a typed exit 1 rather
//! than an abort (`CLI_CONTRACT.md` §8); an unknown or non-core-backed mode is a
//! clap usage exit 2.
//
// @doc-command:begin name=composite about="Blend two images with a blend mode (vips array form, two inputs)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=composite
// @doc-command:begin name=composite2 about="Blend a base and an overlay image with a blend mode." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=composite2

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use libviprs::CompositeMode;

use super::{CommandMeta, OracleClass, Shape, io};

/// The 25 vips 8.18.4 `VipsBlendMode` spellings exposed as `MODE`, in vips enum
/// order. This is exactly the `allowed enums` list `vips composite2` prints; the
/// four core-only non-separable modes (`hue`/`saturation`/`colour`/`luminosity`)
/// are intentionally absent (no vips oracle — see the module header).
const MODES: &[&str] = &[
    "clear",
    "source",
    "over",
    "in",
    "out",
    "atop",
    "dest",
    "dest-over",
    "dest-in",
    "dest-out",
    "dest-atop",
    "xor",
    "add",
    "saturate",
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "colour-dodge",
    "colour-burn",
    "hard-light",
    "soft-light",
    "difference",
    "exclusion",
];

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
///
/// **Shape caveat.** [`Shape`] is a COARSE classifier and its closest fit here is
/// [`Shape::NImageToImage`] (`§3.2`, "n-image->image"), which both commands
/// declare. Strictly, composite is neither variadic nor OUT-last: it is the
/// vips-array op RESTRICTED to a fixed two-input subset carrying a trailing
/// `MODE` enum positional — `BASE OVERLAY OUT MODE` (the core blends exactly two
/// images with one mode, so a third input is a usage exit 2, see [`commands`]).
/// There is no finer `Shape` variant for "fixed-arity multi-input with a trailing
/// enum", so `NImageToImage` is the intended catch-all: it advertises the
/// multi-input nature to a `__dump-commands` consumer, at the cost of the
/// (documented) inaccuracy that `MODE` follows `OUT` rather than inputs preceding
/// a lone `OUT`. `composite2` (the vips pair form) shares the identical positional
/// shape, hence the identical `Shape`.
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "composite",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "composite2",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Both commands take the same fixed `BASE OVERLAY OUT MODE` positional shape:
/// the core op blends exactly two images with one mode, so a fixed encoding (not
/// a variadic one) is the faithful mirror and rejects a third input at clap's
/// usage exit 2 (`composite`'s vips image array is restricted to two inputs per
/// the OP_MAP subset note).
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(composite_command(
            "composite",
            "Blend two images with a blend mode (vips array form, two inputs).",
        )),
        io::with_decode_limit_args(composite_command(
            "composite2",
            "Blend a base and an overlay image with a blend mode.",
        )),
    ]
}

/// Build one `<op> BASE OVERLAY OUT MODE` command (shared by `composite` and
/// `composite2`, which differ only in vips nickname — the core op is identical).
fn composite_command(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .arg(
            Arg::new("BASE")
                .required(true)
                .help("Base image (the backdrop)"),
        )
        .arg(
            Arg::new("OVERLAY")
                .required(true)
                .help("Overlay image (the source), same size as BASE"),
        )
        .arg(Arg::new("OUT").required(true).help("Output image"))
        .arg(
            Arg::new("MODE")
                .required(true)
                .value_name("MODE")
                .value_parser(MODES.to_vec())
                .help(
                    "Blend mode: one of the 25 vips VipsBlendMode names \
                     (clear|source|over|in|out|atop|dest|dest-over|dest-in|\
                     dest-out|dest-atop|xor|add|saturate|multiply|screen|overlay|\
                     darken|lighten|colour-dodge|colour-burn|hard-light|soft-light|\
                     difference|exclusion)",
                ),
        )
}

/// Dispatch a matched composite subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        // Both nicknames map onto the same core op (`try_composite2`): the core
        // blends exactly two images with one mode, so vips's `composite` array
        // form and `composite2` pair form coincide at two inputs.
        "composite" | "composite2" => run_composite(name, m),
        other => bail!("composite family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Map a CLI mode string (a vips `VipsBlendMode` spelling) to a core
/// [`CompositeMode`]. clap's `value_parser` already restricts the input to
/// [`MODES`], so the fallthrough is unreachable in practice but is a typed
/// error, never a panic (`CLI_CONTRACT.md` §8).
fn parse_mode(s: &str) -> Result<CompositeMode> {
    Ok(match s {
        "clear" => CompositeMode::Clear,
        "source" => CompositeMode::Source,
        "over" => CompositeMode::Over,
        "in" => CompositeMode::In,
        "out" => CompositeMode::Out,
        "atop" => CompositeMode::Atop,
        "dest" => CompositeMode::Dest,
        "dest-over" => CompositeMode::DestOver,
        "dest-in" => CompositeMode::DestIn,
        "dest-out" => CompositeMode::DestOut,
        "dest-atop" => CompositeMode::DestAtop,
        "xor" => CompositeMode::Xor,
        "add" => CompositeMode::Add,
        "saturate" => CompositeMode::Saturate,
        "multiply" => CompositeMode::Multiply,
        "screen" => CompositeMode::Screen,
        "overlay" => CompositeMode::Overlay,
        "darken" => CompositeMode::Darken,
        "lighten" => CompositeMode::Lighten,
        "colour-dodge" => CompositeMode::ColourDodge,
        "colour-burn" => CompositeMode::ColourBurn,
        "hard-light" => CompositeMode::HardLight,
        "soft-light" => CompositeMode::SoftLight,
        "difference" => CompositeMode::Difference,
        "exclusion" => CompositeMode::Exclusion,
        other => bail!(
            "unknown blend mode {other:?} (expected one of: {})",
            MODES.join("|")
        ),
    })
}

/// `<op> BASE OVERLAY OUT MODE` — S2; blend the overlay onto the base.
fn run_composite(_name: &str, m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let base_path = PathBuf::from(pos(m, "BASE"));
    let overlay_path = PathBuf::from(pos(m, "OVERLAY"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let mode = parse_mode(pos(m, "MODE"))?;

    // @doc-snippet:begin command=composite2 slot=load imports=decode_file
    let base = io::load(&base_path, &limits)?;
    let overlay = io::load(&overlay_path, &limits)?;
    // @doc-snippet:end command=composite2 slot=load

    // @doc-snippet:begin command=composite2 slot=apply
    // @doc-test: composite.rs::over_blends_half_transparent_overlay:771 repo=libviprs
    let out = base.try_composite2(&overlay, mode)?;
    // @doc-snippet:end command=composite2 slot=apply

    // @doc-snippet:begin command=composite2 slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=composite2 slot=save
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
        assert_eq!(meta_names.len(), 2, "the composite family has two commands");
    }

    #[test]
    fn composite2_parses_positionals_in_vips_order() {
        let m = cmd("composite2")
            .try_get_matches_from(["composite2", "base.png", "over.png", "out.png", "over"])
            .unwrap();
        assert_eq!(pos(&m, "BASE"), "base.png");
        assert_eq!(pos(&m, "OVERLAY"), "over.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "MODE"), "over");
    }

    #[test]
    fn composite_has_same_shape_as_composite2() {
        // `composite` is the vips array form restricted to two inputs; on the CLI
        // it takes the identical BASE OVERLAY OUT MODE positional shape.
        let m = cmd("composite")
            .try_get_matches_from(["composite", "b.png", "o.png", "out.png", "multiply"])
            .unwrap();
        assert_eq!(pos(&m, "BASE"), "b.png");
        assert_eq!(pos(&m, "MODE"), "multiply");
    }

    #[test]
    fn hyphenated_modes_parse() {
        // vips's hyphenated spellings must be accepted verbatim.
        for spelling in ["dest-over", "colour-dodge", "hard-light", "soft-light"] {
            let m = cmd("composite2")
                .clone()
                .try_get_matches_from(["composite2", "b.png", "o.png", "out.png", spelling])
                .unwrap_or_else(|e| panic!("mode {spelling} must parse: {e}"));
            assert_eq!(pos(&m, "MODE"), spelling);
        }
    }

    #[test]
    fn every_exposed_mode_maps_to_a_core_variant() {
        // The value_parser list and the parse_mode match must stay in lockstep:
        // every advertised spelling maps to a CompositeMode without error.
        for spelling in MODES {
            parse_mode(spelling)
                .unwrap_or_else(|e| panic!("MODE {spelling} has no core mapping: {e}"));
        }
        assert_eq!(MODES.len(), 25, "vips 8.18.4 has 25 blend modes");
    }

    #[test]
    fn non_separable_modes_are_rejected() {
        // The four core-only PDF non-separable modes have no vips oracle and are
        // not exposed: clap must reject them (usage exit 2), not silently accept.
        for spelling in ["hue", "saturation", "colour", "luminosity"] {
            assert!(
                cmd("composite2")
                    .clone()
                    .try_get_matches_from(["composite2", "b.png", "o.png", "out.png", spelling])
                    .is_err(),
                "core-only non-separable mode {spelling} must be rejected"
            );
        }
    }

    #[test]
    fn unknown_mode_is_rejected() {
        assert!(
            cmd("composite2")
                .try_get_matches_from(["composite2", "b.png", "o.png", "out.png", "screenx"])
                .is_err(),
            "an unknown blend mode must be rejected by clap"
        );
    }

    #[test]
    fn missing_positional_is_a_usage_error() {
        assert!(
            cmd("composite2")
                .try_get_matches_from(["composite2", "b.png", "o.png", "out.png"])
                .is_err(),
            "a missing MODE must be a usage error"
        );
    }
}
