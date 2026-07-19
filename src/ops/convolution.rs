//! Convolution / correlation op family — a Wave-2 per-family lane
//! (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md` convolution section).
//!
//! The core `src/convolution.rs` exposes 22 `pub fn` that fold to nine base
//! ops; those become **nine** `viprs` subcommands here, covering four of the
//! six §3 command shapes:
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `gaussmat OUT SIGMA MIN-AMPL [--separable] [--precision]` | `gaussmat` | S5 creator | BOUNDED-TOL | matrix → float `.v`; integer precision tol 0 |
//! | `logmat OUT SIGMA MIN-AMPL [--separable] [--precision]`   | `logmat`   | S5 creator | BOUNDED-TOL | as gaussmat |
//! | `conv IN OUT MASK.mat [--precision]`                      | `conv`     | S1 | EXACT | matrix FILE arg; integer precision tol 0, float `.v` |
//! | `convsep IN OUT MASK.mat [--precision]`                   | `convsep`  | S1 | EXACT | separable (1xN / Nx1) mask |
//! | `compass IN OUT MASK.mat [--times] [--angle] [--combine] [--precision]` | `compass` | S1 | BOUNDED-TOL | rotating odd-square mask |
//! | `gaussblur IN OUT SIGMA [--min-ampl] [--precision]`       | `gaussblur`| S1 | EXACT | separable Gaussian; integer precision tol 0 |
//! | `sharpen IN OUT [--sigma] [--m1] [--m2]`                  | `sharpen`  | S1 | BOUNDED-TOL | LabS unsharp mask |
//! | `spcor IN REF OUT`                                        | `spcor`    | S2 | BOUNDED-TOL | normalised cross-correlation → float `.v` |
//! | `fastcor IN REF OUT`                                      | `fastcor`  | S2 | EXACT | sum-of-squared-differences → float `.v` |
//!
//! Positional orders, flag names, enum spellings, and input value bounds mirror
//! vips 8.18.4 exactly (verified against `vips <op>`): `gaussmat`/`logmat` take
//! `sigma min-ampl` positionally with optional `--separable` / `--precision`;
//! `conv`/`convsep`/`compass` read the mask as a **matrix FILE** through the
//! shared [`MatFile`] loader (`CLI_CONTRACT.md` §3/§9). vips's `approximate`
//! precision and `compass --combine min` are intentionally NOT exposed — the
//! core [`Precision`] / [`Combine`] enums do not implement them, so offering the
//! spellings would be invented parity.
//!
//! The two mask creators (`gaussmat`, `logmat`) produce a matrix, not an image:
//! the coefficient grid is written as a single-band float raster to the native
//! `.v` container ([`io::save`]), the carrier the differential decode-compares.
//!
//! The emitted matrix `.v` **intentionally carries no `scale`/`offset` header**
//! ([`save_matrix`] writes only the coefficient grid as a plain float raster).
//! vips's `gaussmat`/`logmat` `.v` carries a `scale` (and `offset`) header entry
//! that a vips consumer would read; viprs's raster carrier drops it. The raster
//! differential samples only the coefficient grid, never the header, so **the
//! `Kernel.scale` computed by these creators is NOT pinned by the differential**.
//! `gaussmat`'s scale is exercised indirectly (the pinned `gaussblur` builds a
//! `gaussmat` internally and divides by its scale), but `logmat`'s scale feeds no
//! downstream op in the suite and so is unverified — a wrong `logmat` scale would
//! be invisible here. This is a deliberate carrier limitation, not an oversight.
//!
//! `spcor` / `fastcor` output 32-bit float surfaces (the core returns a float
//! image for both correlations), also carried as `.v`.
//!
//! Handlers keep the §3 `load → try_op → save` shape and call only the
//! panic-free `try_*` core APIs, so a bad input becomes exit 1 rather than an
//! abort (`CLI_CONTRACT.md` §8). No `unwrap`/`expect`/`as`-cast on user input
//! can abort: every `usize`/`u32` matrix-dimension conversion is guarded.
//
// @doc-command:begin name=gaussmat about="Make a Gaussian mask matrix." \
//     slot-order=apply,save imports-base=save_file
// @doc-command:end name=gaussmat
// @doc-command:begin name=logmat about="Make a Laplacian-of-Gaussian mask matrix." \
//     slot-order=apply,save imports-base=save_file
// @doc-command:end name=logmat
// @doc-command:begin name=conv about="Convolve an image with a mask matrix." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=conv
// @doc-command:begin name=convsep about="Separably convolve an image with a 1xN or Nx1 mask." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=convsep
// @doc-command:begin name=compass about="Convolve with a rotating compass mask and combine the results." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=compass
// @doc-command:begin name=gaussblur about="Gaussian-blur an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=gaussblur
// @doc-command:begin name=sharpen about="Unsharp-mask an image for print." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=sharpen
// @doc-command:begin name=spcor about="Normalised spatial cross-correlation of an image against a template." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=spcor
// @doc-command:begin name=fastcor about="Fast (sum-of-squared-differences) correlation of an image against a template." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=fastcor

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::{Angle45, Combine, Kernel, PixelFormat, Precision, Raster};

