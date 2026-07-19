//! Matrix op family — a per-family Wave-2 lane (`CLI_CONTRACT.md` §3/§6,
//! `OP_MAP.md` matrix section).
//!
//! The core `src/matrix.rs` exposes six `pub fn` that fold to three base ops;
//! those become **two** `viprs` subcommands here (`invertlut_size` collapses
//! into `invertlut --size`, exactly as `OP_MAP.md` prescribes). Both are shape
//! **S1** (`IN OUT`) and oracle class **BOUNDED-TOL**:
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `matrixinvert IN.mat OUT`      | `matrixinvert` | S1 | BOUNDED-TOL | dense inverse of a square matrix |
//! | `invertlut IN.mat OUT --size`  | `invertlut`    | S1 | BOUNDED-TOL | inverse LUT from measured `(x, f(x))` rows |
//!
//! # A matrix-in / matrix-out family, not an image one
//!
//! Unlike every image op, the INPUT here is a **matrix file** — vips's text
//! matrix format (`width height [scale [offset]]` then a flat value stream),
//! loaded through the shared [`MatFile`](super::matfile::MatFile) loader and
//! reconstructed into a one-band `Interpretation::Matrix` raster via the core
//! [`Raster::try_from_matrix`]. There is no image to decode, so these commands
//! carry **no** `--max-*` decode-limit flags (the matrix loader has its own
//! file-size / element-count caps).
//!
//! # Carrier & oracle (BOUNDED-TOL, f32 not f64)
//!
//! Both ops produce a **float matrix** (`matrixinvert` → `Interpretation::Matrix`,
//! `invertlut` → `Interpretation::Histogram`), written to the native `.v`
//! container. The core computes in `f64` but stores results as **`f32`**
//! (libvips stores `double`), so the differential compares f32-viprs against the
//! vips double result **cast to float** (`vips cast … float`) — the libviprs
//! `.v` decoder rejects a DOUBLE band format, and the compare harness reads f32
//! samples. Because the core is a faithful port of libvips' `matrixinvert.c` /
//! `invertlut.c` (identical operation order), the two f64 computations agree and
//! the f32-cast results match to within f32 rounding, i.e. **BOUNDED-TOL** at a
//! small f32 epsilon — NOT the 1e-9 f64 tol the OP_MAP note assumed for a
//! double carrier (see the wave report open question).
//!
//! # No panics on user input
//!
//! Every handler calls only fallible core APIs (`try_from_matrix`,
//! `try_matrixinvert`, `try_invertlut_size`); a singular / non-square /
//! out-of-range / bad-size matrix becomes a typed exit-1 error rather than a
//! `#[track_caller]` panic (`CLI_CONTRACT.md` §8). Clap enforces the vips
//! `--size` bound (`1..=1000000`) at parse time; the i64→u32 narrowing that
//! follows is a typed error, never an unchecked cast.
//
// @doc-command:begin name=matrixinvert about="Invert a square matrix read from a vips text-matrix file." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=matrixinvert
// @doc-command:begin name=invertlut about="Build an inverse look-up table from a matrix of measured (x, f(x)) rows." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=invertlut

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::Raster;

