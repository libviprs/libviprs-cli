//! Arithmetic family — **part B** (Wave-2 arith-b lane, `OP_MAP.md` arithmetic
//! section, `CLI_CONTRACT.md` §3/§5/§6).
//!
//! This lane fills the disjoint second half of the arithmetic family (part A
//! owns statistics / const-linear / unary-rounding / hough). It contributes 20
//! `viprs` subcommands whose names, positional order, flag names, enum
//! spellings, and input bounds mirror vips 8.18.4 exactly (verified against
//! `vips <op>` usage). Every handler keeps the §3 `load → try_op → save` shape
//! and calls only the panic-free core APIs, so a bad input becomes exit 1
//! rather than a process abort (`CLI_CONTRACT.md` §8):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `subtract L R OUT`            | `subtract`        | S2 | EAC | core saturates the diff at 0 (unsigned out); the differential adds an `a>=b` case so the full range is exercised without a clip dead-zone |
//! | `multiply L R OUT`            | `multiply`        | S2 | EAC | int promote (uchar→ushort), `.v` |
//! | `divide L R OUT`             | `divide`          | S2 | EAC | float quotient, `.v` |
//! | `minpair L R OUT`           | `minpair`         | S2 | EXACT | format-preserving |
//! | `maxpair L R OUT`           | `maxpair`         | S2 | EXACT | format-preserving |
//! | `sum A B [C…] OUT`          | `sum`             | S2 variadic | EAC | image array → ushort, `.v` |
//! | `relational L R OUT OP`     | `relational`      | S2 | EXACT | enum equal\|noteq\|less\|lesseq\|more\|moreeq → 0/255 |
//! | `relational_const IN OUT OP C`| `relational_const`| S1 | EXACT | scalar const (per-band vector is core-limited) |
//! | `boolean L R OUT OP`       | `boolean`         | S2 | EXACT | enum and\|or\|eor (lshift/rshift are core-only-const, excluded) |
//! | `boolean_const IN OUT OP C` | `boolean_const`   | S1 | EXACT | enum and\|or\|eor\|lshift\|rshift, scalar const |
//! | `scale IN OUT [--log]`      | `scale`           | S1 | BOUNDED-TOL | ≤1 LSB; vips `--exp` is fixed at core 0.25, not exposed |
//! | `stdif IN OUT W H`         | `stdif`           | S1 | BOUNDED-TOL | vips `--a --m0 --b --s0` fixed at core defaults |
//! | `recomb IN OUT M.mat`      | `recomb`          | S1 | EAC | matrix FILE arg (shared matfile loader) |
//! | `premultiply IN OUT`       | `premultiply`     | S1 | BOUNDED-TOL | ≤1 LSB post-cast |
//! | `unpremultiply IN OUT`     | `unpremultiply`   | S1 | BOUNDED-TOL | ≤1 LSB post-cast |
//! | `math IN OUT OP`           | `math`            | S1 | FOURIER | 16-way enum, float→`.v` (see the oracle-class note below) |
//! | `math2 L R OUT OP`        | `math2`           | S2 | FOURIER | enum atan2\|pow\|wop, float→`.v` (see the oracle-class note below) |
//! | `complexform L R OUT`      | `complexform`     | S2 | FOURIER | two reals → complex band-pairs, `.v` |
//! | `complex IN OUT OP`       | `complex`         | S1 | FOURIER | enum polar\|rect\|conj, `.v` |
//! | `complexget IN OUT OP`    | `complexget`      | S1 | FOURIER | enum real\|imag, `.v` |
//!
//! ## `math` / `math2` oracle class — `FOURIER`, not the OP_MAP `EAC` prediction
//!
//! OP_MAP.md provisionally predicts `EXACT-AFTER-CAST` for `math`/`math2`, but
//! the differential deliberately validates them via the stricter **float
//! carrier** (`.v`, compared at a small float eps) — the same FOURIER treatment
//! the complex family uses, not the uchar save-cast EAC path. An EAC-uchar save
//! of, say, `sin` output in `[-1, 1]` would round nearly everything to `0`/`1`
//! and be near-vacuous, so the float carrier is the honest, more discriminating
//! choice. [`metas`] therefore declares both as [`OracleClass::Fourier`] so
//! `__dump-commands` and any §6 contract audit report the class actually used;
//! this is a documented deviation from OP_MAP's prediction (cf. `recomb` /
//! `gamma`, which similarly diverge from their EAC predictions).
//!
//! ## Float-input safety (`CLI_CONTRACT.md` §8)
//!
//! Several core methods are integer-only and would **panic** (abort, exit 101)
//! on a float raster (`depth_max` / `unary_map`'s `!is_float` assert /
//! `read_u32`): `boolean`, `boolean_const`, `scale`, `stdif`, `recomb`,
//! `premultiply`, `unpremultiply`. Their handlers reject a float input up front
//! via [`reject_float`] with a typed exit-1 error rather than reaching the
//! panic. The remaining ops are float-safe: `subtract` / `divide` /
//! `multiply` / `minpair` / `maxpair` / `sum` and the relational family accept
//! float (returning a typed error where they cannot), and the transcendental /
//! complex ops (`math` / `math2` / `complexform` / `complex` / `complexget`)
//! take float by design.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::{ArithmeticError, Raster};

