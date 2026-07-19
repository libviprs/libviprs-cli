//! Colour op family — the Wave-2 **colour** lane (`CLI_CONTRACT.md` §3/§5,
//! `OP_MAP.md` colour section).
//!
//! The core `src/colour.rs` exposes 19 `pub fn` that fold to nine base ops;
//! two are EXCLUDED (`de00_sharma` — a non-vips CIEDE2000 variant with no
//! distinct nickname; `constant` — a creator helper the create family covers),
//! leaving **seven** `viprs` subcommands. Every command outputs a **non-RGB
//! interpretation** — LAB / XYZ / scRGB float from `colourspace`, a float ΔE
//! from the difference metrics, or a re-profiled device image from the ICC
//! ops — so every command is oracle class **BOUNDED-TOL** (`CLI_CONTRACT.md`
//! §5, colour round-trips):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `colourspace IN OUT SPACE --source-space` | `colourspace` | S1 | BOUNDED-TOL | ≤1 LSB uchar / 1e-4 float via `.v` |
//! | `dE76 LEFT RIGHT OUT`  | `dE76`  | S2 | BOUNDED-TOL | float ΔE out via `.v`, eps ~1e-4 |
//! | `dE00 LEFT RIGHT OUT`  | `dE00`  | S2 | BOUNDED-TOL | libvips `vips_col_dE00` parity, eps ~1e-4 |
//! | `dECMC LEFT RIGHT OUT` | `dECMC` | S2 | BOUNDED-TOL | eps ~1e-4 |
//! | `icc_import IN OUT --input-profile --intent --pcs`  | `icc_import`    | S1 | BOUNDED-TOL | matrix-shaper sRGB only; lcms caveat |
//! | `icc_export IN OUT --output-profile --intent --depth` | `icc_export`  | S1 | BOUNDED-TOL | matrix-shaper sRGB only; lcms caveat |
//! | `icc_transform IN OUT OUTPUT_PROFILE --input-profile --intent --depth` | `icc_transform` | S1 | BOUNDED-TOL | matrix-shaper sRGB only; lcms caveat |
//!
//! **The capital-E vips spellings are exact** (`dE76`, `dE00`, `dECMC`,
//! verified against `vips <op>`). The ΔE metrics take two image inputs and
//! write a float difference image, so they carry a `.v` sink; `colourspace`
//! writes `.v` for its float LAB/XYZ/scRGB outputs and PNG for an sRGB / uchar
//! target (the integer sink runs the interpretation-aware `→ sRGB` conversion in
//! [`io::save`], libviprs-cli #36).
//!
//! # ICC caveat (matrix-shaper only)
//!
//! libviprs ships a **native, pure-Rust ICC engine** (moxcms) while homebrew
//! vips uses **lcms2**. The two agree on **matrix-shaper RGB** profiles (sRGB
//! and friends — a colorant matrix plus per-channel TRC curves, evaluated
//! exactly on both sides) but diverge by design on LUT (CMYK and other
//! table-based) profiles, where the two CMSs interpolate different grids. The
//! differential suite therefore restricts the ICC cross-oracle to a matrix
//! -shaper sRGB profile at a **measured** tolerance; CMYK / LUT combos are
//! GOLDEN-ONLY or excluded (`OP_MAP.md` colour notes). The CLI itself imposes no
//! such restriction — it faithfully drives the core engine on any profile.
//!
//! Every handler keeps the §3 `load → try_op → save` shape and calls only the
//! panic-free `try_*` core APIs, so a bad input becomes exit 1 rather than an
//! abort (`CLI_CONTRACT.md` §8). Positional orders, flag names, enum spellings,
//! and value bounds mirror vips 8.18.4 **for the exposed surface** — the CLI is
//! a deliberate SUBSET of vips's ICC flag surface, not an exact byte-for-byte
//! mirror of every flag.
//!
//! # §9 documented subset — vips flags this CLI deliberately drops
//!
//! Verified against `vips <op> --help` (8.18.4). None of the dropped flags has a
//! core-op backing, so exposing them would be a no-op knob or require core work:
//!
//! | vips flag | on ops | why dropped |
//! |---|---|---|
//! | `intent auto` | icc_import / icc_export / icc_transform | the core [`Intent`] has no `auto` path (no CMS heuristic) |
//! | `--embedded` | icc_import / icc_transform | vips defaults `embedded=false` and errors without `--input-profile`; this CLI instead falls back to the embedded profile when `--input-profile` is absent (a behavioural choice — the embedded profile is always used as the fallback, never gated behind a flag) |
//! | `--black-point-compensation` | icc_import / icc_export / icc_transform | the moxcms core transform path exposes no BPC toggle |
//! | `--pcs {lab,xyz}` | icc_export / icc_transform | the core reads the PCS from the input raster's own interpretation tag (icc_export) / fixes Lab as the internal hop (icc_transform); only `icc_import`, whose OUTPUT PCS the caller must choose, exposes `--pcs` |
//!
//! The one intentional EXTENSION-shaped divergence is the embedded-profile
//! fallback above: unlike vips (which requires `--embedded`), this CLI treats an
//! absent `--input-profile` as "use the embedded profile", erroring `NoProfile`
//! only when neither is present. This mirrors the core `try_icc_import_with`
//! contract and is called out here so the surface is not mistaken for an exact
//! vips mirror.
//
// @doc-command:begin name=colourspace about="Convert an image to a new colour space." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=colourspace
// @doc-command:begin name=dE76 about="CIE76 colour difference between two images." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=dE76
// @doc-command:begin name=dE00 about="CIEDE2000 colour difference between two images." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=dE00
// @doc-command:begin name=dECMC about="CMC colour difference between two images." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=dECMC
// @doc-command:begin name=icc_import about="Import a device image to the profile connection space with an ICC profile." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=icc_import
// @doc-command:begin name=icc_export about="Export a PCS image to a device colour space with an ICC profile." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=icc_export
// @doc-command:begin name=icc_transform about="Transform a device image to another device profile in one step." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=icc_transform

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::{Intent, Interpretation, Pcs};

