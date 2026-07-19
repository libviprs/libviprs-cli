//! Arithmetic family — **part A** (the arith-A lane, `OP_MAP.md` arithmetic
//! section, statistics / const-linear / unary-rounding / hough rows).
//!
//! This lane fills the eighteen `viprs` subcommands below; `part_b` owns the
//! binary/relational/boolean/math/complex rows and `mod.rs` (untouched here)
//! aggregates both parts and routes by `metas()` name. Positional orders, flag
//! names, enum spellings, and input value bounds mirror vips 8.18.4 exactly
//! (verified against `vips <op>` usage, 2026-07-19). Every handler keeps the §3
//! `load → try_op → save` shape, calls the panic-free core APIs, and turns a bad
//! user input into a typed exit-1 error rather than a process abort
//! (`CLI_CONTRACT.md` §8) — including the `clamp` `min > max` case, which the
//! core `clamp` would otherwise `assert!`-panic on.
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `avg IN`                         | `avg`             | S3 | EXACT | prints the mean (rational → S3 rel-eps) |
//! | `deviate IN`                     | `deviate`         | S3 | BOUNDED-TOL | sample sd, rel eps |
//! | `min IN [--x] [--y]`             | `min`             | S3 | EXACT | folds `minpos`: `--x`/`--y` print the position |
//! | `max IN [--x] [--y]`             | `max`             | S3 | EXACT | folds `maxpos` |
//! | `stats IN OUT`                   | `stats`           | S1 | BOUNDED-TOL | 6-col double matrix → `.v` (vips cols 6..10 are a core subset gap) |
//! | `measure IN OUT H V`             | `measure`         | S1 | BOUNDED-TOL | patch-means matrix → `.v` |
//! | `find_trim IN [--background]`    | `find_trim`       | S3 | EXACT | prints 4 ints (left top width height) |
//! | `profile IN COLS ROWS`          | `profile`         | S4 | EXACT | two 16-bit position images → `.v` |
//! | `project IN COLS ROWS`          | `project`         | S4 | EXACT | two 16-bit sum images → `.v` |
//! | `linear IN OUT "a" "b" [--uchar]`| `linear`          | S1 | EXACT-AFTER-CAST | scalar (broadcast) a·in+b; float out (`--uchar` = tol-0 uchar) |
//! | `remainder_const IN OUT "c"`     | `remainder_const` | S1 | EXACT | int remainder, scalar c |
//! | `math2_const IN OUT pow "c"`     | `math2_const`     | S1 | EXACT-AFTER-CAST | power, scalar exponent |
//! | `abs IN OUT`                     | `abs`             | S1 | EXACT | \|v\| (meaningful on float `.v`) |
//! | `sign IN OUT`                    | `sign`            | S1 | EXACT-AFTER-CAST | −1/0/1 (float in → signed out) |
//! | `clamp IN OUT [--min] [--max]`   | `clamp`           | S1 | EXACT | clip to [min,max]; NaN/inverted bounds → typed exit 1 |
//! | `round IN OUT rint\|ceil\|floor` | `round`           | S1 | EXACT (ceil/floor); GOLDEN-ONLY (rint) | rounding mode enum — rint's half-rule diverges from vips (see below) |
//! | `hough_line IN OUT`              | `hough_line`      | S1 | GOLDEN-ONLY | 256×256 accumulator; core binning diverges from vips (see below) |
//! | `hough_circle IN OUT MIN MAX`    | `hough_circle`    | S1 | GOLDEN-ONLY | scale-1 accumulator; core vote model diverges from vips. MIN/MAX are REQUIRED positionals (an intentional deviation — vips exposes them as optional `--min-radius`/`--max-radius`) |
//!
//! **`round rint` divergence (honest).** `OP_MAP.md` provisionally rated all three
//! `round` modes EXACT, but the differential (on an afloat input that actually
//! reaches the half-integer domain) measured a genuine, deterministic divergence
//! from vips 8.18.4 at exact half-integers: the core maps `f64::round` (round
//! half **away from zero**: 0.5→1, 2.5→3, −2.5→−3), while vips's C `rint` rounds
//! half **to even** (0.5→0, 2.5→2, −2.5→−2). `ceil`/`floor` have no tie-break and
//! stay EXACT. So `round rint` is carried GOLDEN-ONLY (a committed viprs-generated
//! regression pin, no vips parity oracle) and a core issue is filed to reconcile
//! `rint` with vips's round-half-to-even (and to correct the core doc comment at
//! `arithmetic.rs` that wrongly states vips `rint` "rounds halves away from zero").
//!
//! **`hough_circle` surface (honest).** vips 8.18.4 exposes the radii as OPTIONAL
//! flags (`--min-radius`/`--max-radius`, defaults 10/20), so `vips hough_circle in
//! out` is valid on its own; this CLI instead takes MIN_RADIUS/MAX_RADIUS as
//! REQUIRED positionals (no vips-default fallback). This is an intentional,
//! documented surface deviation — the op is GOLDEN-ONLY, so there is no
//! cross-oracle affected, and the core computes vips's `--scale 1` parameter
//! space (vips 8.18.4's own `--scale` default is 1, not 3).
//!
//! **Hough divergence (honest).** `OP_MAP.md` provisionally lists both Hough ops
//! EXACT, but the differential measured a genuine, non-tolerance divergence from
//! vips 8.18.4: `hough_line`'s distance binning is offset by one accumulator cell
//! (≤1 per vote, but N at a peak where N collinear pixels concentrate — a
//! horizontal line measured max-abs-diff 32), and `hough_circle`'s per-cell vote
//! model differs (a single point yields a core max of 1 vs a vips max of 4).
//! Neither is a bounded rounding tolerance, so both are carried GOLDEN-ONLY
//! (a committed viprs-generated regression pin, no vips parity oracle) and a core
//! issue is filed to reconcile the binning / vote model with vips.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::{PixelFormat, Raster};

