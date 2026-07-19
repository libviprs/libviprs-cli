//! Core raster ops (`add` / `getpoint`) op family — the Wave-2 **core** lane
//! (`CLI_CONTRACT.md` §1/§2/§8, `OP_MAP.md` core section).
//!
//! `add` and `getpoint` live in the core crate's `src/raster_ops.rs` (outside
//! the sixteen audited op modules) but the frozen contract hard-codes both
//! (`viprs add a.png b.png out.png` §1; `vips getpoint` `.5` fixtures §2). Two
//! vips ops → two `viprs` subcommands, covering two of the six §3 shapes:
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `add LEFT RIGHT OUT`  | `add`      | S2 (fixed 2-image → OUT) | EXACT-AFTER-CAST | **uchar-only, equal-bands** surface: uchar+uchar → ushort widening compares tol 0. 16-bit and float inputs are REJECTED (exit 1), not silently saturated/computed — see the band/depth boundary notes below |
//! | `getpoint IN X Y`     | `getpoint` | S3 image→stdout-scalar   | EXACT | prints the per-band pixel vector in vips numeric format (numeric compare; stdout text formatting is out of §9 scope — see [`fmt_vips_vec`]) |
//!
//! # Accepted `add` surface = the set where core == vips (`OP_MAP.md` core `add`)
//!
//! Core `Raster::add` matches vips `add` ONLY on 8-bit (uchar), equal-band
//! inputs, so the CLI restricts the accepted surface to exactly that set — every
//! rejected input is one where core would diverge from vips:
//!
//! * **16-bit / float depth** — vips `add` PROMOTES `ushort → uint` (and float
//!   stays float), returning the true sum, whereas core keeps a 16-bit input at
//!   16-bit and SATURATES the sum at `65535` (`raster_ops.rs`), and panics on
//!   float. Both are rejected here as typed exit-1 errors (a later arithmetic
//!   wave owns wide/float addition), so `add` never emits a silently-saturated
//!   16-bit result. Filed as a core-side follow-up (16-bit promotion parity).
//! * **Unequal band counts** — vips `add` BAND-BROADCASTS a 1-band operand
//!   across a multi-band one (`add rgb gray → per-band rgb+gray`), whereas core
//!   asserts equal band counts. This is a documented SUBSET limitation, not
//!   parity: core cannot broadcast without a core change. The CLI keeps the
//!   exit-1 rejection; band-broadcast parity is a core-side follow-up.
//!
//! Positional orders, flag names, and input value bounds mirror vips 8.18.4
//! exactly (verified against `vips add` = `add left right out` and
//! `vips getpoint` = `getpoint in x y`, both with `x`/`y` `min: 0`). `add` is
//! **not** variadic — vips's `add` is strictly binary (only `sum`/`bandjoin`
//! take an image array), so it is three fixed positionals rather than the S2
//! `num_args(2..)` idiom.
//!
//! # No panics on user input (`CLI_CONTRACT.md` §8)
//!
//! Unlike most families, the core ops have **no** `try_*` variant: `Raster::add`
//! and `Raster::getpoint` are `#[track_caller]` panicking APIs
//! (`raster_ops.rs`). Every condition that would abort them is therefore
//! pre-validated here and turned into a typed exit-1 error:
//!
//! * `add` panics on a dimension mismatch, a channel-count mismatch, or a float
//!   input — all three are checked before the call (float inputs land with a
//!   later arithmetic batch; `OP_MAP.md` core `add` note). A 16-bit input is
//!   also rejected here (core silently saturates it at `65535` instead of
//!   promoting like vips — see the surface notes above), and
//! * `getpoint` panics when `(x, y)` is outside the raster — the coordinate is
//!   bounds-checked against the decoded dimensions before the call.
//
// @doc-command:begin name=add about="Add two images pixel-by-pixel (8-bit widens to 16-bit to avoid wrap)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=add
// @doc-command:begin name=getpoint about="Read one pixel and print its per-band values in vips numeric format." \
//     slot-order=load,apply imports-base=decode_file
// @doc-command:end name=getpoint

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command, value_parser};