use super::{CommandMeta, OracleClass, Shape, io};

/// The exact vips 8.18.4 `colourspace` space enum (its `allowed enums` minus
/// `error`, which is not a colour space). The CLI mirrors this surface; a space
/// with no core route (`multiband`, `histogram`, `labq`, `fourier`, `matrix` as
/// a target) parses here but is rejected by the core with a typed exit-1 error,
/// exactly as vips rejects an unsupported route.
const COLOURSPACES: &[&str] = &[
    "multiband",
    "b-w",
    "histogram",
    "xyz",
    "lab",
    "cmyk",
    "labq",
    "rgb",
    "cmc",
    "lch",
    "labs",
    "srgb",
    "yxy",
    "fourier",
    "rgb16",
    "grey16",
    "matrix",
    "scrgb",
    "hsv",
    "oklab",
    "oklch",
];

/// The rendering intents the core `Intent` supports. vips also offers `auto`,
/// which the core has no path for, so it is deliberately NOT exposed (a
/// documented subset, `CLI_CONTRACT.md` §9 / `OP_MAP.md` colour notes).
const INTENTS: &[&str] = &["perceptual", "relative", "saturation", "absolute"];

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6). Every
/// colour command is BOUNDED-TOL (non-RGB float / round-trip output).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "colourspace",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "dE76",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "dE00",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "dECMC",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "icc_import",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "icc_export",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "icc_transform",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
    ]
}

/// Shared `--intent` flag (vips default `relative`; `auto` deliberately absent).
fn intent_arg() -> Arg {
    Arg::new("intent")
        .long("intent")
        .value_name("INTENT")
        .default_value("relative")
        .value_parser(INTENTS.to_vec())
        .help("Rendering intent (perceptual|relative|saturation|absolute; vips's `auto` is not core-backed)")
}

