//! Frequency-filter (Fourier) op family — a per-family Wave-2 lane
//! (`CLI_CONTRACT.md` §3/§5/§6, `OP_MAP.md` freqfilt section).
//!
//! The core `src/freqfilt.rs` exposes twelve `pub fn` that fold to six base ops;
//! those become **five** `viprs` subcommands here (`invfft_real` is not a
//! separate command — it IS `invfft --real`, exactly as vips 8.18.4 folds it
//! into a single `invfft` op with a `--real` boolean, verified against
//! `vips invfft`):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `fwfft IN OUT`             | `fwfft`    | S1 image→image     | FOURIER | real/complex in → complex `.v` out |
//! | `invfft IN OUT [--real]`   | `invfft`   | S1 image→image     | FOURIER | complex `.v` out; `--real` folds `invfft_real` → real `.v` |
//! | `freqmult IN MASK OUT`     | `freqmult` | S2 (2-image)→image | FOURIER | mask is the 2nd input (a real float mask from a `mask_*` creator) |
//! | `spectrum IN OUT`          | `spectrum` | S1 image→image     | FOURIER | displayable log-magnitude — a uchar PNG, still FFT-derived |
//! | `phasecor IN IN2 OUT`      | `phasecor` | S2 (2-image)→image | FOURIER | phase correlation of two images → real `.v` |
//!
//! **Every op is oracle class FOURIER** (`CLI_CONTRACT.md` §5): the outputs are
//! FFT-derived floats compared as f64 band-pairs from the native `.v` container
//! at a stated epsilon. The core runs each transform in `f64` through the pure-Rust
//! `rustfft` planner and stores `f32`; the vips oracle runs FFTW in `double` and
//! stores `dpcomplex` / `double`. The two therefore diverge at the level of
//! `f32` rounding and differing summation order, so the differential compares at
//! a *measured* tolerance rather than tol 0 (see `cli_freqfilt_diff.rs`); `spectrum` and
//! `freqmult` fold their float result back to a `uchar` raster and are compared
//! at the corresponding integer tolerance.
//!
//! Positional orders, flag names, and enum spellings mirror vips 8.18.4 exactly
//! (verified against `vips <op>` usage): `fwfft in out`, `invfft in out [--real]`,
//! `freqmult in mask out`, `spectrum in out`, `phasecor in in2 out`. None of the
//! five ops takes a numeric argument with a value bound, so there is no
//! value-range surface to restrict (unlike the bands lane's `extract_band` /
//! `bandrank`). `freqmult` and `phasecor` take a FIXED pair of named inputs (a
//! mask / a second image), so they are encoded as three explicit positionals —
//! NOT the variadic [`io::inputs_and_out`] idiom, which is for genuinely
//! variadic N-image ops (`bandjoin`) and would over-accept a third input vips
//! rejects. Handlers keep the §3 `load → try_op → save` shape and call only the
//! panic-free `try_*` core APIs, so a bad input becomes exit 1, never an abort
//! (`CLI_CONTRACT.md` §8).
//
// @doc-command:begin name=fwfft about="Forward 2D fast Fourier transform (real or complex input → complex spectrum)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=fwfft
// @doc-command:begin name=invfft about="Inverse 2D fast Fourier transform; --real keeps only the real part." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=invfft
// @doc-command:begin name=freqmult about="Multiply an image by a mask in Fourier space." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=freqmult
// @doc-command:begin name=spectrum about="Make a displayable log-scaled power spectrum." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=spectrum
// @doc-command:begin name=phasecor about="Phase correlation of two images (the peak is their translation)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=phasecor

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Command};

