//! Arithmetic family — **part A** (Wave-2 stub, filled by the arith-a lane).
//!
//! Owns: statistics (`avg`, `deviate`, `min`, `max`, `stats`, `measure`,
//! `find_trim`, `profile`, `project`), const/linear (`linear`,
//! `remainder_const`, `math2_const`), unary/rounding (`abs`, `sign`, `clamp`,
//! `round`), and hough (`hough_line`, `hough_circle`) per `OP_MAP.md`.

use clap::{ArgMatches, Command};

use super::super::CommandMeta;

/// The clap commands this part contributes (none until the arith-a lane lands).
pub fn commands() -> Vec<Command> {
    Vec::new()
}

/// Static per-command metadata (none until the arith-a lane lands).
pub fn metas() -> Vec<CommandMeta> {
    Vec::new()
}

/// Dispatch a matched command to its handler.
///
/// # Errors
///
/// Bails for any name until the arith-a lane lands (unreachable via the
/// registry, which only routes registered names).
pub fn run(name: &str, _m: &ArgMatches) -> anyhow::Result<()> {
    anyhow::bail!("command {name:?} is not implemented yet (arithmetic part_a stub)")
}
