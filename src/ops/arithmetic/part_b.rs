//! Arithmetic family — **part B** (Wave-2 stub, filled by the arith-b lane).
//!
//! Owns: binary/N-ary (`subtract`, `multiply`, `divide`, `minpair`, `maxpair`,
//! `sum`), relational (`relational`, `relational_const`), boolean (`boolean`,
//! `boolean_const`), windowed (`scale`, `stdif`, `recomb`, `premultiply`,
//! `unpremultiply`), math (`math`, `math2`), and complex/Fourier (`complexform`,
//! `complex`, `complexget`) per `OP_MAP.md`.

use clap::{ArgMatches, Command};

use super::super::CommandMeta;

/// The clap commands this part contributes (none until the arith-b lane lands).
pub fn commands() -> Vec<Command> {
    Vec::new()
}

/// Static per-command metadata (none until the arith-b lane lands).
pub fn metas() -> Vec<CommandMeta> {
    Vec::new()
}

/// Dispatch a matched command to its handler.
///
/// # Errors
///
/// Bails for any name until the arith-b lane lands (unreachable via the
/// registry, which only routes registered names).
pub fn run(name: &str, _m: &ArgMatches) -> anyhow::Result<()> {
    anyhow::bail!("command {name:?} is not implemented yet (arithmetic part_b stub)")
}