use super::super::matfile::MatFile;
use super::super::{CommandMeta, OracleClass, Shape, io};

/// The relational enum spellings, verified against `vips relational` 8.18.4.
const RELATIONAL_OPS: [&str; 6] = ["equal", "noteq", "less", "lesseq", "more", "moreeq"];
/// The `math` enum spellings, verified against `vips math` 8.18.4.
const MATH_OPS: [&str; 16] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "log", "log10", "exp", "exp10", "sinh", "cosh",
    "tanh", "asinh", "acosh", "atanh",
];

/// Static command metadata for the family part (name → shape + oracle class),
/// used by `__dump-commands` and the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    use OracleClass::{BoundedTol, Exact, ExactAfterCast, Fourier};
    use Shape::{ImageToImage, NImageToImage};
    vec![
        CommandMeta {
            name: "subtract",
            shape: NImageToImage,
            oracle_class: ExactAfterCast,
        },
        CommandMeta {
            name: "multiply",
            shape: NImageToImage,
            oracle_class: ExactAfterCast,
        },
        CommandMeta {
            name: "divide",
            shape: NImageToImage,
            oracle_class: ExactAfterCast,
        },
        CommandMeta {
            name: "minpair",
            shape: NImageToImage,
            oracle_class: Exact,
        },
        CommandMeta {
            name: "maxpair",
            shape: NImageToImage,
            oracle_class: Exact,
        },
        CommandMeta {
            name: "sum",
            shape: NImageToImage,
            oracle_class: ExactAfterCast,
        },
        CommandMeta {
            name: "relational",
            shape: NImageToImage,
            oracle_class: Exact,
        },
        CommandMeta {
            name: "relational_const",
            shape: ImageToImage,
            oracle_class: Exact,
        },
        CommandMeta {
            name: "boolean",
            shape: NImageToImage,
            oracle_class: Exact,
        },
        CommandMeta {
            name: "boolean_const",
            shape: ImageToImage,
            oracle_class: Exact,
        },
        CommandMeta {
            name: "scale",
            shape: ImageToImage,
            oracle_class: BoundedTol,
        },
        CommandMeta {
            name: "stdif",
            shape: ImageToImage,
            oracle_class: BoundedTol,
        },
        CommandMeta {
            name: "recomb",
            shape: ImageToImage,
            oracle_class: ExactAfterCast,
        },
        CommandMeta {
            name: "premultiply",
            shape: ImageToImage,
            oracle_class: BoundedTol,
        },
        CommandMeta {
            name: "unpremultiply",
            shape: ImageToImage,
            oracle_class: BoundedTol,
        },
        CommandMeta {
            // FOURIER (float carrier), not OP_MAP's EAC prediction: the
            // differential compares native float `.v` at a float eps, the
            // stricter class actually used (see the module-header note).
            name: "math",
            shape: ImageToImage,
            oracle_class: Fourier,
        },
        CommandMeta {
            // FOURIER (float carrier); see `math` above and the module header.
            name: "math2",
            shape: NImageToImage,
            oracle_class: Fourier,
        },
        CommandMeta {
            name: "complexform",
            shape: NImageToImage,
            oracle_class: Fourier,
        },
        CommandMeta {
            name: "complex",
            shape: ImageToImage,
            oracle_class: Fourier,
        },
        CommandMeta {
            name: "complexget",
            shape: ImageToImage,
            oracle_class: Fourier,
        },
    ]
}