/// Shared `--depth` flag for the device-output ICC ops (vips min 8, max 16,
/// default 8; the core only realises 8 or 16 and rejects any other with exit 1).
fn depth_arg() -> Arg {
    Arg::new("depth")
        .long("depth")
        .value_name("BITS")
        .default_value("8")
        .value_parser(value_parser!(u32).range(8..=16))
        .help("Output device-space depth in bits (8 or 16)")
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("colourspace")
                .about("Convert an image to a new colour space.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("SPACE")
                        .required(true)
                        .value_name("space")
                        .value_parser(COLOURSPACES.to_vec())
                        .help("Destination colour space (vips VipsInterpretation nickname)"),
                )
                .arg(
                    Arg::new("source-space")
                        .long("source-space")
                        .value_name("space")
                        .value_parser(COLOURSPACES.to_vec())
                        .help(
                            "Source colour space; overrides the input's own interpretation tag \
                             (default: the input's interpretation)",
                        ),
                ),
        ),
        de_command("dE76", "CIE76 colour difference between two images."),
        de_command("dE00", "CIEDE2000 colour difference between two images."),
        de_command("dECMC", "CMC colour difference between two images."),
        io::with_decode_limit_args(
            Command::new("icc_import")
                .about("Import a device image to the profile connection space with an ICC profile.")
                .arg(Arg::new("IN").required(true).help("Input device image"))
                .arg(Arg::new("OUT").required(true).help("Output PCS image"))
                .arg(
                    Arg::new("input-profile")
                        .long("input-profile")
                        .value_name("PROFILE")
                        .help(
                            "ICC profile file to import from (default: the image's embedded \
                             icc-profile-data)",
                        ),
                )
                .arg(intent_arg())
                .arg(
                    Arg::new("pcs")
                        .long("pcs")
                        .value_name("PCS")
                        .default_value("lab")
                        .value_parser(["lab", "xyz"])
                        .help("Profile connection space of the output (lab|xyz)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("icc_export")
                .about("Export a PCS image to a device colour space with an ICC profile.")
                .arg(Arg::new("IN").required(true).help("Input PCS (Lab) image"))
                .arg(Arg::new("OUT").required(true).help("Output device image"))
                .arg(
                    Arg::new("output-profile")
                        .long("output-profile")
                        .value_name("PROFILE")
                        .help(
                            "ICC profile file to export to (default: the image's embedded \
                             icc-profile-data)",
                        ),
                )
                .arg(intent_arg())
                .arg(depth_arg()),
        ),
        io::with_decode_limit_args(
            Command::new("icc_transform")
                .about("Transform a device image to another device profile in one step.")
                .arg(Arg::new("IN").required(true).help("Input device image"))
                .arg(Arg::new("OUT").required(true).help("Output device image"))
                .arg(
                    Arg::new("OUTPUT_PROFILE")
                        .required(true)
                        .value_name("output-profile")
                        .help("ICC profile file to export to (positional, as in vips)"),
                )
                .arg(
                    Arg::new("input-profile")
                        .long("input-profile")
                        .value_name("PROFILE")
                        .help(
                            "ICC profile file to import from (default: the image's embedded \
                             icc-profile-data)",
                        ),
                )
                .arg(intent_arg())
                .arg(depth_arg()),
        ),
    ]
}

/// Build one of the three colour-difference commands (`dE76`/`dE00`/`dECMC`):
/// two image inputs → a float difference image (S2, vips `left right out`).
fn de_command(name: &'static str, about: &'static str) -> Command {
    io::with_decode_limit_args(
        Command::new(name)
            .about(about)
            .arg(
                Arg::new("LEFT")
                    .required(true)
                    .help("Left-hand input image"),
            )
            .arg(
                Arg::new("RIGHT")
                    .required(true)
                    .help("Right-hand input image"),
            )
            .arg(
                Arg::new("OUT")
                    .required(true)
                    .help("Output difference image"),
            ),
    )
}