use super::super::{CommandMeta, OracleClass, Shape, io};

/// Static per-command metadata (name → shape + oracle class) for this part.
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "avg",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "deviate",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "min",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "max",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "stats",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "measure",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "find_trim",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "profile",
            shape: Shape::TwoOutputs,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "project",
            shape: Shape::TwoOutputs,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "linear",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::ExactAfterCast,
        },
        CommandMeta {
            name: "remainder_const",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "math2_const",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::ExactAfterCast,
        },
        CommandMeta {
            name: "abs",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "sign",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::ExactAfterCast,
        },
        CommandMeta {
            name: "clamp",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // Mixed-mode (as `smartcrop`): `ceil`/`floor` are EXACT against vips,
            // but `rint` diverges at exact half-integers — the core uses
            // `f64::round` (half away from zero) while vips's C `rint` rounds half
            // to even, so `round rint` is GOLDEN-ONLY (viprs regression pin, core
            // issue filed). The dominant modes are EXACT; the per-mode oracle is
            // authoritative in the differential suite + OP_MAP + PROVENANCE.
            name: "round",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // NOT EXACT (OP_MAP.md provisionally listed EXACT): the core's
            // distance-binning normalization differs from vips 8.18.4 by a
            // one-cell shift, which is ≤1 per independent vote but amplifies to
            // N at a peak where N collinear pixels vote (a horizontal line
            // measured max-abs-diff 32, not a bounded tolerance). There is thus
            // no meaningful vips tolerance oracle; the reference is a
            // viprs-generated regression pin (GOLDEN-ONLY) and a core issue is
            // filed to reconcile the binning with vips. See the wave report.
            name: "hough_line",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            // NOT EXACT: the core's circle rasterization / accumulation differs
            // structurally from vips 8.18.4 (a single voting point yields a core
            // per-cell max of 1 but a vips max of 4 — a different vote model,
            // not a rounding tolerance). GOLDEN-ONLY viprs pin; core issue filed.
            name: "hough_circle",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::GoldenOnly,
        },
    ]
}

