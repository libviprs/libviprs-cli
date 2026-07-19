//! Per-family op registry (`CLI_CONTRACT.md` §6 refactor invariants).
//!
//! The op surface is **additive**: each family lives in its own
//! `src/ops/<family>.rs` module exposing
//!
//! * `pub fn commands() -> Vec<clap::Command>` — the clap commands it
//!   contributes (each with its about, positionals, and flags), and
//! * `pub fn run(name: &str, m: &clap::ArgMatches) -> anyhow::Result<()>` — the
//!   handler dispatched to when one of its commands matched, and
//! * `pub fn metas() -> Vec<CommandMeta>` — the static name → shape +
//!   oracle-class side table used by `__dump-commands` and the dispatcher.
//!
//! A single static [`FAMILIES`] table lists **every** OP_MAP family module,
//! written ONCE in Wave 1 (`CLI_CONTRACT.md` §6). Every family other than the
//! Wave-1 reference (`morphology`) is a pre-declared **stub** — an empty
//! `commands()` / `metas()` and a `run()` that bails — so a later per-family
//! wave fills in only its own module and NEVER edits this file. `main()`
//! assembles the CLI by unioning the frozen derived commands
//! (pyramid/info/plan/test-image) with every family's `commands()` plus the
//! hidden `__dump-commands`, then routes any subcommand it did not recognise as
//! a built-in to the owning family's `run()` via [`dispatch`]. There are **no**
//! per-op enum arms and the pyramid code is never rewritten.

// Wave-1 reference family (real) + shared harness.
pub mod dump;
pub mod io;
pub mod matfile;
pub mod morphology;

// Every remaining OP_MAP family, pre-declared once (`CLI_CONTRACT.md` §6). Each
// is an empty stub until its own wave lands; listing them here now means no
// later wave has to touch `mod.rs`.
pub mod arithmetic;
pub mod bands;
pub mod colour;
pub mod composite;
pub mod conversion;
pub mod convolution;
pub mod core;
pub mod create;
pub mod draw;
pub mod extract;
pub mod freqfilt;
pub mod histogram;
pub mod matrix;
pub mod mosaicing;
pub mod resample;

use clap::{ArgMatches, Command};

/// The six `CLI_CONTRACT.md` §3 command shapes, rendered to the frozen
/// SCHEMA_V2 §3.1 strings via [`Shape::as_str`].
///
/// Using an enum (rather than a raw `&'static str`) means a typo like
/// `"image->imgae"` cannot compile: every shape must be one of these variants.
///
/// This is the complete §3 vocabulary, declared once in Wave 1. Only the
/// shapes the morphology reference family uses are constructed today; the rest
/// are the surface later per-family waves fill in, so the whole enum carries
/// `#[allow(dead_code)]` until then (as with [`MatFile`](matfile::MatFile)'s
/// convolution-only accessors).
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// `<op> IN OUT [--flags]` (§3.1).
    ImageToImage,
    /// N-image → image, variadic inputs before OUT (§3.2).
    NImageToImage,
    /// image → stdout scalar(s), no OUT (§3.3).
    StdoutScalar,
    /// image → two outputs (§3.4).
    TwoOutputs,
    /// creator, OUT first (§3.5).
    Creator,
    /// draw op, `<op> IN OUT --ink …` (§3.6).
    Draw,
}

impl Shape {
    /// The exact SCHEMA_V2 / OP_MAP spelling emitted in `__dump-commands`.
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::ImageToImage => "image->image",
            Shape::NImageToImage => "n-image->image",
            Shape::StdoutScalar => "image->stdout-scalar",
            Shape::TwoOutputs => "image->two-outputs",
            Shape::Creator => "creator",
            Shape::Draw => "draw",
        }
    }
}

