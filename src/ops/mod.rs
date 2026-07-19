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
//! A single static [`FAMILIES`] table lists every family module. `main()`
//! assembles the CLI by unioning the frozen derived commands
//! (pyramid/info/plan/test-image) with every family's `commands()` plus the
//! hidden `__dump-commands`, then routes any subcommand it did not recognise as
//! a built-in to the owning family's `run()` via [`dispatch`]. There are **no**
//! per-op enum arms and the pyramid code is never rewritten.

pub mod dump;
pub mod io;
pub mod matfile;
pub mod morphology;

use clap::{ArgMatches, Command};

/// Static per-command metadata carried alongside each family's clap commands.
///
/// `shape` is one of the six `CLI_CONTRACT.md` §3 shapes rendered as a
/// SCHEMA_V2 §3.1 string (`image->image`, `n-image->image`,
/// `image->stdout-scalar`, `image->two-outputs`, `creator`, `draw`);
/// `oracle_class` is one of the §5 classes.
#[derive(Clone, Copy)]
pub struct CommandMeta {
    /// The command name (= exact vips nickname).
    pub name: &'static str,
    /// The §3 command shape as a SCHEMA_V2 §3.1 string.
    pub shape: &'static str,
    /// The §5 oracle class.
    pub oracle_class: &'static str,
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

/// The registry of op families. Written once in Wave 1; later per-family waves
/// add exactly one row each and touch nothing else here.
pub static FAMILIES: &[Family] = &[Family {
    name: "morphology",
    commands: morphology::commands,
    run: morphology::run,
    metas: morphology::metas,
}];

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
    fn assembled_cli_has_builtins_and_family_commands() {
        let cli = assembled_cli();
        let names: Vec<&str> = cli.get_subcommands().map(|c| c.get_name()).collect();
        for builtin in ["pyramid", "info", "plan", "test-image"] {
            assert!(names.contains(&builtin), "missing builtin {builtin}");
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
}