/// Dispatch a matched colour subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "colourspace" => run_colourspace(m),
        "dE76" => run_de(m, DeKind::E76),
        "dE00" => run_de(m, DeKind::E00),
        "dECMC" => run_de(m, DeKind::Cmc),
        "icc_import" => run_icc_import(m),
        "icc_export" => run_icc_export(m),
        "icc_transform" => run_icc_transform(m),
        other => bail!("colour family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Parse a colour-space nickname (already restricted to [`COLOURSPACES`] by
/// clap) into an [`Interpretation`]. Any name outside the core's `FromStr`
/// surface becomes a typed exit-1 error rather than a panic.
fn parse_space(s: &str) -> Result<Interpretation> {
    Interpretation::from_str(s).map_err(|e| anyhow::anyhow!("unknown colour space {s:?}: {e}"))
}

/// Map the `--intent` flag to the core [`Intent`].
fn intent_of(m: &ArgMatches) -> Result<Intent> {
    Ok(match pos_flag(m, "intent") {
        "perceptual" => Intent::Perceptual,
        "relative" => Intent::Relative,
        "saturation" => Intent::Saturation,
        "absolute" => Intent::Absolute,
        other => {
            bail!("unknown intent {other:?} (expected perceptual|relative|saturation|absolute)")
        }
    })
}

/// Read a flag with a clap default as a `&str`.
fn pos_flag<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a flag with a default is present")
}

/// `colourspace IN OUT SPACE --source-space` — S1 image→image.
fn run_colourspace(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let target = parse_space(pos(m, "SPACE"))?;
    let source = m
        .get_one::<String>("source-space")
        .map(|s| parse_space(s))
        .transpose()?;

    // @doc-snippet:begin command=colourspace slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=colourspace slot=load

    // @doc-snippet:begin command=colourspace slot=apply
    // @doc-test: colour.rs::ported_colourspace_roundtrip:38 repo=libviprs
    // `--source-space` re-tags the input's interpretation (the core reads the
    // source space from the raster's tag) before the conversion; absent, the
    // input's own interpretation is used, mirroring vips.
    let input = match source {
        Some(space) => raster.copy().interpretation(space).build(),
        None => raster,
    };
    let out = input.try_colourspace(target)?;
    // @doc-snippet:end command=colourspace slot=apply

    // @doc-snippet:begin command=colourspace slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=colourspace slot=save
    Ok(())
}

/// Which colour-difference metric a `dE*` invocation computes.
#[derive(Clone, Copy)]
enum DeKind {
    E76,
    E00,
    Cmc,
}