/// The seven `CLI_CONTRACT.md` §5 oracle classes, rendered to the frozen
/// OP_MAP / SCHEMA_V2 strings via [`OracleClass::as_str`].
///
/// Enum (not `&'static str`) so a typo like `"EXCAT"` cannot compile. The full
/// §5 vocabulary is declared once in Wave 1; only `Exact` is constructed today
/// (the morphology reference family), so the enum carries `#[allow(dead_code)]`
/// for the classes later waves will use.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OracleClass {
    /// Integer-in/integer-out, decode-compare, tol 0.
    Exact,
    /// Float-promoting, compared after the §2 round-half-even save-cast.
    ExactAfterCast,
    /// Bounded numeric tolerance (resample/colour round-trips).
    BoundedTol,
    /// Complex / Fourier float-pair bands via `.v`.
    Fourier,
    /// No vips CLI oracle; regression pin from a committed fixture.
    GoldenOnly,
    /// Helper / no-CLI-sense; not exercised.
    Excluded,
    /// Needs signed/complex carriers libviprs lacks (#283/#285).
    Deferred,
}

impl OracleClass {
    /// The exact SCHEMA_V2 / OP_MAP spelling emitted in `__dump-commands`.
    pub fn as_str(self) -> &'static str {
        match self {
            OracleClass::Exact => "EXACT",
            OracleClass::ExactAfterCast => "EXACT-AFTER-CAST",
            OracleClass::BoundedTol => "BOUNDED-TOL",
            OracleClass::Fourier => "FOURIER",
            OracleClass::GoldenOnly => "GOLDEN-ONLY",
            OracleClass::Excluded => "EXCLUDED",
            OracleClass::Deferred => "DEFERRED",
        }
    }
}

/// Static per-command metadata carried alongside each family's clap commands.
///
/// `shape` is one of the six `CLI_CONTRACT.md` §3 [`Shape`]s and `oracle_class`
/// one of the §5 [`OracleClass`]es; both render to their frozen SCHEMA_V2 §3.1
/// strings in `__dump-commands`.
#[derive(Clone, Copy)]
pub struct CommandMeta {
    /// The command name (= exact vips nickname).
    pub name: &'static str,
    /// The §3 command shape.
    pub shape: Shape,
    /// The §5 oracle class.
    pub oracle_class: OracleClass,
}

/// One op family module, referenced by the static [`FAMILIES`] table.
///
/// Function pointers (not trait objects) keep the table `const`-friendly and
/// the registry a single flat list edited once in Wave 1.
pub struct Family {
    /// Family name (module name), e.g. `morphology`.
    pub name: &'static str,
    /// The clap commands this family contributes.
    pub commands: fn() -> Vec<Command>,
    /// Handler for a matched command of this family.
    pub run: fn(&str, &ArgMatches) -> anyhow::Result<()>,
    /// Static per-command metadata.
    pub metas: fn() -> Vec<CommandMeta>,
}

/// The registry of op families. Written once in Wave 1; a later per-family wave
/// fills in exactly one stub module below and touches nothing else here.
pub static FAMILIES: &[Family] = &[
    Family {
        name: "arithmetic",
        commands: arithmetic::commands,
        run: arithmetic::run,
        metas: arithmetic::metas,
    },
    Family {
        name: "composite",
        commands: composite::commands,
        run: composite::run,
        metas: composite::metas,
    },
    Family {
        name: "resample",
        commands: resample::commands,
        run: resample::run,
        metas: resample::metas,
    },
    Family {
        name: "colour",
        commands: colour::commands,
        run: colour::run,
        metas: colour::metas,
    },
    Family {
        name: "draw",
        commands: draw::commands,
        run: draw::run,
        metas: draw::metas,
    },
    Family {
        name: "bands",
        commands: bands::commands,
        run: bands::run,
        metas: bands::metas,
    },
    Family {
        name: "conversion",
        commands: conversion::commands,
        run: conversion::run,
        metas: conversion::metas,
    },
    Family {
        name: "convolution",
        commands: convolution::commands,
        run: convolution::run,
        metas: convolution::metas,
    },
    Family {
        name: "morphology",
        commands: morphology::commands,
        run: morphology::run,
        metas: morphology::metas,
    },
    Family {
        name: "histogram",
        commands: histogram::commands,
        run: histogram::run,
        metas: histogram::metas,
    },
    Family {
        name: "mosaicing",
        commands: mosaicing::commands,
        run: mosaicing::run,
        metas: mosaicing::metas,
    },
    Family {
        name: "freqfilt",
        commands: freqfilt::commands,
        run: freqfilt::run,
        metas: freqfilt::metas,
    },
    Family {
        name: "create",
        commands: create::commands,
        run: create::run,
        metas: create::metas,
    },
    Family {
        name: "extract",
        commands: extract::commands,
        run: extract::run,
        metas: extract::metas,
    },
    Family {
        name: "matrix",
        commands: matrix::commands,
        run: matrix::run,
        metas: matrix::metas,
    },
    Family {
        name: "core",
        commands: core::commands,
        run: core::run,
        metas: core::metas,
    },
];