use super::matfile::MatFile;
use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "gaussmat",
            shape: Shape::Creator,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "logmat",
            shape: Shape::Creator,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "conv",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "convsep",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "compass",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "gaussblur",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "sharpen",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "spcor",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "fastcor",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
    ]
}

/// A `--precision integer|float` flag with the given vips default. The core
/// [`Precision`] enum has no `approximate` variant, so that vips spelling is
/// intentionally not offered (no invented parity).
fn precision_arg(default: &'static str) -> Arg {
    Arg::new("precision")
        .long("precision")
        .value_name("integer|float")
        .default_value(default)
        .value_parser(["integer", "float"])
        .help("Calculation precision (integer|float; vips's approximate is not core-backed)")
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every image-loading command carries the shared decode-limit flags via
/// [`io::with_decode_limit_args`]. `gaussmat` / `logmat` are pure creators and
/// load no image, so they omit them (S5, `CLI_CONTRACT.md` §3.5).
pub fn commands() -> Vec<Command> {
    vec![
        // gaussmat / logmat — S5 creators (OUT first). No image input, so no
        // decode-limit flags. vips: `<op> out sigma min-ampl [--separable]
        // [--precision]`; sigma / min-ampl are gdouble in 1e-06..=10000 (the
        // core validates the range and returns a typed exit-1 error).
        Command::new("gaussmat")
            .about("Make a Gaussian mask matrix.")
            .arg(Arg::new("OUT").required(true).help("Output matrix (.v)"))
            .arg(
                Arg::new("SIGMA")
                    .required(true)
                    .value_parser(value_parser!(f64))
                    .help("Sigma of the Gaussian (1e-06..=10000)"),
            )
            .arg(
                Arg::new("MIN-AMPL")
                    .required(true)
                    .value_parser(value_parser!(f64))
                    .help("Minimum amplitude of the Gaussian (1e-06..=10000)"),
            )
            .arg(
                Arg::new("separable")
                    .long("separable")
                    .action(ArgAction::SetTrue)
                    .help("Generate the separable (1xN centre row) form"),
            )
            .arg(precision_arg("integer")),
        Command::new("logmat")
            .about("Make a Laplacian-of-Gaussian mask matrix.")
            .arg(Arg::new("OUT").required(true).help("Output matrix (.v)"))
            .arg(
                Arg::new("SIGMA")
                    .required(true)
                    .value_parser(value_parser!(f64))
                    .help("Radius of the Gaussian (1e-06..=10000)"),
            )
            .arg(
                Arg::new("MIN-AMPL")
                    .required(true)
                    .value_parser(value_parser!(f64))
                    .help("Minimum amplitude of the Gaussian (1e-06..=10000)"),
            )
            .arg(
                Arg::new("separable")
                    .long("separable")
                    .action(ArgAction::SetTrue)
                    .help("Generate the separable (1xN centre row) form"),
            )
            .arg(precision_arg("integer")),
        // conv / convsep — S1 with a matrix FILE mask arg. vips default
        // precision is float.
        io::with_decode_limit_args(
            Command::new("conv")
                .about("Convolve an image with a mask matrix.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("MASK")
                        .required(true)
                        .value_name("MASK.mat")
                        .help("Convolution mask as a vips text matrix (with an optional scale)"),
                )
                .arg(precision_arg("float")),
        ),
        io::with_decode_limit_args(
            Command::new("convsep")
                .about("Separably convolve an image with a 1xN or Nx1 mask.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("MASK")
                        .required(true)
                        .value_name("MASK.mat")
                        .help("Separable (1xN or Nx1) mask as a vips text matrix"),
                )
                .arg(precision_arg("float")),
        ),
        // compass — S1, rotating mask + combine. vips defaults: times 2, angle
        // d90, combine max, precision float.
        io::with_decode_limit_args(
            Command::new("compass")
                .about("Convolve with a rotating compass mask and combine the results.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("MASK")
                        .required(true)
                        .value_name("MASK.mat")
                        .help("Odd-sided square mask as a vips text matrix"),
                )
                .arg(
                    Arg::new("times")
                        .long("times")
                        .value_name("N")
                        .default_value("2")
                        // vips bounds times at 1..=1000; reject anything outside
                        // rather than silently folding it (strict parity).
                        .value_parser(value_parser!(u32).range(1..=1000))
                        .help("Rotate and convolve this many times (1..=1000)"),
                )
                .arg(
                    Arg::new("angle")
                        .long("angle")
                        .value_name("d0|d45|…|d315")
                        .default_value("d90")
                        .value_parser(["d0", "d45", "d90", "d135", "d180", "d225", "d270", "d315"])
                        .help("Rotate the mask by this much between convolutions"),
                )
                .arg(
                    Arg::new("combine")
                        .long("combine")
                        .value_name("max|sum")
                        .default_value("max")
                        .value_parser(["max", "sum"])
                        .help("Combine the results (max|sum; vips's min is not core-backed)"),
                )
                .arg(precision_arg("float")),
        ),
        // gaussblur — S1. vips: `gaussblur in out sigma [--min-ampl]
        // [--precision]`; sigma gdouble 0..=1000, min-ampl 0.001..=1 default
        // 0.2, precision default integer.
        io::with_decode_limit_args(
            Command::new("gaussblur")
                .about("Gaussian-blur an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("SIGMA")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Sigma of the Gaussian (0..=1000)"),
                )
                .arg(
                    Arg::new("min-ampl")
                        .long("min-ampl")
                        .value_name("A")
                        .default_value("0.2")
                        .value_parser(value_parser!(f64))
                        .help("Minimum amplitude of the Gaussian (0.001..=1)"),
                )
                .arg(precision_arg("integer")),
        ),
        // sharpen — S1. Core exposes only sigma / m1 / m2; vips's x1 / y2 / y3
        // stay at the shared libvips defaults (2 / 10 / 20), which the core
        // hard-codes, so they are not exposed (documented subset, not parity).
        io::with_decode_limit_args(
            Command::new("sharpen")
                .about("Unsharp-mask an image for print.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (sRGB / mono)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("sigma")
                        .long("sigma")
                        .value_name("S")
                        .default_value("0.5")
                        .value_parser(value_parser!(f64))
                        .help("Sigma of the blur Gaussian (1e-06..=10)"),
                )
                .arg(
                    Arg::new("m1")
                        .long("m1")
                        .value_name("M")
                        .default_value("0")
                        .value_parser(value_parser!(f64))
                        .help("Slope for flat areas (vips default 0)"),
                )
                .arg(
                    Arg::new("m2")
                        .long("m2")
                        .value_name("M")
                        .default_value("3")
                        .value_parser(value_parser!(f64))
                        .help("Slope for jaggy areas (vips default 3)"),
                ),
        ),
        // spcor / fastcor — S2 with EXACTLY two image inputs then OUT (a fixed
        // arity, so three plain positionals — not the variadic idiom).
        io::with_decode_limit_args(
            Command::new("spcor")
                .about("Normalised spatial cross-correlation of an image against a template.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("REF")
                        .required(true)
                        .help("Template / reference image"),
                )
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output correlation surface (.v)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("fastcor")
                .about(
                    "Fast (sum-of-squared-differences) correlation of an image against a template.",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("REF")
                        .required(true)
                        .help("Template / reference image"),
                )
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output correlation surface (.v)"),
                ),
        ),
    ]
}