use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            // EXACT-AFTER-CAST (`OP_MAP.md`): `add` widens like vips promotion
            // (uchar+uchar → ushort). The output is integer, so the §2 save-cast
            // is a no-op and the differential compares at tol 0. The accepted
            // surface is uchar-only, equal-bands (the set where core == vips):
            // 16-bit and float inputs are rejected (core would saturate/panic
            // where vips promotes; a later arithmetic batch owns them).
            name: "add",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::ExactAfterCast,
        },
        CommandMeta {
            name: "getpoint",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Both commands load at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("add")
                .about("Add two images pixel-by-pixel (8-bit widens to 16-bit to avoid wrap).")
                // vips `add left right out` — strictly binary (NOT an image
                // array), so three fixed positionals rather than the variadic
                // `num_args(2..)` S2 idiom.
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
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("getpoint")
                .about("Read one pixel and print its per-band values in vips numeric format.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("X")
                        .required(true)
                        // vips's minimum is 0 (`getpoint … x` is `input gint`,
                        // `min: 0`); a u32 parser rejects negatives at parse time
                        // (exit 2) with no negative-from-edge extension vips lacks.
                        .value_parser(value_parser!(u32))
                        .help("Column to read (>= 0)"),
                )
                .arg(
                    Arg::new("Y")
                        .required(true)
                        .value_parser(value_parser!(u32))
                        .help("Row to read (>= 0)"),
                ),
        ),
    ]
}