use super::matfile::MatFile;
use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            // BOUNDED-TOL: core computes in f64 but stores f32, so the f32-cast
            // vips double result agrees only to f32 rounding (see module header
            // and OP_MAP.md).
            name: "matrixinvert",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::ExactAfterCast,
        },
        CommandMeta {
            name: "invertlut",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Neither command decodes an image (the input is a matrix FILE loaded through
/// [`MatFile`]), so — unlike the image families — they carry NO `--max-*`
/// decode-limit flags.
pub fn commands() -> Vec<Command> {
    vec![
        Command::new("matrixinvert")
            .about("Invert a square matrix read from a vips text-matrix file.")
            .arg(
                Arg::new("IN")
                    .required(true)
                    .value_name("IN.mat")
                    .help("Input square matrix as a vips text-matrix file"),
            )
            .arg(
                Arg::new("OUT")
                    .required(true)
                    .help("Output matrix path (use .v: a float matrix carrier)"),
            ),
        Command::new("invertlut")
            .about("Build an inverse look-up table from a matrix of measured (x, f(x)) rows.")
            .arg(Arg::new("IN").required(true).value_name("IN.mat").help(
                "Input matrix of measured points (column 0 = input level, each further \
                         column = one band's response, all in 0..=1), as a vips text-matrix file",
            ))
            .arg(
                Arg::new("OUT")
                    .required(true)
                    .help("Output LUT path (use .v: a float matrix carrier)"),
            )
            .arg(
                Arg::new("size")
                    .long("size")
                    .value_name("N")
                    .default_value("256")
                    // vips's own declared range is 1..=1000000; mirror it exactly
                    // rather than exposing the core's tighter 1..=65536 (a size in
                    // 65537..=1000000 parses here and becomes a typed exit-1
                    // core error, never a panic — see run_invertlut).
                    .value_parser(value_parser!(i64).range(1..=1_000_000))
                    .help("LUT size to generate (vips range 1..=1000000, default 256)"),
            ),
    ]
}

/// Dispatch a matched matrix subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "matrixinvert" => run_matrixinvert(m),
        "invertlut" => run_invertlut(m),
        other => bail!("matrix family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Load a vips text-matrix file into a one-band `Interpretation::Matrix` raster
/// via the shared [`MatFile`] loader + the core [`Raster::try_from_matrix`].
///
/// The header `scale` / `offset` are irrelevant to both matrix ops (vips's
/// `matrixinvert` / `invertlut` read the raw cell values), so only the value
/// grid is used — mirroring vips's matrix load with the default scale 1 /
/// offset 0.
fn load_matrix(in_path: &Path) -> Result<Raster> {
    let mat = MatFile::load(in_path)?;
    let rows: Vec<Vec<f64>> = mat.as_f64_rows().into_iter().map(<[f64]>::to_vec).collect();
    Ok(Raster::try_from_matrix(&rows)?)
}

/// `matrixinvert IN.mat OUT` — S1; dense inverse of a square matrix.
fn run_matrixinvert(m: &ArgMatches) -> Result<()> {
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=matrixinvert slot=load imports=decode_file
    let matrix = load_matrix(&in_path)?;
    // @doc-snippet:end command=matrixinvert slot=load

    // @doc-snippet:begin command=matrixinvert slot=apply
    // @doc-test: matrix.rs::matrixinvert_2x2_analytic:516 repo=libviprs
    let out = matrix.try_matrixinvert()?;
    // @doc-snippet:end command=matrixinvert slot=apply

    // @doc-snippet:begin command=matrixinvert slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=matrixinvert slot=save
    Ok(())
}

/// `invertlut IN.mat OUT --size N` — S1; inverse LUT from measured points.
fn run_invertlut(m: &ArgMatches) -> Result<()> {
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    // Clap's `1..=1000000` bound guarantees a positive, u32-representable value;
    // the narrowing is still a typed exit-1 error, never an unchecked `as` cast
    // that could abort (CLI_CONTRACT.md §8 / the bands B2 lesson).
    let size_raw = *m.get_one::<i64>("size").expect("clap default 256");
    let size = u32::try_from(size_raw)
        .map_err(|_| anyhow!("--size {size_raw} is out of range (expected 1..=1000000)"))?;

    // @doc-snippet:begin command=invertlut slot=load imports=decode_file
    let matrix = load_matrix(&in_path)?;
    // @doc-snippet:end command=invertlut slot=load

    // @doc-snippet:begin command=invertlut slot=apply
    // @doc-test: matrix.rs::invertlut_ported_table:655 repo=libviprs
    let out = matrix.try_invertlut_size(size)?;
    // @doc-snippet:end command=invertlut slot=apply

    // @doc-snippet:begin command=invertlut slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=invertlut slot=save
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
        assert_eq!(meta_names.len(), 2, "the matrix family has two commands");
    }

    #[test]
    fn matrixinvert_parses_positionals_in_vips_order() {
        let m = cmd("matrixinvert")
            .try_get_matches_from(["matrixinvert", "m.mat", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "m.mat");
        assert_eq!(pos(&m, "OUT"), "out.v");
    }

    #[test]
    fn invertlut_parses_in_out_and_size() {
        let m = cmd("invertlut")
            .try_get_matches_from(["invertlut", "lut.mat", "out.v", "--size", "64"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "lut.mat");
        assert_eq!(pos(&m, "OUT"), "out.v");
        assert_eq!(*m.get_one::<i64>("size").unwrap(), 64);
    }

    #[test]
    fn invertlut_size_defaults_to_256() {
        let m = cmd("invertlut")
            .try_get_matches_from(["invertlut", "lut.mat", "out.v"])
            .unwrap();
        assert_eq!(*m.get_one::<i64>("size").unwrap(), 256);
    }

    #[test]
    fn invertlut_rejects_size_below_one() {
        // vips's minimum size is 1; the `1..=1000000` value_parser rejects 0 at
        // parse time (strict parity — no sub-1 extension).
        assert!(
            cmd("invertlut")
                .try_get_matches_from(["invertlut", "lut.mat", "out.v", "--size", "0"])
                .is_err(),
            "a --size below 1 must be rejected"
        );
    }

    #[test]
    fn invertlut_rejects_size_above_vips_max() {
        // vips's declared maximum is 1000000; anything larger is rejected at
        // parse time (no core-only extension beyond the vips surface).
        assert!(
            cmd("invertlut")
                .try_get_matches_from(["invertlut", "lut.mat", "out.v", "--size", "1000001"])
                .is_err(),
            "a --size above 1000000 must be rejected"
        );
    }

    #[test]
    fn matrixinvert_singular_input_is_error_not_panic() {
        // A singular matrix is a typed exit-1 error via the fallible core API,
        // NOT a #[track_caller] panic/abort (CLI_CONTRACT.md §8). The matrix is
        // built in memory so no fixture file is needed.
        let matrix = Raster::try_from_matrix(&[vec![1.0, 2.0], vec![2.0, 4.0]]).unwrap();
        assert!(
            matrix.try_matrixinvert().is_err(),
            "a singular matrix must be a typed error"
        );
    }

    #[test]
    fn invertlut_size_narrowing_is_within_range() {
        // Every size clap admits (1..=1000000) narrows to u32 without error.
        for &s in &[1_i64, 256, 65536, 1_000_000] {
            assert!(u32::try_from(s).is_ok(), "size {s} must narrow to u32");
        }
    }
}