/// The frozen derived built-ins that live in `main.rs` (`CLI_CONTRACT.md` §6),
/// plus the hidden `__dump-commands`. Used by the registry-consistency check so
/// a family can never shadow a built-in name.
pub const BUILTIN_COMMANDS: &[&str] = &["pyramid", "info", "plan", "test-image"];

/// Assemble the full `viprs` CLI: the frozen derived commands unioned with
/// every family's commands and the hidden `__dump-commands`.
///
/// This is the single source of the assembled command tree, shared by `main()`
/// (for `get_matches`) and by `__dump-commands` (for introspection).
pub fn assembled_cli() -> Command {
    let mut cli = crate::base_cli();
    for fam in FAMILIES {
        for cmd in (fam.commands)() {
            cli = cli.subcommand(cmd);
        }
    }
    cli.subcommand(dump::command())
}

/// Route a non-built-in subcommand to the family that owns it.
///
/// # Errors
///
/// Propagates the handler's error, or reports an unknown command (which should
/// be unreachable because clap only yields registered subcommands).
pub fn dispatch(name: &str, m: &ArgMatches) -> anyhow::Result<()> {
    for fam in FAMILIES {
        if (fam.metas)().iter().any(|cm| cm.name == name) {
            return (fam.run)(name, m);
        }
    }
    anyhow::bail!("no family owns the command {name:?}")
}

/// Handle the hidden `__dump-commands` subcommand.
pub fn run_dump(m: &ArgMatches) {
    if !m.get_flag("json") {
        eprintln!("Error: __dump-commands requires --json");
        std::process::exit(2);
    }
    let value = dump::dump_commands_json(&assembled_cli());
    match serde_json::to_string_pretty(&value) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("Error: failed to serialize command registry: {e}");
            std::process::exit(1);
        }
    }
}

/// Check the whole command registry for consistency (`CLI_CONTRACT.md` §6):
///
/// * every family's `commands()` names and `metas()` names are the same set
///   (no command without a meta, no meta without a command), and
/// * every command name is globally unique across all families, the built-ins
///   (`pyramid`/`info`/`plan`/`test-image`) and the hidden `__dump-commands`.
///
/// Returns a human-readable description of the first violation found, or `None`
/// when the registry is consistent. Drives both the startup `debug_assert` in
/// `main()` and the unit test below, turning a silent mis-route into a hard
/// failure.
fn registry_consistency_error() -> Option<String> {
    use std::collections::HashSet;

    // Per-family: commands() name set == metas() name set.
    for fam in FAMILIES {
        let cmd_names: Vec<String> = (fam.commands)()
            .iter()
            .map(|c| c.get_name().to_string())
            .collect();
        let meta_names: Vec<String> = (fam.metas)().iter().map(|m| m.name.to_string()).collect();
        for name in &meta_names {
            if !cmd_names.iter().any(|c| c == name) {
                return Some(format!(
                    "family {} declares meta {name:?} with no matching command",
                    fam.name
                ));
            }
        }
        for name in &cmd_names {
            if !meta_names.iter().any(|m| m == name) {
                return Some(format!(
                    "family {} declares command {name:?} with no matching meta",
                    fam.name
                ));
            }
        }
    }

    // Global uniqueness across families + built-ins + __dump-commands.
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::new();
    all.extend(BUILTIN_COMMANDS.iter().map(|s| s.to_string()));
    all.push("__dump-commands".to_string());
    for fam in FAMILIES {
        all.extend((fam.commands)().iter().map(|c| c.get_name().to_string()));
    }
    for name in all {
        if !seen.insert(name.clone()) {
            return Some(format!(
                "command name {name:?} is registered more than once"
            ));
        }
    }

    None
}