use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6). Every
/// freqfilt op is oracle class **FOURIER** (`CLI_CONTRACT.md` §5).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "fwfft",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Fourier,
        },
        CommandMeta {
            name: "invfft",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Fourier,
        },
        CommandMeta {
            name: "freqmult",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Fourier,
        },
        CommandMeta {
            name: "spectrum",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Fourier,
        },
        CommandMeta {
            name: "phasecor",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Fourier,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("fwfft")
                .about(
                    "Forward 2D fast Fourier transform (real or complex input → complex spectrum).",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output spectrum (write to .v — a complex float-pair raster)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("invfft")
                .about("Inverse 2D fast Fourier transform; --real keeps only the real part.")
                .arg(Arg::new("IN").required(true).help("Input image / spectrum"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output image (write to .v — complex, or real with --real)"),
                )
                .arg(
                    Arg::new("real")
                        .long("real")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Output only the real part of the transform (vips `invfft --real`, \
                             the folded `invfft_real` op)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("freqmult")
                .about("Multiply an image by a mask in Fourier space.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("MASK")
                        .required(true)
                        .value_name("MASK")
                        .help("Input mask image (a real float mask, same size as IN)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("spectrum")
                .about("Make a displayable log-scaled power spectrum.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output spectrum (a uchar image; DC is moved to the centre)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("phasecor")
                .about("Phase correlation of two images (the peak is their translation).")
                .arg(Arg::new("IN").required(true).help("First input image"))
                .arg(
                    Arg::new("IN2")
                        .required(true)
                        .value_name("IN2")
                        .help("Second input image (same size as IN)"),
                )
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output correlation surface (write to .v — a real float raster)"),
                ),
        ),
    ]
}

/// Dispatch a matched freqfilt subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "fwfft" => run_fwfft(m),
        "invfft" => run_invfft(m),
        "freqmult" => run_freqmult(m),
        "spectrum" => run_spectrum(m),
        "phasecor" => run_phasecor(m),
        other => bail!("freqfilt family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// `fwfft IN OUT` — S1; forward FFT (real/complex → complex spectrum).
fn run_fwfft(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=fwfft slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=fwfft slot=load

    // @doc-snippet:begin command=fwfft slot=apply
    let out = raster.try_fwfft()?;
    // @doc-snippet:end command=fwfft slot=apply

    // @doc-snippet:begin command=fwfft slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=fwfft slot=save
    Ok(())
}

/// `invfft IN OUT [--real]` — S1; inverse FFT. `--real` folds `invfft_real`
/// (keep only the real part), exactly as vips's single `invfft --real` op.
fn run_invfft(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let real = m.get_flag("real");

    // @doc-snippet:begin command=invfft slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=invfft slot=load

    // @doc-snippet:begin command=invfft slot=apply
    let out = if real {
        raster.try_invfft_real()?
    } else {
        raster.try_invfft()?
    };
    // @doc-snippet:end command=invfft slot=apply

    // @doc-snippet:begin command=invfft slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=invfft slot=save
    Ok(())
}

/// `freqmult IN MASK OUT` — S2 (fixed 2-image); multiply in Fourier space by a
/// real float mask (the 2nd input). Fixed positionals, NOT the variadic
/// [`io::inputs_and_out`] idiom: the mask is a distinct named input and vips
/// itself declares exactly `freqmult in mask out`.
fn run_freqmult(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let mask_path = PathBuf::from(pos(m, "MASK"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=freqmult slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let mask = io::load(&mask_path, &limits)?;
    // @doc-snippet:end command=freqmult slot=load

    // @doc-snippet:begin command=freqmult slot=apply
    let out = raster.try_freqmult(&mask)?;
    // @doc-snippet:end command=freqmult slot=apply

    // @doc-snippet:begin command=freqmult slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=freqmult slot=save
    Ok(())
}

/// `spectrum IN OUT` — S1; displayable log-scaled power spectrum (a uchar
/// raster with DC moved to the centre).
fn run_spectrum(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=spectrum slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=spectrum slot=load

    // @doc-snippet:begin command=spectrum slot=apply
    let out = raster.try_spectrum()?;
    // @doc-snippet:end command=spectrum slot=apply

    // @doc-snippet:begin command=spectrum slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=spectrum slot=save
    Ok(())
}

/// `phasecor IN IN2 OUT` — S2 (fixed 2-image); phase correlation of two images.
/// Fixed positionals, NOT the variadic idiom (vips declares `phasecor in in2 out`).
fn run_phasecor(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let in2_path = PathBuf::from(pos(m, "IN2"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=phasecor slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let other = io::load(&in2_path, &limits)?;
    // @doc-snippet:end command=phasecor slot=load

    // @doc-snippet:begin command=phasecor slot=apply
    let out = raster.try_phasecor(&other)?;
    // @doc-snippet:end command=phasecor slot=apply

    // @doc-snippet:begin command=phasecor slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=phasecor slot=save
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
        assert_eq!(meta_names.len(), 5, "the freqfilt family has five commands");
    }

    #[test]
    fn all_ops_are_oracle_class_fourier() {
        for meta in metas() {
            assert_eq!(
                meta.oracle_class,
                OracleClass::Fourier,
                "freqfilt op {} must be FOURIER",
                meta.name
            );
        }
    }

    #[test]
    fn fwfft_parses_positionals_in_vips_order() {
        let m = cmd("fwfft")
            .try_get_matches_from(["fwfft", "in.png", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.v");
    }

    #[test]
    fn invfft_real_flag_defaults_false_and_parses() {
        let m = cmd("invfft")
            .clone()
            .try_get_matches_from(["invfft", "in.v", "out.v"])
            .unwrap();
        assert!(!m.get_flag("real"), "--real defaults to false");
        let m = cmd("invfft")
            .try_get_matches_from(["invfft", "in.v", "out.v", "--real"])
            .unwrap();
        assert!(m.get_flag("real"), "--real sets the flag");
    }

    #[test]
    fn freqmult_parses_three_fixed_positionals() {
        let m = cmd("freqmult")
            .try_get_matches_from(["freqmult", "in.png", "mask.v", "out.png"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "MASK"), "mask.v");
        assert_eq!(pos(&m, "OUT"), "out.png");
    }

    #[test]
    fn freqmult_rejects_a_fourth_positional() {
        // A FIXED 2-image op: a third input (which the variadic idiom would
        // wrongly accept) must be rejected at parse time, mirroring vips's own
        // `freqmult in mask out` arity.
        assert!(
            cmd("freqmult")
                .try_get_matches_from(["freqmult", "a.png", "b.v", "c.png", "out.png"])
                .is_err(),
            "freqmult takes exactly IN MASK OUT"
        );
    }

    #[test]
    fn phasecor_parses_three_fixed_positionals() {
        let m = cmd("phasecor")
            .try_get_matches_from(["phasecor", "a.png", "b.png", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "a.png");
        assert_eq!(pos(&m, "IN2"), "b.png");
        assert_eq!(pos(&m, "OUT"), "out.v");
    }

    #[test]
    fn commands_require_their_positionals() {
        // Each op needs all its positionals: a missing OUT is a usage error.
        assert!(
            cmd("fwfft")
                .try_get_matches_from(["fwfft", "in.png"])
                .is_err()
        );
        assert!(
            cmd("phasecor")
                .try_get_matches_from(["phasecor", "a.png", "b.png"])
                .is_err()
        );
    }

    #[test]
    fn run_rejects_unknown_command() {
        let m = cmd("fwfft")
            .try_get_matches_from(["fwfft", "in.png", "out.v"])
            .unwrap();
        assert!(run("nope", &m).is_err(), "an unknown command must error");
    }
}
