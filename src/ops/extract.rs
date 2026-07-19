//! Extract op family — Wave-1 pre-declared **stub** (`CLI_CONTRACT.md` §6).
//!
//! Registered in [`super::FAMILIES`] now so the extract wave fills in only this
//! module and never edits `ops/mod.rs`. Until that wave lands the family
//! contributes no commands and no metadata; its `run` is unreachable because
//! [`super::dispatch`] only calls `run` for a name present in [`metas`].

use clap::{ArgMatches, Command};

use super::CommandMeta;

/// The clap commands this family contributes (none until the extract wave lands).
pub fn commands() -> Vec<Command> {
    Vec::new()
}

/// Static per-command metadata (none until the extract wave lands).
pub fn metas() -> Vec<CommandMeta> {
    Vec::new()
}

/// Dispatch a matched command to its handler.
///
/// # Errors
///
/// Always errors in Wave 1: the extract family registers no commands yet, so
/// this is never reached through [`super::dispatch`]; a direct call reports the
/// unimplemented command rather than panicking.
pub fn run(name: &str, _m: &ArgMatches) -> anyhow::Result<()> {
    anyhow::bail!(
        "command {name:?} is not implemented yet ({} family stub)",
        "extract"
    )
}