/// A binary S2 image→OUT command (`<op> LEFT RIGHT OUT`). vips names the two
/// inputs `left`/`right`; the trailing positional is the output.
fn binary_cmd(name: &'static str, about: &'static str) -> Command {
    io::with_decode_limit_args(
        Command::new(name)
            .about(about)
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
    )
}

/// An S1 image→image command with no extra args beyond IN/OUT.
fn unary_cmd(name: &'static str, about: &'static str) -> Command {
    io::with_decode_limit_args(
        Command::new(name)
            .about(about)
            .arg(Arg::new("IN").required(true).help("Input image"))
            .arg(Arg::new("OUT").required(true).help("Output image")),
    )
}

/// The clap commands this family part contributes.
pub fn commands() -> Vec<Command> {
    vec![
        binary_cmd(
            "subtract",
            "Subtract the right image from the left (left - right).",
        ),
        binary_cmd("multiply", "Multiply two images samplewise."),
        binary_cmd(
            "divide",
            "Divide the left image by the right (left / right).",
        ),
        binary_cmd("minpair", "Samplewise minimum of a pair of images."),
        binary_cmd("maxpair", "Samplewise maximum of a pair of images."),
        io::with_decode_limit_args(
            Command::new("sum")
                .about("Sum an array of two or more images into one image.")
                .arg(
                    Arg::new("INPUTS")
                        .required(true)
                        .num_args(2..)
                        .value_name("A B [C…] OUT")
                        .help("Two or more input images followed by the output path"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("relational")
                .about("Samplewise relational comparison of two images (0/255 mask out).")
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
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("equal|noteq|less|lesseq|more|moreeq")
                        .value_parser(RELATIONAL_OPS)
                        .help("Relational operator to apply (left OP right)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("relational_const")
                .about("Samplewise relational comparison against a constant (0/255 mask out).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("equal|noteq|less|lesseq|more|moreeq")
                        .value_parser(RELATIONAL_OPS)
                        .help("Relational operator to apply (in OP c)"),
                )
                .arg(
                    Arg::new("C")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help(
                            "Constant compared against every sample. NOTE: vips takes a \
                             per-band constant vector; core applies ONE constant to all \
                             bands, so a single scalar is accepted here.",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("boolean")
                .about("Samplewise bitwise boolean operation on two images.")
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
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("and|or|eor")
                        // vips `boolean` also lists lshift|rshift, but those are
                        // 2-image shifts the core exposes only as
                        // `boolean_const` (const shift). They are intentionally
                        // EXCLUDED from the 2-image command.
                        .value_parser(["and", "or", "eor"])
                        .help(
                            "Boolean operator (and|or|eor). vips's lshift|rshift are \
                             2-image in vips but core-only as boolean_const constants, \
                             so they are NOT accepted here — use boolean_const.",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("boolean_const")
                .about("Samplewise bitwise boolean operation against a constant.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("and|or|eor|lshift|rshift")
                        .value_parser(["and", "or", "eor", "lshift", "rshift"])
                        .help("Boolean operator to apply against the constant"),
                )
                .arg(
                    Arg::new("C")
                        .required(true)
                        .value_parser(value_parser!(i64))
                        .help(
                            "Integer constant (bit-mask for and|or|eor, shift count \
                             for lshift|rshift; a shift must be >= 0). NOTE: vips takes \
                             a per-band constant vector; core applies ONE constant to \
                             all bands, so a single scalar is accepted here.",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("scale")
                .about("Scale an image linearly (or with a log curve) to fill 0..=255 uchar.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("log")
                        .long("log")
                        .action(clap::ArgAction::SetTrue)
                        // vips also exposes `--exp` (log-curve exponent, default
                        // 0.25); the core fixes that exponent at 0.25 and offers
                        // no knob, so `--exp` is NOT exposed here (red-flag).
                        .help(
                            "Use the log-scaling curve instead of a linear stretch. \
                             (vips's --exp log-curve exponent is fixed at 0.25 in core \
                             and not configurable.)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("stdif")
                .about("Statistical differencing over a sliding window.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("WIDTH")
                        .required(true)
                        // vips bounds: min 1, max 256.
                        .value_parser(value_parser!(u32).range(1..=256))
                        .help("Window width in pixels (1..=256)"),
                )
                .arg(
                    Arg::new("HEIGHT")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..=256))
                        .help(
                            "Window height in pixels (1..=256). (vips's --a --m0 --b \
                             --s0 tuning knobs are fixed at core defaults 0.5/128/0.5/50.)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("recomb")
                .about("Recombine bands with a coefficient matrix (one output band per row).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(Arg::new("M").required(true).value_name("M.mat").help(
                    "Coefficient matrix as a vips text matrix file (one row per \
                             output band, one column per input band)",
                )),
        ),
        unary_cmd(
            "premultiply",
            "Premultiply the colour bands by the alpha band.",
        ),
        unary_cmd(
            "unpremultiply",
            "Unpremultiply (divide) the colour bands by the alpha band.",
        ),
        io::with_decode_limit_args(
            Command::new("math")
                .about("Apply a unary math (trig / log / exp) operation; float output.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output image (float; use .v)"),
                )
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("sin|cos|tan|asin|…|atanh")
                        .value_parser(MATH_OPS)
                        .help(
                            "Math operation (trig args/results are in DEGREES; asin/acos \
                             need inputs in [-1,1], log/exp domains apply)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("math2")
                .about("Apply a binary math (atan2 / pow / wop) operation; float output.")
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
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output image (float; use .v)"),
                )
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("atan2|pow|wop")
                        .value_parser(["atan2", "pow", "wop"])
                        .help("Binary math operation (pow = left^right, wop = right^left)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("complexform")
                .about("Form a complex image from two real images (left=real, right=imag).")
                .arg(
                    Arg::new("LEFT")
                        .required(true)
                        .help("Real-part input image"),
                )
                .arg(
                    Arg::new("RIGHT")
                        .required(true)
                        .help("Imaginary-part input image"),
                )
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output complex image as (re,im) band pairs (use .v)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("complex")
                .about("Apply a complex operation to a complex (re,im band-pair) image.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input complex image ((re,im) band pairs, use .v)"),
                )
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output complex image (use .v)"),
                )
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("polar|rect|conj")
                        .value_parser(["polar", "rect", "conj"])
                        .help("Complex operation (polar/rect angles in DEGREES)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("complexget")
                .about("Extract the real or imaginary component from a complex image.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input complex image ((re,im) band pairs, use .v)"),
                )
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output real image (float; use .v)"),
                )
                .arg(
                    Arg::new("OP")
                        .required(true)
                        .value_name("real|imag")
                        .value_parser(["real", "imag"])
                        .help("Component to extract"),
                ),
        ),
    ]
}

/// Dispatch a matched arithmetic part-B subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "subtract" => run_binary(m, Raster::try_sub),
        "multiply" => run_binary(m, Raster::try_mul),
        "divide" => run_binary(m, Raster::try_div),
        "minpair" => run_binary(m, Raster::try_minpair),
        "maxpair" => run_binary(m, Raster::try_maxpair),
        "sum" => run_sum(m),
        "relational" => run_relational(m),
        "relational_const" => run_relational_const(m),
        "boolean" => run_boolean(m),
        "boolean_const" => run_boolean_const(m),
        "scale" => run_scale(m),
        "stdif" => run_stdif(m),
        "recomb" => run_recomb(m),
        "premultiply" => run_premultiply(m),
        "unpremultiply" => run_unpremultiply(m),
        "math" => run_math(m),
        "math2" => run_math2(m),
        "complexform" => run_complexform(m),
        "complex" => run_complex(m),
        "complexget" => run_complexget(m),
        other => bail!("arithmetic part_b has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Reject a float raster for an integer-only core op, converting what would be
/// a core panic (via `depth_max` / `read_u32` / `unary_map`'s `!is_float`
/// assert) into a typed exit-1 error (`CLI_CONTRACT.md` §8).
fn reject_float(op: &str, raster: &Raster) -> Result<()> {
    if raster.format().is_float() {
        bail!(
            "{op}: this operation is integer-only and does not accept a float \
             input raster; cast to an unsigned 8/16-bit format first"
        );
    }
    Ok(())
}

/// `<op> LEFT RIGHT OUT` — S2 binary image→OUT via a fallible core method.
fn run_binary(
    m: &ArgMatches,
    apply: fn(&Raster, &Raster) -> std::result::Result<Raster, ArithmeticError>,
) -> Result<()> {
    let limits = io::decode_limits(m);
    let left = io::load(&PathBuf::from(pos(m, "LEFT")), &limits)?;
    let right = io::load(&PathBuf::from(pos(m, "RIGHT")), &limits)?;
    let out = apply(&left, &right)?;
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `sum A B [C…] OUT` — S2 variadic image-array sum.
fn run_sum(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let (inputs, out_path) = io::inputs_and_out(m, "INPUTS")?;
    let rasters: Vec<Raster> = inputs
        .iter()
        .map(|p| io::load(p, &limits))
        .collect::<Result<_>>()?;
    let refs: Vec<&Raster> = rasters.iter().collect();
    let out = Raster::try_sum(&refs)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `relational LEFT RIGHT OUT OP` — S2 enum → 0/255 mask.
fn run_relational(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let left = io::load(&PathBuf::from(pos(m, "LEFT")), &limits)?;
    let right = io::load(&PathBuf::from(pos(m, "RIGHT")), &limits)?;
    let out = match pos(m, "OP") {
        "equal" => left.try_equal(&right)?,
        "noteq" => left.try_noteq(&right)?,
        "less" => left.try_less_than(&right)?,
        "lesseq" => left.try_less_eq(&right)?,
        "more" => left.try_more_than(&right)?,
        "moreeq" => left.try_more_eq(&right)?,
        other => bail!("unknown relational operator {other:?}"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `relational_const IN OUT OP C` — S1 enum + scalar const → 0/255 mask.
fn run_relational_const(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    let c = *m.get_one::<f64>("C").expect("required positional");
    // The const relational ops are infallible in the core (they never error).
    let out = match pos(m, "OP") {
        "equal" => raster.equal_const(c),
        "noteq" => raster.noteq_const(c),
        "less" => raster.less_than_const(c),
        "lesseq" => raster.less_eq_const(c),
        "more" => raster.more_than_const(c),
        "moreeq" => raster.more_eq_const(c),
        other => bail!("unknown relational operator {other:?}"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `boolean LEFT RIGHT OUT OP` — S2 enum and|or|eor (integer-only).
fn run_boolean(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let left = io::load(&PathBuf::from(pos(m, "LEFT")), &limits)?;
    let right = io::load(&PathBuf::from(pos(m, "RIGHT")), &limits)?;
    reject_float("boolean", &left)?;
    reject_float("boolean", &right)?;
    let out = match pos(m, "OP") {
        "and" => left.try_bitand(&right)?,
        "or" => left.try_bitor(&right)?,
        "eor" => left.try_bitxor(&right)?,
        other => bail!("unknown boolean operator {other:?} (expected and|or|eor)"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `boolean_const IN OUT OP C` — S1 enum and|or|eor|lshift|rshift + int const.
fn run_boolean_const(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    reject_float("boolean_const", &raster)?;
    let c = *m.get_one::<i64>("C").expect("required positional");
    // and|or|eor take an i64 mask; lshift|rshift take a u32 shift count. A
    // negative or too-large shift is a typed exit-1 error, NEVER a panic
    // (bands B2 lesson): u32::try_from surfaces the range fault.
    let out = match pos(m, "OP") {
        "and" => raster.bitand_const(c),
        "or" => raster.bitor_const(c),
        "eor" => raster.bitxor_const(c),
        "lshift" => raster.lshift(shift_count(c)?),
        "rshift" => raster.rshift(shift_count(c)?),
        other => bail!("unknown boolean operator {other:?}"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// Convert a shift constant to the core's `u32` count, rejecting a negative or
/// out-of-range value with a typed error instead of a panic (`CLI_CONTRACT.md`
/// §8).
fn shift_count(c: i64) -> Result<u32> {
    u32::try_from(c)
        .map_err(|_| anyhow!("shift count {c} is out of range (must be 0..={})", u32::MAX))
}

/// `scale IN OUT [--log]` — S1 linear/log stretch to uchar.
fn run_scale(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    reject_float("scale", &raster)?;
    let out = raster.scaleimage(Some(m.get_flag("log")));
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `stdif IN OUT WIDTH HEIGHT` — S1 statistical differencing.
fn run_stdif(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    reject_float("stdif", &raster)?;
    let width = *m.get_one::<u32>("WIDTH").expect("required positional");
    let height = *m.get_one::<u32>("HEIGHT").expect("required positional");
    let out = raster.try_stdif(width, height)?;
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `recomb IN OUT M.mat` — S1 band recombination with a matrix file.
fn run_recomb(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    reject_float("recomb", &raster)?;
    let matrix = MatFile::load(&PathBuf::from(pos(m, "M")))?;
    let rows = matrix.as_f64_rows();
    let out = raster.try_recomb(&rows)?;
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `premultiply IN OUT` — S1 alpha premultiply.
fn run_premultiply(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    reject_float("premultiply", &raster)?;
    let out = raster.try_premultiply()?;
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `unpremultiply IN OUT` — S1 alpha unpremultiply.
fn run_unpremultiply(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    reject_float("unpremultiply", &raster)?;
    let out = raster.try_unpremultiply()?;
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `math IN OUT OP` — S1 unary transcendental (float out). The core math
/// methods are infallible and accept every input depth including float.
fn run_math(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    let out = match pos(m, "OP") {
        "sin" => raster.sin(),
        "cos" => raster.cos(),
        "tan" => raster.tan(),
        "asin" => raster.asin(),
        "acos" => raster.acos(),
        "atan" => raster.atan(),
        "log" => raster.log(),
        "log10" => raster.log10(),
        "exp" => raster.exp(),
        "exp10" => raster.exp10(),
        "sinh" => raster.sinh(),
        "cosh" => raster.cosh(),
        "tanh" => raster.tanh(),
        "asinh" => raster.asinh(),
        "acosh" => raster.acosh(),
        "atanh" => raster.atanh(),
        other => bail!("unknown math operation {other:?}"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `math2 LEFT RIGHT OUT OP` — S2 binary transcendental (float out).
fn run_math2(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let left = io::load(&PathBuf::from(pos(m, "LEFT")), &limits)?;
    let right = io::load(&PathBuf::from(pos(m, "RIGHT")), &limits)?;
    let out = match pos(m, "OP") {
        "atan2" => left.try_atan2(&right)?,
        "pow" => left.try_pow(&right)?,
        "wop" => left.try_wop(&right)?,
        other => bail!("unknown math2 operation {other:?} (expected atan2|pow|wop)"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `complexform LEFT RIGHT OUT` — S2 two reals → complex (re,im) band pairs.
fn run_complexform(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let left = io::load(&PathBuf::from(pos(m, "LEFT")), &limits)?;
    let right = io::load(&PathBuf::from(pos(m, "RIGHT")), &limits)?;
    let out = Raster::try_complexform(&left, &right)?;
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `complex IN OUT OP` — S1 complex operation on (re,im) band pairs.
fn run_complex(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    let out = match pos(m, "OP") {
        "polar" => raster.try_polar()?,
        "rect" => raster.try_rect()?,
        "conj" => raster.try_conj()?,
        other => bail!("unknown complex operation {other:?} (expected polar|rect|conj)"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

/// `complexget IN OUT OP` — S1 extract real/imag component.
fn run_complexget(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let raster = io::load(&PathBuf::from(pos(m, "IN")), &limits)?;
    let out = match pos(m, "OP") {
        "real" => raster.try_real()?,
        "imag" => raster.try_imag()?,
        other => bail!("unknown complexget operation {other:?} (expected real|imag)"),
    };
    io::save(&out, &PathBuf::from(pos(m, "OUT")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use libviprs::PixelFormat;

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
        assert_eq!(cmd_names.len(), meta_names.len());
        for name in &meta_names {
            assert!(
                cmd_names.iter().any(|c| c == name),
                "meta {name} has no command"
            );
        }
        assert_eq!(meta_names.len(), 20, "arith-b contributes 20 commands");
    }

    #[test]
    fn subtract_parses_left_right_out_in_vips_order() {
        let m = cmd("subtract")
            .try_get_matches_from(["subtract", "l.png", "r.png", "o.png"])
            .unwrap();
        assert_eq!(pos(&m, "LEFT"), "l.png");
        assert_eq!(pos(&m, "RIGHT"), "r.png");
        assert_eq!(pos(&m, "OUT"), "o.png");
    }

    #[test]
    fn sum_splits_variadic_into_inputs_and_out() {
        let m = cmd("sum")
            .try_get_matches_from(["sum", "a.png", "b.png", "c.png", "out.v"])
            .unwrap();
        let (inputs, out) = io::inputs_and_out(&m, "INPUTS").unwrap();
        assert_eq!(inputs.len(), 3);
        assert_eq!(out, PathBuf::from("out.v"));
    }

    #[test]
    fn sum_requires_at_least_two_positionals() {
        assert!(
            cmd("sum")
                .try_get_matches_from(["sum", "only.png"])
                .is_err(),
            "num_args(2..) must reject a single positional"
        );
    }

    #[test]
    fn relational_enforces_the_enum() {
        assert!(
            cmd("relational")
                .clone()
                .try_get_matches_from(["relational", "l.png", "r.png", "o.png", "more"])
                .is_ok()
        );
        assert!(
            cmd("relational")
                .try_get_matches_from(["relational", "l.png", "r.png", "o.png", "gt"])
                .is_err(),
            "only the six vips relational spellings are accepted"
        );
    }

    #[test]
    fn boolean_rejects_lshift_and_rshift() {
        // The 2-image boolean command excludes lshift|rshift (core-only-const).
        assert!(
            cmd("boolean")
                .clone()
                .try_get_matches_from(["boolean", "l.png", "r.png", "o.png", "and"])
                .is_ok()
        );
        assert!(
            cmd("boolean")
                .try_get_matches_from(["boolean", "l.png", "r.png", "o.png", "lshift"])
                .is_err(),
            "lshift is not a 2-image core op and must be rejected"
        );
    }

    #[test]
    fn boolean_const_accepts_all_five_enums() {
        for op in ["and", "or", "eor", "lshift", "rshift"] {
            assert!(
                cmd("boolean_const")
                    .clone()
                    .try_get_matches_from(["boolean_const", "in.png", "out.png", op, "3"])
                    .is_ok(),
                "boolean_const must accept {op}"
            );
        }
    }

    #[test]
    fn boolean_const_negative_shift_is_error_not_panic() {
        // A negative shift parses (i64) but converting to the core's u32 count
        // must be a typed error, NEVER a panic/abort (CLI_CONTRACT.md §8).
        assert!(
            shift_count(-1).is_err(),
            "a negative shift must be rejected"
        );
        assert!(shift_count(0).is_ok());
        assert_eq!(shift_count(5).unwrap(), 5);
    }

    #[test]
    fn stdif_enforces_vips_window_bounds() {
        assert!(
            cmd("stdif")
                .clone()
                .try_get_matches_from(["stdif", "in.png", "out.png", "3", "3"])
                .is_ok()
        );
        // vips min is 1: a 0 window is rejected at parse time.
        assert!(
            cmd("stdif")
                .clone()
                .try_get_matches_from(["stdif", "in.png", "out.png", "0", "3"])
                .is_err(),
            "a zero window must be rejected (vips min 1)"
        );
        // vips max is 256: 257 is out of range.
        assert!(
            cmd("stdif")
                .try_get_matches_from(["stdif", "in.png", "out.png", "257", "3"])
                .is_err(),
            "a window above 256 must be rejected (vips max 256)"
        );
    }

    #[test]
    fn math_enforces_the_sixteen_enum() {
        assert_eq!(MATH_OPS.len(), 16);
        assert!(
            cmd("math")
                .clone()
                .try_get_matches_from(["math", "in.png", "out.v", "sin"])
                .is_ok()
        );
        assert!(
            cmd("math")
                .try_get_matches_from(["math", "in.png", "out.v", "sec"])
                .is_err(),
            "sec is not a vips math operation"
        );
    }

    #[test]
    fn math2_enforces_the_three_enum() {
        for op in ["atan2", "pow", "wop"] {
            assert!(
                cmd("math2")
                    .clone()
                    .try_get_matches_from(["math2", "l.v", "r.v", "o.v", op])
                    .is_ok()
            );
        }
        assert!(
            cmd("math2")
                .try_get_matches_from(["math2", "l.v", "r.v", "o.v", "hypot"])
                .is_err()
        );
    }

    #[test]
    fn complex_and_complexget_enforce_their_enums() {
        assert!(
            cmd("complex")
                .clone()
                .try_get_matches_from(["complex", "in.v", "out.v", "polar"])
                .is_ok()
        );
        assert!(
            cmd("complex")
                .try_get_matches_from(["complex", "in.v", "out.v", "real"])
                .is_err(),
            "real is a complexget op, not a complex op"
        );
        assert!(
            cmd("complexget")
                .clone()
                .try_get_matches_from(["complexget", "in.v", "out.v", "imag"])
                .is_ok()
        );
        assert!(
            cmd("complexget")
                .try_get_matches_from(["complexget", "in.v", "out.v", "polar"])
                .is_err()
        );
    }

    #[test]
    fn reject_float_guards_the_integer_only_ops() {
        // An in-memory float raster must be refused with a typed error, never a
        // panic (the guard that keeps `scale`/`boolean`/`stdif`/`recomb`/
        // `premultiply`/`unpremultiply` from aborting on a float `.v` input).
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let float_raster = Raster::from_f32_samples(2, 1, fmt, &[0.0, 1.0]).unwrap();
        assert!(reject_float("scale", &float_raster).is_err());

        // An integer raster passes the guard.
        let int_raster = Raster::zeroed(2, 2, PixelFormat::Gray8).unwrap();
        assert!(reject_float("scale", &int_raster).is_ok());
    }
}