/// Whether the command registry is consistent (used by the startup
/// `debug_assert` in `main()`; see [`registry_consistency_error`]).
pub fn registry_is_consistent() -> bool {
    registry_consistency_error().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_command_has_a_meta_and_vice_versa() {
        for fam in FAMILIES {
            let cmd_names: Vec<String> = (fam.commands)()
                .iter()
                .map(|c| c.get_name().to_string())
                .collect();
            let meta_names: Vec<&str> = (fam.metas)().iter().map(|m| m.name).collect();
            assert_eq!(
                cmd_names.len(),
                meta_names.len(),
                "family {} command/meta count mismatch",
                fam.name
            );
            for name in &meta_names {
                assert!(
                    cmd_names.iter().any(|c| c == name),
                    "family {} meta {name} has no command",
                    fam.name
                );
            }
        }
    }

    #[test]
    fn registry_is_globally_consistent() {
        // Uniqueness across every family + the built-ins + __dump-commands, and
        // the per-family command<->meta bijection. A mis-route (duplicate name
        // or a command with no meta) is a hard failure, not a silent shadow.
        assert!(
            registry_consistency_error().is_none(),
            "{}",
            registry_consistency_error().unwrap_or_default()
        );
        assert!(registry_is_consistent());
    }

    #[test]
    fn assembled_cli_has_builtins_and_family_commands() {
        let cli = assembled_cli();
        let names: Vec<&str> = cli.get_subcommands().map(|c| c.get_name()).collect();
        for builtin in BUILTIN_COMMANDS {
            assert!(names.contains(builtin), "missing builtin {builtin}");
        }
        assert!(names.contains(&"morph"), "missing family command morph");
        assert!(names.contains(&"__dump-commands"));
    }

    #[test]
    fn dispatch_rejects_unknown_command() {
        // A synthetic empty matches for a non-existent command name.
        let cmd = Command::new("nope");
        let m = cmd.try_get_matches_from(["nope"]).unwrap();
        assert!(dispatch("nope", &m).is_err());
    }

    #[test]
    fn stub_families_contribute_nothing_yet() {
        // A pre-declared *stub* family (its `metas()` still empty) must also
        // contribute no commands — no half-wired module. An *implemented*
        // family (non-empty `metas()`, e.g. the morphology reference and the
        // Wave-2 `bands` lane) is exercised by its own module's tests instead.
        // Keying off metas-emptiness (rather than a hard-coded family name
        // list) keeps this check wave-agnostic: each family wave fills in its
        // module without editing this file.
        for fam in FAMILIES {
            if !(fam.metas)().is_empty() {
                continue;
            }
            assert!(
                (fam.commands)().is_empty(),
                "stub family {} unexpectedly has commands",
                fam.name
            );
        }
    }

    #[test]
    fn shape_and_oracle_class_render_to_frozen_strings() {
        assert_eq!(Shape::ImageToImage.as_str(), "image->image");
        assert_eq!(Shape::NImageToImage.as_str(), "n-image->image");
        assert_eq!(Shape::StdoutScalar.as_str(), "image->stdout-scalar");
        assert_eq!(Shape::TwoOutputs.as_str(), "image->two-outputs");
        assert_eq!(Shape::Creator.as_str(), "creator");
        assert_eq!(Shape::Draw.as_str(), "draw");
        assert_eq!(OracleClass::Exact.as_str(), "EXACT");
        assert_eq!(OracleClass::ExactAfterCast.as_str(), "EXACT-AFTER-CAST");
        assert_eq!(OracleClass::BoundedTol.as_str(), "BOUNDED-TOL");
        assert_eq!(OracleClass::Fourier.as_str(), "FOURIER");
        assert_eq!(OracleClass::GoldenOnly.as_str(), "GOLDEN-ONLY");
        assert_eq!(OracleClass::Excluded.as_str(), "EXCLUDED");
        assert_eq!(OracleClass::Deferred.as_str(), "DEFERRED");
    }
}