/// Dispatch a matched convolution subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "gaussmat" => run_gaussmat(m),
        "logmat" => run_logmat(m),
        "conv" => run_conv(m),
        "convsep" => run_convsep(m),
        "compass" => run_compass(m),
        "gaussblur" => run_gaussblur(m),
        "sharpen" => run_sharpen(m),
        "spcor" => run_spcor(m),
        "fastcor" => run_fastcor(m),
        other => bail!("convolution family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Map the `--precision` flag to the core [`Precision`].
fn precision(m: &ArgMatches) -> Result<Precision> {
    match pos(m, "precision") {
        "integer" => Ok(Precision::Integer),
        "float" => Ok(Precision::Float),
        other => bail!("unknown precision {other:?} (expected integer|float)"),
    }
}

/// Map the `--angle` flag to the core [`Angle45`].
fn angle45(s: &str) -> Result<Angle45> {
    Ok(match s {
        "d0" => Angle45::D0,
        "d45" => Angle45::D45,
        "d90" => Angle45::D90,
        "d135" => Angle45::D135,
        "d180" => Angle45::D180,
        "d225" => Angle45::D225,
        "d270" => Angle45::D270,
        "d315" => Angle45::D315,
        other => bail!("unknown angle {other:?} (expected d0|d45|…|d315)"),
    })
}

/// Map the `--combine` flag to the core [`Combine`].
fn combine(s: &str) -> Result<Combine> {
    match s {
        "max" => Ok(Combine::Max),
        "sum" => Ok(Combine::Sum),
        other => bail!("unknown combine {other:?} (expected max|sum)"),
    }
}

/// Load a `.mat` mask file into a convolution [`Kernel`] (coefficients + header
/// scale). Shared by `conv` / `convsep` / `compass` (`CLI_CONTRACT.md` §3/§9).
fn load_kernel(path: &Path) -> Result<Kernel> {
    let mat = MatFile::load(path)?;
    let data: Vec<Vec<f64>> = mat.as_f64_rows().iter().map(|r| r.to_vec()).collect();
    Ok(Kernel {
        data,
        scale: mat.scale(),
    })
}

/// Write a convolution [`Kernel`] as a single-band float raster (`.v` carrier).
///
/// The coefficient grid is the matrix vips's `gaussmat` / `logmat` emit; the
/// header `scale` is separate mask metadata that the raster differential does
/// not compare. Matrix dimensions are guarded (`u32` conversion) so a
/// pathological mask cannot abort.
fn save_matrix(kernel: &Kernel, out: &Path) -> Result<()> {
    let height = kernel.data.len();
    let width = kernel.data.first().map_or(0, |r| r.len());
    if width == 0 || height == 0 {
        bail!("mask generator produced an empty matrix");
    }
    let w = u32::try_from(width).map_err(|_| anyhow!("mask width {width} out of range"))?;
    let h = u32::try_from(height).map_err(|_| anyhow!("mask height {height} out of range"))?;
    let samples: Vec<f32> = kernel.data.iter().flatten().map(|&v| v as f32).collect();
    let fmt = PixelFormat::with_channels(1, 4)
        .ok_or_else(|| anyhow!("no single-band float format available for the mask"))?;
    let raster = Raster::from_f32_samples(w, h, fmt, &samples)?;
    io::save(&raster, out)
}

/// `gaussmat OUT SIGMA MIN-AMPL [--separable] [--precision]` — S5 creator.
fn run_gaussmat(m: &ArgMatches) -> Result<()> {
    let out = PathBuf::from(pos(m, "OUT"));
    let sigma = *m.get_one::<f64>("SIGMA").expect("required");
    let min_ampl = *m.get_one::<f64>("MIN-AMPL").expect("required");
    let separable = m.get_flag("separable");
    let precision = precision(m)?;

    // @doc-snippet:begin command=gaussmat slot=apply
    // @doc-test: convolution.rs::gaussmat_integer_exact_matrix:1329 repo=libviprs
    let kernel = Kernel::try_gaussmat(sigma, min_ampl, separable, precision)?;
    // @doc-snippet:end command=gaussmat slot=apply

    // @doc-snippet:begin command=gaussmat slot=save imports=save_file
    save_matrix(&kernel, &out)?;
    // @doc-snippet:end command=gaussmat slot=save
    Ok(())
}

/// `logmat OUT SIGMA MIN-AMPL [--separable] [--precision]` — S5 creator.
fn run_logmat(m: &ArgMatches) -> Result<()> {
    let out = PathBuf::from(pos(m, "OUT"));
    let sigma = *m.get_one::<f64>("SIGMA").expect("required");
    let min_ampl = *m.get_one::<f64>("MIN-AMPL").expect("required");
    let separable = m.get_flag("separable");
    let precision = precision(m)?;

    // @doc-snippet:begin command=logmat slot=apply
    // @doc-test: convolution.rs::logmat_float_separable:1404 repo=libviprs
    let kernel = Kernel::try_logmat(sigma, min_ampl, separable, precision)?;
    // @doc-snippet:end command=logmat slot=apply

    // @doc-snippet:begin command=logmat slot=save imports=save_file
    save_matrix(&kernel, &out)?;
    // @doc-snippet:end command=logmat slot=save
    Ok(())
}

/// `conv IN OUT MASK.mat [--precision]` — S1, matrix-file mask.
fn run_conv(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let mask_path = PathBuf::from(pos(m, "MASK"));
    let precision = precision(m)?;

    // @doc-snippet:begin command=conv slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=conv slot=load

    let kernel = load_kernel(&mask_path)?;

    // @doc-snippet:begin command=conv slot=apply
    // @doc-test: convolution.rs::conv_box_blur_hand_values:1459 repo=libviprs
    let out = raster.try_conv(&kernel, precision)?;
    // @doc-snippet:end command=conv slot=apply

    // @doc-snippet:begin command=conv slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=conv slot=save
    Ok(())
}

/// `convsep IN OUT MASK.mat [--precision]` — S1, separable matrix-file mask.
fn run_convsep(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let mask_path = PathBuf::from(pos(m, "MASK"));
    let precision = precision(m)?;

    // @doc-snippet:begin command=convsep slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=convsep slot=load

    let kernel = load_kernel(&mask_path)?;

    // @doc-snippet:begin command=convsep slot=apply
    // @doc-test: convolution.rs::convsep_vertical_mask_matches_manual_passes:1582 repo=libviprs
    let out = raster.try_convsep(&kernel, precision)?;
    // @doc-snippet:end command=convsep slot=apply

    // @doc-snippet:begin command=convsep slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=convsep slot=save
    Ok(())
}

/// `compass IN OUT MASK.mat [--times] [--angle] [--combine] [--precision]` — S1.
fn run_compass(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let mask_path = PathBuf::from(pos(m, "MASK"));
    let times = *m.get_one::<u32>("times").expect("clap default 2");
    let angle = angle45(pos(m, "angle"))?;
    let combine = combine(pos(m, "combine"))?;
    let precision = precision(m)?;

    // @doc-snippet:begin command=compass slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=compass slot=load

    let kernel = load_kernel(&mask_path)?;

    // @doc-snippet:begin command=compass slot=apply
    // @doc-test: convolution.rs::compass_matches_manual_combination:1643 repo=libviprs
    let out = raster.try_compass(&kernel, times, angle, combine, precision)?;
    // @doc-snippet:end command=compass slot=apply

    // @doc-snippet:begin command=compass slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=compass slot=save
    Ok(())
}

/// `gaussblur IN OUT SIGMA [--min-ampl] [--precision]` — S1.
fn run_gaussblur(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let sigma = *m.get_one::<f64>("SIGMA").expect("required");
    let min_ampl = *m.get_one::<f64>("min-ampl").expect("clap default 0.2");
    let precision = precision(m)?;

    // @doc-snippet:begin command=gaussblur slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=gaussblur slot=load

    // @doc-snippet:begin command=gaussblur slot=apply
    // @doc-test: convolution.rs::gaussblur_is_separable_gaussian:1697 repo=libviprs
    let out = raster.try_gaussblur(sigma, min_ampl, precision)?;
    // @doc-snippet:end command=gaussblur slot=apply

    // @doc-snippet:begin command=gaussblur slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=gaussblur slot=save
    Ok(())
}

/// `sharpen IN OUT [--sigma] [--m1] [--m2]` — S1, LabS unsharp mask.
fn run_sharpen(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let sigma = *m.get_one::<f64>("sigma").expect("clap default 0.5");
    let m1 = *m.get_one::<f64>("m1").expect("clap default 0");
    let m2 = *m.get_one::<f64>("m2").expect("clap default 3");

    // @doc-snippet:begin command=sharpen slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=sharpen slot=load

    // @doc-snippet:begin command=sharpen slot=apply
    // @doc-test: convolution.rs::sharpen_sharpens_luminance_only:1788 repo=libviprs
    let out = raster.try_sharpen(sigma, m1, m2)?;
    // @doc-snippet:end command=sharpen slot=apply

    // @doc-snippet:begin command=sharpen slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=sharpen slot=save
    Ok(())
}

/// `spcor IN REF OUT` — S2, normalised cross-correlation → float `.v`.
fn run_spcor(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let ref_path = PathBuf::from(pos(m, "REF"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=spcor slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let template = io::load(&ref_path, &limits)?;
    // @doc-snippet:end command=spcor slot=load

    // @doc-snippet:begin command=spcor slot=apply
    // @doc-test: convolution.rs::correlation_peaks_at_match:1725 repo=libviprs
    let out = raster.try_spcor(&template)?;
    // @doc-snippet:end command=spcor slot=apply

    // @doc-snippet:begin command=spcor slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=spcor slot=save
    Ok(())
}

/// `fastcor IN REF OUT` — S2, sum-of-squared-differences → float `.v`.
fn run_fastcor(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let ref_path = PathBuf::from(pos(m, "REF"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=fastcor slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let template = io::load(&ref_path, &limits)?;
    // @doc-snippet:end command=fastcor slot=load

    // @doc-snippet:begin command=fastcor slot=apply
    // @doc-test: convolution.rs::fastcor_hand_values:1713 repo=libviprs
    let out = raster.try_fastcor(&template)?;
    // @doc-snippet:end command=fastcor slot=apply

    // @doc-snippet:begin command=fastcor slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=fastcor slot=save
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
        assert_eq!(
            meta_names.len(),
            9,
            "the convolution family has nine commands"
        );
    }

    #[test]
    fn gaussmat_parses_positionals_and_flags() {
        let m = cmd("gaussmat")
            .try_get_matches_from([
                "gaussmat",
                "out.v",
                "1.5",
                "0.1",
                "--separable",
                "--precision",
                "float",
            ])
            .unwrap();
        assert_eq!(pos(&m, "OUT"), "out.v");
        assert_eq!(*m.get_one::<f64>("SIGMA").unwrap(), 1.5);
        assert_eq!(*m.get_one::<f64>("MIN-AMPL").unwrap(), 0.1);
        assert!(m.get_flag("separable"));
        assert_eq!(precision(&m).unwrap(), Precision::Float);
    }

    #[test]
    fn gaussmat_precision_defaults_to_integer() {
        let m = cmd("gaussmat")
            .try_get_matches_from(["gaussmat", "out.v", "1", "0.1"])
            .unwrap();
        assert!(!m.get_flag("separable"));
        assert_eq!(precision(&m).unwrap(), Precision::Integer);
    }

    #[test]
    fn gaussmat_rejects_approximate_precision() {
        // vips offers integer|float|approximate; the core has no approximate, so
        // the CLI must reject that spelling at parse time (no invented parity).
        assert!(
            cmd("gaussmat")
                .try_get_matches_from([
                    "gaussmat",
                    "out.v",
                    "1",
                    "0.1",
                    "--precision",
                    "approximate"
                ])
                .is_err()
        );
    }

    #[test]
    fn conv_parses_mask_and_precision_default_float() {
        let m = cmd("conv")
            .try_get_matches_from(["conv", "in.png", "out.png", "blur.mat"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "MASK"), "blur.mat");
        // vips conv default precision is float.
        assert_eq!(precision(&m).unwrap(), Precision::Float);
    }

    #[test]
    fn compass_parses_all_flags() {
        let m = cmd("compass")
            .try_get_matches_from([
                "compass",
                "in.png",
                "out.png",
                "sobel.mat",
                "--times",
                "4",
                "--angle",
                "d45",
                "--combine",
                "sum",
                "--precision",
                "integer",
            ])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("times").unwrap(), 4);
        assert_eq!(angle45(pos(&m, "angle")).unwrap(), Angle45::D45);
        assert_eq!(combine(pos(&m, "combine")).unwrap(), Combine::Sum);
        assert_eq!(precision(&m).unwrap(), Precision::Integer);
    }

    #[test]
    fn compass_defaults_match_vips() {
        let m = cmd("compass")
            .try_get_matches_from(["compass", "in.png", "out.png", "sobel.mat"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("times").unwrap(), 2);
        assert_eq!(angle45(pos(&m, "angle")).unwrap(), Angle45::D90);
        assert_eq!(combine(pos(&m, "combine")).unwrap(), Combine::Max);
        // vips compass default precision is float.
        assert_eq!(precision(&m).unwrap(), Precision::Float);
    }

    #[test]
    fn compass_rejects_combine_min() {
        // vips offers max|sum|min; the core has no Min, so the CLI must reject it.
        assert!(
            cmd("compass")
                .try_get_matches_from(["compass", "in.png", "out.png", "m.mat", "--combine", "min"])
                .is_err()
        );
    }

    #[test]
    fn compass_rejects_out_of_range_times() {
        // vips bounds times at 1..=1000.
        assert!(
            cmd("compass")
                .try_get_matches_from(["compass", "in.png", "out.png", "m.mat", "--times", "0"])
                .is_err(),
            "times below 1 must be rejected"
        );
        assert!(
            cmd("compass")
                .try_get_matches_from(["compass", "in.png", "out.png", "m.mat", "--times", "1001"])
                .is_err(),
            "times above 1000 must be rejected"
        );
    }

    #[test]
    fn compass_rejects_bad_angle() {
        assert!(
            cmd("compass")
                .try_get_matches_from(["compass", "in.png", "out.png", "m.mat", "--angle", "d30"])
                .is_err()
        );
    }

    #[test]
    fn gaussblur_parses_sigma_and_min_ampl() {
        let m = cmd("gaussblur")
            .try_get_matches_from(["gaussblur", "in.png", "out.png", "1.5", "--min-ampl", "0.1"])
            .unwrap();
        assert_eq!(*m.get_one::<f64>("SIGMA").unwrap(), 1.5);
        assert_eq!(*m.get_one::<f64>("min-ampl").unwrap(), 0.1);
        // vips gaussblur default precision is integer.
        assert_eq!(precision(&m).unwrap(), Precision::Integer);
    }

    #[test]
    fn sharpen_defaults_match_vips() {
        let m = cmd("sharpen")
            .try_get_matches_from(["sharpen", "in.png", "out.png"])
            .unwrap();
        assert_eq!(*m.get_one::<f64>("sigma").unwrap(), 0.5);
        assert_eq!(*m.get_one::<f64>("m1").unwrap(), 0.0);
        assert_eq!(*m.get_one::<f64>("m2").unwrap(), 3.0);
    }

    #[test]
    fn spcor_and_fastcor_take_two_inputs_then_out() {
        let m = cmd("spcor")
            .try_get_matches_from(["spcor", "in.png", "ref.png", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "REF"), "ref.png");
        assert_eq!(pos(&m, "OUT"), "out.v");

        let m = cmd("fastcor")
            .try_get_matches_from(["fastcor", "in.png", "ref.png", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "REF"), "ref.png");
    }

    #[test]
    fn save_matrix_builds_a_single_band_float_raster() {
        // The integer sigma-1 gaussmat is the exact 5x5 libvips matrix.
        let k = Kernel::try_gaussmat(1.0, 0.1, false, Precision::Integer).unwrap();
        let dir = std::env::temp_dir();
        let out = dir.join(format!("viprs_conv_gm_{}.v", std::process::id()));
        save_matrix(&k, &out).expect("gaussmat matrix must save to .v");
        let decoded = libviprs::decode_file(&out).expect("decode the .v matrix");
        assert_eq!((decoded.width(), decoded.height()), (5, 5));
        assert_eq!(decoded.format().channels(), 1);
        assert!(decoded.format().is_float());
        // Centre coefficient is 20 (integer gaussmat, sigma 1).
        assert_eq!(decoded.getpoint(2, 2), vec![20.0]);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn angle_and_combine_maps_cover_every_spelling() {
        for (s, a) in [
            ("d0", Angle45::D0),
            ("d45", Angle45::D45),
            ("d90", Angle45::D90),
            ("d135", Angle45::D135),
            ("d180", Angle45::D180),
            ("d225", Angle45::D225),
            ("d270", Angle45::D270),
            ("d315", Angle45::D315),
        ] {
            assert_eq!(angle45(s).unwrap(), a);
        }
        assert_eq!(combine("max").unwrap(), Combine::Max);
        assert_eq!(combine("sum").unwrap(), Combine::Sum);
    }
}