/// Dispatch a matched core subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "add" => run_add(m),
        "getpoint" => run_getpoint(m),
        other => bail!("core family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// `add LEFT RIGHT OUT` — S2 (fixed binary); pixel-by-pixel sum with 8→16-bit
/// widening.
///
/// The core `Raster::add` PANICS on a dimension / channel mismatch or a float
/// input (it has no `try_` twin), so all three are pre-validated into typed
/// exit-1 errors before the call (`CLI_CONTRACT.md` §8).
fn run_add(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let left_path = PathBuf::from(pos(m, "LEFT"));
    let right_path = PathBuf::from(pos(m, "RIGHT"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=add slot=load imports=decode_file
    let left = io::load(&left_path, &limits)?;
    let right = io::load(&right_path, &limits)?;
    // @doc-snippet:end command=add slot=load

    // @doc-snippet:begin command=add slot=apply
    // @doc-test: raster_ops.rs::add_colour_per_band:200 repo=libviprs
    // Guard the three conditions core's `add` asserts on, so a bad input is exit
    // 1 (a clear message) and NEVER a process abort (exit 101).
    if left.format().is_float() || right.format().is_float() {
        bail!(
            "add: float inputs are not supported yet (a later arithmetic batch adds float \
             addition); cast to an unsigned 8-bit format first"
        );
    }
    // Reject 16-bit (and any non-uchar) inputs: vips `add` promotes ushort→uint
    // and returns the true sum, but core keeps 16-bit at 16-bit and SATURATES at
    // 65535 (raster_ops.rs). Accepting them would emit a silently-wrong result,
    // so the accepted surface is restricted to 8-bit (uchar), where core == vips.
    // Wide-input addition lands with a later arithmetic batch (see the module
    // docs / OP_MAP `add` note).
    if left.format().bytes_per_channel() != 1 || right.format().bytes_per_channel() != 1 {
        bail!(
            "add: 16-bit inputs are not supported yet (a later arithmetic batch adds \
             ushort→uint promotion; core would saturate the sum at 65535 instead of \
             promoting like vips); cast to an unsigned 8-bit format first"
        );
    }
    if (left.width(), left.height()) != (right.width(), right.height()) {
        bail!(
            "add: dimension mismatch — LEFT is {}x{} but RIGHT is {}x{}",
            left.width(),
            left.height(),
            right.width(),
            right.height()
        );
    }
    if left.format().channels() != right.format().channels() {
        bail!(
            "add: channel-count mismatch — LEFT has {} band(s) but RIGHT has {}",
            left.format().channels(),
            right.format().channels()
        );
    }
    let out = left.add(&right);
    // @doc-snippet:end command=add slot=apply

    // @doc-snippet:begin command=add slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=add slot=save
    Ok(())
}

/// `getpoint IN X Y` — S3 image→stdout-scalar; print the per-band pixel vector.
///
/// The core `Raster::getpoint` PANICS when `(x, y)` is out of bounds (it has no
/// `try_` twin), so the coordinate is bounds-checked against the decoded
/// dimensions before the call (`CLI_CONTRACT.md` §8).
fn run_getpoint(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let x = *m.get_one::<u32>("X").expect("required positional");
    let y = *m.get_one::<u32>("Y").expect("required positional");

    // @doc-snippet:begin command=getpoint slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=getpoint slot=load

    // @doc-snippet:begin command=getpoint slot=apply
    // @doc-test: raster_ops.rs::getpoint_reads_bands_8bit:161 repo=libviprs
    // Bounds-check before the call: core's getpoint asserts `x < width` /
    // `y < height`, so an out-of-range coordinate must become exit 1, not abort.
    if x >= raster.width() || y >= raster.height() {
        bail!(
            "getpoint: coordinate ({x}, {y}) is out of bounds for a {}x{} image",
            raster.width(),
            raster.height()
        );
    }
    let pixel = raster.getpoint(x, y);
    // @doc-snippet:end command=getpoint slot=apply

    println!("{}", fmt_vips_vec(&pixel));
    Ok(())
}

/// Format a per-band pixel vector in the vips `getpoint` print format:
/// space-separated values on one line. Integer-valued samples print without a
/// decimal point (`10 20 30`), floats print their natural decimal form
/// (`0.5 1.25 2.5`), matching vips 8.18.4 for dyadic values.
///
/// **stdout text formatting is NOT a pinned parity surface (§9 missing-scope);
/// the real oracle is the numeric compare, not the text.** Each f32 sample is
/// widened to f64 and printed via `f64::to_string` (e.g. f32 `0.1` prints
/// `0.10000000149011612`). vips's own `getpoint` text format for a non-dyadic
/// float is NOT contracted here and can differ by value class or vips build, so
/// the differential never text-compares: it float-PARSES each value and compares
/// with an epsilon (`CLI_CONTRACT.md` §3), so the numeric value (within tol),
/// never the exact text, carries the case. The `getpoint_float_nd` differential
/// case (a non-dyadic `[0.1 0.2 0.3]` float) pins exactly this — it de-rigs the
/// deliberately-dyadic `getpoint_float` fixture by proving the numeric-eps
/// compare, not a dyadic text match, is what carries a float getpoint case.
fn fmt_vips_vec(vals: &[f64]) -> String {
    vals.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
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
        assert_eq!(meta_names.len(), 2, "the core family has two commands");
    }

    #[test]
    fn add_parses_three_positionals_in_vips_order() {
        let m = cmd("add")
            .try_get_matches_from(["add", "a.png", "b.png", "out.png"])
            .unwrap();
        assert_eq!(pos(&m, "LEFT"), "a.png");
        assert_eq!(pos(&m, "RIGHT"), "b.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
    }

    #[test]
    fn add_requires_exactly_three_positionals() {
        // Two positionals (missing OUT) is a usage error …
        assert!(
            cmd("add")
                .clone()
                .try_get_matches_from(["add", "a.png", "b.png"])
                .is_err(),
            "add needs LEFT RIGHT OUT"
        );
        // … and a fourth positional is rejected too (add is strictly binary in
        // vips, NOT a variadic image array like sum/bandjoin).
        assert!(
            cmd("add")
                .try_get_matches_from(["add", "a.png", "b.png", "c.png", "out.png"])
                .is_err(),
            "add must reject a fourth positional (it is not variadic)"
        );
    }

    #[test]
    fn getpoint_parses_in_and_coords() {
        let m = cmd("getpoint")
            .try_get_matches_from(["getpoint", "in.png", "3", "5"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(*m.get_one::<u32>("X").unwrap(), 3);
        assert_eq!(*m.get_one::<u32>("Y").unwrap(), 5);
    }

    #[test]
    fn getpoint_rejects_a_negative_coordinate() {
        // vips's minimum x/y is 0; the u32 parser rejects negatives at parse time
        // (exit 2) rather than offering a negative-from-edge extension vips lacks.
        assert!(
            cmd("getpoint")
                .clone()
                .try_get_matches_from(["getpoint", "in.png", "-1", "0"])
                .is_err(),
            "a negative X must be rejected"
        );
        assert!(
            cmd("getpoint")
                .try_get_matches_from(["getpoint", "in.png", "0", "-1"])
                .is_err(),
            "a negative Y must be rejected"
        );
    }

    #[test]
    fn fmt_vips_vec_matches_vips_numeric_format() {
        // Integer-valued samples: bare integers, space-separated (as vips prints
        // `10 20 30` for an integer image).
        assert_eq!(fmt_vips_vec(&[10.0, 20.0, 30.0]), "10 20 30");
        // Single-band pixel.
        assert_eq!(fmt_vips_vec(&[77.0]), "77");
        // Float samples: natural decimal form (vips prints `0.5 1.25 2.5`).
        assert_eq!(fmt_vips_vec(&[0.5, 1.25, 2.5]), "0.5 1.25 2.5");
    }
}
