//! Arithmetic op family (`CLI_CONTRACT.md` §6).
//!
//! Split into two submodules so two Wave-2 lanes can author disjoint files
//! without conflict:
//! * [`part_a`] — statistics, const/linear, unary/rounding, hough.
//! * [`part_b`] — binary N-ary, relational, boolean, windowed, math/math2,
//!   complex (Fourier).
//!
//! This module only aggregates them; [`super::FAMILIES`] references
//! `arithmetic::{commands, run, metas}` unchanged.

use clap::{ArgMatches, Command};

use super::CommandMeta;

pub mod part_a;
pub mod part_b;

/// All clap commands this family contributes (union of both parts).
pub fn commands() -> Vec<Command> {
    let mut cmds = part_a::commands();
    cmds.extend(part_b::commands());
    cmds
}

/// Static per-command metadata (union of both parts).
pub fn metas() -> Vec<CommandMeta> {
    let mut metas = part_a::metas();
    metas.extend(part_b::metas());
    metas
}

/// Dispatch a matched command to the owning part's handler.
///
/// # Errors
///
/// Propagates the handler's error; bails if no part owns `name` (unreachable via
/// [`super::dispatch`], which only calls `run` for a registered name).
pub fn run(name: &str, m: &ArgMatches) -> anyhow::Result<()> {
    if part_a::metas().iter().any(|cm| cm.name == name) {
        return part_a::run(name, m);
    }
    if part_b::metas().iter().any(|cm| cm.name == name) {
        return part_b::run(name, m);
    }
    anyhow::bail!("no arithmetic part owns the command {name:?}")
}