/// The clap commands this part contributes. Every command loads at least one
/// image, so each carries the shared decode-limit flags via
/// [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        // ---- statistics: image → stdout scalar(s) ----
        io::with_decode_limit_args(
            Command::new("avg")
                .about("Find the mean of every sample in an image and print it.")
                .arg(Arg::new("IN").required(true).help("Input image")),
        ),
        io::with_decode_limit_args(
            Command::new("deviate")
                .about("Find the sample standard deviation of an image and print it.")
                .arg(Arg::new("IN").required(true).help("Input image")),
        ),
        io::with_decode_limit_args(
            Command::new("min")
                .about(
                    "Find the minimum sample of an image and print it (with --x/--y, its position).",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("x")
                        .long("x")
                        .action(ArgAction::SetTrue)
                        .help("Also print the horizontal position of the first minimum"),
                )
                .arg(
                    Arg::new("y")
                        .long("y")
                        .action(ArgAction::SetTrue)
                        .help("Also print the vertical position of the first minimum"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("max")
                .about(
                    "Find the maximum sample of an image and print it (with --x/--y, its position).",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("x")
                        .long("x")
                        .action(ArgAction::SetTrue)
                        .help("Also print the horizontal position of the first maximum"),
                )
                .arg(
                    Arg::new("y")
                        .long("y")
                        .action(ArgAction::SetTrue)
                        .help("Also print the vertical position of the first maximum"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("stats")
                .about("Compute a per-band statistics matrix and write it to a .v image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output statistics matrix (.v; 6 columns min/max/sum/sum2/mean/sd)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("measure")
                .about(
                    "Measure the mean of each patch in an H×V grid and write the matrix to a .v \
                     image.",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output patch-means matrix (.v)"),
                )
                .arg(
                    Arg::new("H")
                        .required(true)
                        // vips's own minimum for h is 1.
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Number of patches across (>= 1)"),
                )
                .arg(
                    Arg::new("V")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Number of patches down (>= 1)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("find_trim")
                .about(
                    "Find the bounding box of non-background content and print \
                     left/top/width/height.",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("background")
                        .long("background")
                        .value_name("c…")
                        .help(
                            "Background colour as a space-separated vector (default 255 per band); \
                             vips's --threshold (default 10) and --line-art are not core-exposed",
                        ),
                ),
        ),
        // ---- statistics: image → two outputs ----
        io::with_decode_limit_args(
            Command::new("profile")
                .about("Find, per column and per row, the first non-zero sample position.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("COLS")
                        .required(true)
                        .help("Output columns image (.v)"),
                )
                .arg(Arg::new("ROWS").required(true).help("Output rows image (.v)")),
        ),
        io::with_decode_limit_args(
            Command::new("project")
                .about("Sum every column and every row; write the two projection images.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("COLS")
                        .required(true)
                        .help("Output column-sums image (.v)"),
                )
                .arg(
                    Arg::new("ROWS")
                        .required(true)
                        .help("Output row-sums image (.v)"),
                ),
        ),
        // ---- const / linear ----
        io::with_decode_limit_args(
            Command::new("linear")
                .about(
                    "Compute a·in + b (scalar, broadcast across bands); float out, or uchar with \
                     --uchar.",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("A")
                        .required(true)
                        .value_name("a…")
                        .help(
                            "Multiplier as a space-separated vector (a single scalar is broadcast)",
                        ),
                )
                .arg(
                    Arg::new("B")
                        .required(true)
                        .value_name("b…")
                        .help("Addend as a space-separated vector (a single scalar is broadcast)"),
                )
                .arg(
                    Arg::new("uchar")
                        .long("uchar")
                        .action(ArgAction::SetTrue)
                        .help("Clip and truncate the result into an 8-bit image (vips --uchar)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("remainder_const")
                .about("Remainder of every sample divided by a constant (format-preserving integer op).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("C")
                        .required(true)
                        .value_name("c")
                        .help(
                            "Divisor constant (a single scalar; per-band vectors are not \
                             core-backed)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("math2_const")
                .about("Binary math with a constant; only the power operation (pow) is core-backed.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("MATH2")
                        .required(true)
                        .value_name("pow")
                        // vips also offers wop/atan2; the core backs only pow as
                        // a const op, so those are rejected rather than faked.
                        .value_parser(["pow"])
                        .help("Math operation (only pow is core-backed)"),
                )
                .arg(
                    Arg::new("C")
                        .required(true)
                        .value_name("c")
                        .help(
                            "Exponent constant (a single scalar; per-band vectors are not \
                             core-backed)",
                        ),
                ),
        ),
        // ---- unary / rounding ----
        io::with_decode_limit_args(
            Command::new("abs")
                .about("Absolute value of every sample.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("sign")
                .about("Unit sign of every sample (−1 / 0 / 1; unsigned input yields 0/1).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("clamp")
                .about("Clip every sample into [min, max] (vips defaults 0 and 1).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("min")
                        .long("min")
                        .value_name("V")
                        .value_parser(value_parser!(f64))
                        .help("Lower bound (default 0)"),
                )
                .arg(
                    Arg::new("max")
                        .long("max")
                        .value_name("V")
                        .value_parser(value_parser!(f64))
                        .help("Upper bound (default 1)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("round")
                .about("Round every sample with the chosen rounding mode.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("ROUND")
                        .required(true)
                        .value_name("rint|ceil|floor")
                        .value_parser(["rint", "ceil", "floor"])
                        .help("Rounding operation"),
                ),
        ),
        // ---- hough ----
        io::with_decode_limit_args(
            Command::new("hough_line")
                .about("Hough line transform into the fixed 256×256 accumulator.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output accumulator image (.v)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("hough_circle")
                .about("Hough circle transform (scale 1), one band per radius in MIN..=MAX.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output accumulator image (.v)"),
                )
                .arg(
                    Arg::new("MIN_RADIUS")
                        .required(true)
                        // vips's own minimum radius is 1.
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Smallest radius to search for (>= 1)"),
                )
                .arg(
                    Arg::new("MAX_RADIUS")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Largest radius to search for (>= 1)"),
                ),
        ),
    ]
}

/// Dispatch a matched command to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "avg" => run_avg(m),
        "deviate" => run_deviate(m),
        "min" => run_extremum(m, false),
        "max" => run_extremum(m, true),
        "stats" => run_stats(m),
        "measure" => run_measure(m),
        "find_trim" => run_find_trim(m),
        "profile" => run_profile(m),
        "project" => run_project(m),
        "linear" => run_linear(m),
        "remainder_const" => run_remainder_const(m),
        "math2_const" => run_math2_const(m),
        "abs" => run_abs(m),
        "sign" => run_sign(m),
        "clamp" => run_clamp(m),
        "round" => run_round(m),
        "hough_line" => run_hough_line(m),
        "hough_circle" => run_hough_circle(m),
        other => bail!("arithmetic part_a has no command {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------------

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Load the single `IN` positional under the shared decode limits.
fn load_in(m: &ArgMatches) -> Result<Raster> {
    let limits = io::decode_limits(m);
    io::load(Path::new(pos(m, "IN")), &limits)
}

/// Parse a vips-style space-separated numeric vector (e.g. `"10 20 30"`).
fn parse_f64_vec(s: &str) -> Result<Vec<f64>> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| {
            t.parse::<f64>()
                .map_err(|e| anyhow!("constant {t:?} is not a number: {e}"))
        })
        .collect::<Result<_>>()?;
    if v.is_empty() {
        bail!("expected at least one constant (a space-separated vector like \"10 20\")");
    }
    Ok(v)
}

/// Parse a required scalar constant that the core backs only as a single value
/// (per-band vectors have no core op for `remainder_const` / `math2_const`).
fn parse_scalar_const(m: &ArgMatches, id: &str, what: &str) -> Result<f64> {
    let v = parse_f64_vec(pos(m, id))?;
    if v.len() != 1 {
        bail!(
            "{what} takes a single scalar constant, got {} values ({:?}); \
             per-band vector constants are not core-backed",
            v.len(),
            v
        );
    }
    Ok(v[0])
}

/// Format a scalar in the vips numeric print format (`avg` prints
/// `127.500000`, `CLI_CONTRACT.md` §3). The differential float-parses the value
/// with an epsilon rather than comparing text.
fn fmt_vips_double(v: f64) -> String {
    format!("{v:.6}")
}

/// Build a single-band `f32` matrix raster (`width` columns × `height` rows)
/// from `rows` of equal length — the `.v` carrier for `stats` / `measure`
/// (vips writes these as a 1-band double matrix; libviprs has no `f64` pixel
/// format, so the reference is `vips cast … float` and this side is `f32`).
fn matrix_to_raster(rows: &[Vec<f64>]) -> Result<Raster> {
    let height = rows.len();
    if height == 0 {
        bail!("cannot build a matrix image from zero rows");
    }
    let width = rows[0].len();
    if width == 0 {
        bail!("cannot build a matrix image with zero columns");
    }
    if let Some(bad) = rows.iter().find(|r| r.len() != width) {
        bail!(
            "ragged matrix: expected every row to have {width} columns, found one with {}",
            bad.len()
        );
    }
    let w = u32::try_from(width).map_err(|_| anyhow!("matrix width {width} exceeds u32"))?;
    let h = u32::try_from(height).map_err(|_| anyhow!("matrix height {height} exceeds u32"))?;
    let fmt = PixelFormat::with_channels(1, 4).expect("1-band f32 format exists");
    let samples: Vec<f32> = rows.iter().flatten().map(|&v| v as f32).collect();
    Raster::from_f32_samples(w, h, fmt, &samples)
        .map_err(|e| anyhow!("failed to build the matrix image: {e}"))
}

// ---------------------------------------------------------------------------
// Statistics — image → stdout scalar.
// ---------------------------------------------------------------------------

/// `avg IN` — S3; print the mean.
fn run_avg(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    println!("{}", fmt_vips_double(raster.avg()));
    Ok(())
}

/// `deviate IN` — S3; print the sample standard deviation.
fn run_deviate(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    println!("{}", fmt_vips_double(raster.deviate()));
    Ok(())
}

/// `min|max IN [--x] [--y]` — S3; print the extremum, and (per flag) its
/// position, in vips's `x`, `y`, value order (`minpos`/`maxpos` fold).
fn run_extremum(m: &ArgMatches, is_max: bool) -> Result<()> {
    let raster = load_in(m)?;
    let want_x = m.get_flag("x");
    let want_y = m.get_flag("y");
    // vips prints the position outputs (x, then y) BEFORE the value output.
    let (value, x, y) = if is_max {
        raster.maxpos()
    } else {
        raster.minpos()
    };
    if want_x {
        println!("{x}");
    }
    if want_y {
        println!("{y}");
    }
    println!("{}", fmt_vips_double(value));
    Ok(())
}

/// `find_trim IN [--background "c…"]` — S3; print left/top/width/height.
fn run_find_trim(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let bg = match m.get_one::<String>("background") {
        Some(s) => Some(parse_f64_vec(s)?),
        None => None,
    };
    let (left, top, width, height) = raster.try_find_trim(bg.as_deref())?;
    println!("{left}");
    println!("{top}");
    println!("{width}");
    println!("{height}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Statistics — image → matrix .v.
// ---------------------------------------------------------------------------

/// `stats IN OUT` — S1; write the 6-column per-band statistics matrix.
fn run_stats(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let matrix = raster.stats();
    let out = matrix_to_raster(&matrix)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `measure IN OUT H V` — S1; write the H×V patch-means matrix.
fn run_measure(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let h = *m.get_one::<u32>("H").expect("required");
    let v = *m.get_one::<u32>("V").expect("required");
    let matrix = raster.try_measure(h, v)?;
    let out = matrix_to_raster(&matrix)?;
    io::save(&out, &out_path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Statistics — image → two outputs.
// ---------------------------------------------------------------------------

/// `profile IN COLS ROWS` — S4; first non-zero position per column / row.
fn run_profile(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let cols_path = PathBuf::from(pos(m, "COLS"));
    let rows_path = PathBuf::from(pos(m, "ROWS"));
    let (cols, rows) = raster.profile();
    io::save(&cols, &cols_path)?;
    io::save(&rows, &rows_path)?;
    Ok(())
}

/// `project IN COLS ROWS` — S4; per-column and per-row sums.
fn run_project(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let cols_path = PathBuf::from(pos(m, "COLS"));
    let rows_path = PathBuf::from(pos(m, "ROWS"));
    let (cols, rows) = raster.project();
    io::save(&cols, &cols_path)?;
    io::save(&rows, &rows_path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Const / linear.
// ---------------------------------------------------------------------------

/// `linear IN OUT "a" "b" [--uchar]` — S1; scalar (broadcast) `a·in + b`.
fn run_linear(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let a = parse_f64_vec(pos(m, "A"))?;
    let b = parse_f64_vec(pos(m, "B"))?;
    // The core exposes only the scalar (broadcast) form; a genuine per-band
    // float linear would need a core float vector-linear op the public surface
    // lacks (the OP_MAP compose-note names mul_vec/add_vec, but those are
    // integer-promoting — they cannot compose to a float per-band linear
    // without an intermediate cast). Reject a per-band vector rather than fake
    // it (honest subset; see the wave report open question).
    if a.len() != 1 || b.len() != 1 {
        bail!(
            "linear takes a single scalar a and b (broadcast across bands); got a={a:?}, b={b:?}. \
             Per-band vector coefficients are not core-backed."
        );
    }
    let out = if m.get_flag("uchar") {
        raster.linear_uchar(a[0], b[0])
    } else {
        raster.linear(a[0], b[0])
    };
    io::save(&out, &out_path)?;
    Ok(())
}

/// `remainder_const IN OUT "c"` — S1; integer remainder by a scalar constant.
fn run_remainder_const(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let c = parse_scalar_const(m, "C", "remainder_const")?;
    let out = raster.try_rem_const(c)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `math2_const IN OUT pow "c"` — S1; per-sample power by a scalar exponent.
fn run_math2_const(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let exp = parse_scalar_const(m, "C", "math2_const")?;
    let out = match pos(m, "MATH2") {
        "pow" => raster.try_pow_const(exp)?,
        other => bail!("unsupported math2_const operation {other:?} (only pow is core-backed)"),
    };
    io::save(&out, &out_path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unary / rounding.
// ---------------------------------------------------------------------------

/// `abs IN OUT` — S1; absolute value.
fn run_abs(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    io::save(&raster.abs(), &PathBuf::from(pos(m, "OUT")))
}

/// `sign IN OUT` — S1; unit sign.
fn run_sign(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    io::save(&raster.sign(), &PathBuf::from(pos(m, "OUT")))
}

/// `clamp IN OUT [--min] [--max]` — S1; clip into `[min, max]`.
fn run_clamp(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let min = m.get_one::<f64>("min").copied();
    let max = m.get_one::<f64>("max").copied();
    // The core `clamp` asserts `min <= max` and would PANIC (abort) on a bad
    // pair. Resolve the effective bounds with vips's defaults (0, 1) and reject
    // any pair that is not ordered `lo <= hi` as a typed exit-1 error first
    // (CLI_CONTRACT.md §8). The guard is written as `!(lo <= hi)` rather than
    // `lo > hi` so a NaN bound is ALSO rejected: clap's f64 value_parser accepts
    // "nan"/"NaN", and `NaN > hi` is false (so a bare `>` would let NaN slip
    // past into the core assert and abort with exit 101), whereas `NaN <= hi` is
    // false, so `!(lo <= hi)` catches it while still admitting ordered ±inf.
    let lo = min.unwrap_or(0.0);
    let hi = max.unwrap_or(1.0);
    // Reject any pair that is not ordered `lo <= hi`. This MUST test `lo <= hi`
    // (then negate the bool), NOT `lo > hi`: for a NaN bound `NaN > hi` is false
    // (a bare `>` would let NaN slip into the core assert and abort with exit
    // 101) while `NaN <= hi` is false, so negating the `<=` result catches NaN
    // while still admitting an ordered ±inf pair.
    let ordered = lo <= hi;
    if !ordered {
        bail!("clamp: --min {lo} is not <= --max {hi}");
    }
    io::save(&raster.clamp(min, max), &out_path)
}

/// `round IN OUT rint|ceil|floor` — S1; rounding.
fn run_round(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let out = match pos(m, "ROUND") {
        "rint" => raster.rint(),
        "ceil" => raster.ceil(),
        "floor" => raster.floor(),
        other => bail!("unknown round mode {other:?} (expected rint|ceil|floor)"),
    };
    io::save(&out, &out_path)
}

// ---------------------------------------------------------------------------
// Hough.
// ---------------------------------------------------------------------------

/// `hough_line IN OUT` — S1; fixed 256×256 accumulator.
fn run_hough_line(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    io::save(&raster.hough_line(), &PathBuf::from(pos(m, "OUT")))
}

/// `hough_circle IN OUT MIN MAX` — S1; scale-1 accumulator, radii MIN..=MAX.
fn run_hough_circle(m: &ArgMatches) -> Result<()> {
    let raster = load_in(m)?;
    let out_path = PathBuf::from(pos(m, "OUT"));
    let min_radius = *m.get_one::<u32>("MIN_RADIUS").expect("required");
    let max_radius = *m.get_one::<u32>("MAX_RADIUS").expect("required");
    let out = raster.try_hough_circle(min_radius, max_radius)?;
    io::save(&out, &out_path)
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
        assert_eq!(meta_names.len(), 18, "arith part_a has eighteen commands");
    }

    #[test]
    fn fmt_vips_double_matches_contract_example() {
        assert_eq!(fmt_vips_double(127.5), "127.500000");
        assert_eq!(fmt_vips_double(0.0), "0.000000");
    }

    #[test]
    fn parse_f64_vec_reads_a_space_separated_vector() {
        assert_eq!(parse_f64_vec("10 20 30").unwrap(), vec![10.0, 20.0, 30.0]);
        assert_eq!(parse_f64_vec(" -1 ").unwrap(), vec![-1.0]);
        assert!(parse_f64_vec("   ").is_err(), "an empty vector is an error");
        assert!(parse_f64_vec("10 x").is_err(), "a non-number is an error");
    }

    #[test]
    fn parse_scalar_const_rejects_a_vector() {
        let m = cmd("remainder_const")
            .try_get_matches_from(["remainder_const", "in.png", "out.png", "10 20"])
            .unwrap();
        assert!(
            parse_scalar_const(&m, "C", "remainder_const").is_err(),
            "a multi-element constant must be rejected (no core vector op)"
        );
        let m1 = cmd("remainder_const")
            .try_get_matches_from(["remainder_const", "in.png", "out.png", "10"])
            .unwrap();
        assert_eq!(
            parse_scalar_const(&m1, "C", "remainder_const").unwrap(),
            10.0
        );
    }

    #[test]
    fn matrix_to_raster_builds_a_float_matrix() {
        let r = matrix_to_raster(&[vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();
        assert!(r.format().is_float());
        assert_eq!((r.width(), r.height()), (3, 2));
        assert_eq!(r.f32_samples().unwrap(), vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn matrix_to_raster_rejects_ragged_rows() {
        assert!(matrix_to_raster(&[vec![1.0, 2.0], vec![3.0]]).is_err());
        assert!(matrix_to_raster(&[]).is_err());
    }

    #[test]
    fn min_max_parse_x_y_flags() {
        let m = cmd("min")
            .try_get_matches_from(["min", "in.png", "--x", "--y"])
            .unwrap();
        assert!(m.get_flag("x") && m.get_flag("y"));
        let m2 = cmd("max").try_get_matches_from(["max", "in.png"]).unwrap();
        assert!(!m2.get_flag("x") && !m2.get_flag("y"));
    }

    #[test]
    fn round_enforces_the_enum() {
        assert!(
            cmd("round")
                .clone()
                .try_get_matches_from(["round", "in.v", "out.v", "floor"])
                .is_ok()
        );
        assert!(
            cmd("round")
                .try_get_matches_from(["round", "in.v", "out.v", "trunc"])
                .is_err(),
            "only rint|ceil|floor are accepted"
        );
    }

    #[test]
    fn math2_const_only_accepts_pow() {
        assert!(
            cmd("math2_const")
                .clone()
                .try_get_matches_from(["math2_const", "in.png", "out.v", "pow", "2"])
                .is_ok()
        );
        assert!(
            cmd("math2_const")
                .try_get_matches_from(["math2_const", "in.png", "out.v", "wop", "2"])
                .is_err(),
            "only pow is core-backed; wop/atan2 are rejected"
        );
    }

    #[test]
    fn measure_rejects_zero_patches() {
        assert!(
            cmd("measure")
                .try_get_matches_from(["measure", "in.png", "out.v", "0", "2"])
                .is_err(),
            "vips's minimum patch count is 1"
        );
    }

    #[test]
    fn hough_circle_rejects_zero_radius() {
        assert!(
            cmd("hough_circle")
                .try_get_matches_from(["hough_circle", "in.v", "out.v", "0", "4"])
                .is_err(),
            "vips's minimum radius is 1"
        );
    }

    #[test]
    fn linear_rejects_a_per_band_vector() {
        // A multi-element a/b is a per-band linear the core cannot back; it must
        // be a typed error, not a silent wrong result.
        let m = cmd("linear")
            .try_get_matches_from(["linear", &tmp_gray(), &out_tmp("lin.v"), "2 3", "1 1"])
            .unwrap();
        let err = run_linear(&m).unwrap_err();
        assert!(err.to_string().contains("scalar"), "got: {err}");
    }

    #[test]
    fn clamp_rejects_inverted_bounds() {
        // --min > --max must be a typed exit-1 error, never the core clamp's
        // assert-panic (abort).
        let m = cmd("clamp")
            .try_get_matches_from([
                "clamp",
                &tmp_gray(),
                &out_tmp("cl.png"),
                "--min",
                "200",
                "--max",
                "50",
            ])
            .unwrap();
        let err = run_clamp(&m).unwrap_err();
        assert!(err.to_string().contains("is not <="), "got: {err}");
    }

    #[test]
    fn clamp_rejects_nan_bound() {
        // clap's f64 value_parser accepts "nan"; a NaN bound must be a typed
        // exit-1 error, NOT the core clamp's `assert!(lo <= hi)` abort. `NaN > hi`
        // is false, so the old `lo > hi` guard let NaN through to the panic — the
        // `!(lo <= hi)` guard catches it. Cover both bounds.
        for (min_v, max_v) in [("nan", "200"), ("0", "nan")] {
            let m = cmd("clamp")
                .try_get_matches_from([
                    "clamp",
                    &tmp_gray(),
                    &out_tmp("cl_nan.png"),
                    "--min",
                    min_v,
                    "--max",
                    max_v,
                ])
                .unwrap();
            let err = run_clamp(&m).unwrap_err();
            assert!(
                err.to_string().contains("is not <="),
                "NaN bound (--min {min_v} --max {max_v}) must be a typed error, got: {err}"
            );
        }
    }

    // --- tiny on-disk fixtures for the handler-level tests above ---

    /// Write a 4×4 Gray8 ramp PNG to a temp path and return it.
    fn tmp_gray() -> String {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("viprs_aritha_pa_gray_{}.png", std::process::id()));
        let fmt = PixelFormat::Gray8;
        let data: Vec<u8> = (0..16u16).map(|i| (i * 16) as u8).collect();
        let r = Raster::new(4, 4, fmt, data).unwrap();
        let bytes = libviprs::sink::encode_png(&r).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// A temp output path (never written by the rejection tests, which fail
    /// before the save).
    fn out_tmp(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("viprs_aritha_pa_{}_{name}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }
}