/// `dE76|dE00|dECMC LEFT RIGHT OUT` — S2 two-image → float difference.
fn run_de(m: &ArgMatches, kind: DeKind) -> Result<()> {
    let limits = io::decode_limits(m);
    let left_path = PathBuf::from(pos(m, "LEFT"));
    let right_path = PathBuf::from(pos(m, "RIGHT"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=dE76 slot=load imports=decode_file
    let left = io::load(&left_path, &limits)?;
    let right = io::load(&right_path, &limits)?;
    // @doc-snippet:end command=dE76 slot=load

    // @doc-snippet:begin command=dE76 slot=apply
    // @doc-test: colour.rs::ported_de76:120 repo=libviprs
    let out = match kind {
        DeKind::E76 => left.try_de76(&right)?,
        DeKind::E00 => left.try_de00(&right)?,
        DeKind::Cmc => left.try_de_cmc(&right)?,
    };
    // @doc-snippet:end command=dE76 slot=apply

    // @doc-snippet:begin command=dE76 slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=dE76 slot=save
    Ok(())
}

/// `icc_import IN OUT --input-profile --intent --pcs` — S1 device → PCS.
fn run_icc_import(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let input_profile = m.get_one::<String>("input-profile").map(PathBuf::from);
    let intent = intent_of(m)?;
    let pcs = match pos_flag(m, "pcs") {
        "lab" => Pcs::Lab,
        "xyz" => Pcs::Xyz,
        other => bail!("unknown pcs {other:?} (expected lab|xyz)"),
    };

    // @doc-snippet:begin command=icc_import slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=icc_import slot=load

    // @doc-snippet:begin command=icc_import slot=apply
    // @doc-test: colour.rs::ported_icc_import:363 repo=libviprs
    let out = raster.try_icc_import_with(intent, input_profile.as_deref(), Some(pcs))?;
    // @doc-snippet:end command=icc_import slot=apply

    // @doc-snippet:begin command=icc_import slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=icc_import slot=save
    Ok(())
}

/// `icc_export IN OUT --output-profile --intent --depth` — S1 PCS → device.
fn run_icc_export(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let output_profile = m.get_one::<String>("output-profile").map(PathBuf::from);
    let intent = intent_of(m)?;
    let depth = *m.get_one::<u32>("depth").expect("clap default 8");

    // @doc-snippet:begin command=icc_export slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=icc_export slot=load

    // @doc-snippet:begin command=icc_export slot=apply
    // @doc-test: colour.rs::ported_icc_export:369 repo=libviprs
    let out = raster.try_icc_export_with(depth, intent, output_profile.as_deref())?;
    // @doc-snippet:end command=icc_export slot=apply

    // @doc-snippet:begin command=icc_export slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=icc_export slot=save
    Ok(())
}

/// `icc_transform IN OUT OUTPUT_PROFILE --input-profile --intent --depth` — S1
/// device → device.
///
/// vips's `icc_transform` imports through `--input-profile` (or, absent it, the
/// image's embedded profile) and exports through the positional output profile,
/// honouring `--intent` and `--depth` on both stages. The core
/// `try_icc_transform` convenience wrapper HARDCODES perceptual intent and
/// derives the depth from the input's byte width, so it cannot carry the parsed
/// flags. We therefore compose the two panic-free core steps
/// (`try_icc_import_with` → `try_icc_export_with`) UNIFORMLY in both cases:
/// `try_icc_import_with` with `input_profile = None` already reads the embedded
/// profile, so the no-`--input-profile` path is preserved while `--intent` and
/// `--depth` are honoured (matching vips's `--depth` default of 8 and `--intent`
/// default of `relative`, neither of which the core wrapper would apply).
fn run_icc_transform(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let output_profile = PathBuf::from(pos(m, "OUTPUT_PROFILE"));
    let input_profile = m.get_one::<String>("input-profile").map(PathBuf::from);
    let intent = intent_of(m)?;
    let depth = *m.get_one::<u32>("depth").expect("clap default 8");

    // @doc-snippet:begin command=icc_transform slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=icc_transform slot=load

    // @doc-snippet:begin command=icc_transform slot=apply
    // @doc-test: colour.rs::ported_icc_transform:372 repo=libviprs
    // Compose import→export uniformly so `--intent`/`--depth` are honoured on
    // both the `--input-profile` and the embedded-profile (input_profile=None)
    // paths — the core `try_icc_transform` wrapper would silently substitute
    // perceptual intent and the input's own depth.
    let out = raster
        .try_icc_import_with(intent, input_profile.as_deref(), None)?
        .try_icc_export_with(depth, intent, Some(&output_profile))?;
    // @doc-snippet:end command=icc_transform slot=apply

    // @doc-snippet:begin command=icc_transform slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=icc_transform slot=save
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
        assert_eq!(meta_names.len(), 7, "the colour family has seven commands");
    }

    #[test]
    fn command_names_use_the_exact_capital_e_vips_spellings() {
        // dE76 / dE00 / dECMC — NOT de76 / de00 / decmc (CLI mirrors the vips
        // nickname byte-for-byte).
        let names: Vec<String> = commands()
            .iter()
            .map(|c| c.get_name().to_string())
            .collect();
        for want in ["dE76", "dE00", "dECMC"] {
            assert!(names.iter().any(|n| n == want), "missing {want}");
        }
    }

    #[test]
    fn colourspace_parses_positionals_and_source_space() {
        let m = cmd("colourspace")
            .try_get_matches_from([
                "colourspace",
                "in.png",
                "out.v",
                "lab",
                "--source-space",
                "srgb",
            ])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.v");
        assert_eq!(pos(&m, "SPACE"), "lab");
        assert_eq!(
            m.get_one::<String>("source-space").map(String::as_str),
            Some("srgb")
        );
    }

    #[test]
    fn colourspace_rejects_an_unknown_space() {
        // Only the vips enum spellings are accepted at parse time.
        assert!(
            cmd("colourspace")
                .try_get_matches_from(["colourspace", "in.png", "out.v", "not-a-space"])
                .is_err(),
            "an unknown colour space must be rejected by clap"
        );
    }

    #[test]
    fn colourspace_accepts_the_dash_spelled_bw() {
        // vips spells mono `b-w` with a dash; the parser must accept it.
        let m = cmd("colourspace")
            .try_get_matches_from(["colourspace", "in.png", "out.png", "b-w"])
            .unwrap();
        assert_eq!(pos(&m, "SPACE"), "b-w");
        assert_eq!(parse_space("b-w").unwrap(), Interpretation::Bw);
    }

    #[test]
    fn de_commands_take_two_inputs_and_an_output() {
        for name in ["dE76", "dE00", "dECMC"] {
            let m = cmd(name)
                .try_get_matches_from([name, "a.png", "b.png", "out.v"])
                .unwrap();
            assert_eq!(pos(&m, "LEFT"), "a.png");
            assert_eq!(pos(&m, "RIGHT"), "b.png");
            assert_eq!(pos(&m, "OUT"), "out.v");
        }
    }

    #[test]
    fn de_commands_require_all_three_positionals() {
        assert!(
            cmd("dE76")
                .try_get_matches_from(["dE76", "a.png", "out.v"])
                .is_err(),
            "dE76 needs left, right, and out"
        );
    }

    #[test]
    fn icc_import_defaults_mirror_vips() {
        // vips icc_import defaults: intent relative, pcs lab.
        let m = cmd("icc_import")
            .try_get_matches_from(["icc_import", "in.png", "out.v"])
            .unwrap();
        assert_eq!(pos_flag(&m, "intent"), "relative");
        assert_eq!(pos_flag(&m, "pcs"), "lab");
        assert!(m.get_one::<String>("input-profile").is_none());
        assert_eq!(intent_of(&m).unwrap(), Intent::Relative);
    }

    #[test]
    fn icc_import_rejects_the_vips_auto_intent() {
        // vips offers `auto`; the core has no such path, so the CLI must reject
        // it (documented subset, not a hidden extension).
        assert!(
            cmd("icc_import")
                .try_get_matches_from(["icc_import", "in.png", "out.v", "--intent", "auto"])
                .is_err(),
            "intent auto is not core-backed and must be rejected"
        );
    }

    #[test]
    fn icc_export_depth_defaults_to_8_and_bounds_are_enforced() {
        let m = cmd("icc_export")
            .try_get_matches_from(["icc_export", "in.v", "out.png"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("depth").unwrap(), 8);
        // vips's declared min is 8 / max is 16.
        assert!(
            cmd("icc_export")
                .try_get_matches_from(["icc_export", "in.v", "out.png", "--depth", "4"])
                .is_err(),
            "a depth below vips's minimum (8) must be rejected"
        );
        assert!(
            cmd("icc_export")
                .try_get_matches_from(["icc_export", "in.v", "out.png", "--depth", "32"])
                .is_err(),
            "a depth above vips's maximum (16) must be rejected"
        );
    }

    #[test]
    fn icc_transform_takes_a_positional_output_profile() {
        let m = cmd("icc_transform")
            .try_get_matches_from([
                "icc_transform",
                "in.png",
                "out.png",
                "sRGB.icc",
                "--input-profile",
                "in.icc",
            ])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "OUTPUT_PROFILE"), "sRGB.icc");
        assert_eq!(
            m.get_one::<String>("input-profile").map(String::as_str),
            Some("in.icc")
        );
    }

    #[test]
    fn intent_mapping_covers_every_variant() {
        for (s, want) in [
            ("perceptual", Intent::Perceptual),
            ("relative", Intent::Relative),
            ("saturation", Intent::Saturation),
            ("absolute", Intent::Absolute),
        ] {
            let m = cmd("icc_import")
                .try_get_matches_from(["icc_import", "in.png", "out.v", "--intent", s])
                .unwrap();
            assert_eq!(intent_of(&m).unwrap(), want);
        }
    }
}
