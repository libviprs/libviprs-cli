//! Resample (geometry) op family (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md`
//! resample section).
//!
//! The core `src/resample.rs` exposes 43 `pub fn` that fold to fifteen base ops;
//! thirteen become `viprs` subcommands here (the two `thumbnail_buffer` /
//! `constant_u8` rows are EXCLUDED per `OP_MAP.md`, and the `_with` / free-fn
//! twins collapse into flags). Every command is oracle class **BOUNDED-TOL**
//! (the premultiply / rounding campaign #406-418): ≤1 LSB for 8-bit and ≤1 LSB
//! per channel for 16-bit, because the core computes the reduce / interpolate
//! masks in `f64` per output position while vips quantises the sub-pixel offset
//! into fixed-point tables — the same convolution, off by at most one
//! least-significant bit.
//!
//! **Oracle scope — non-alpha inputs only.** The ≤1 LSB BOUNDED-TOL contract
//! holds for inputs WITHOUT an alpha channel. The core `shrink` / `reduce` /
//! `resize` premultiply alpha before resampling (a documented, intentional
//! divergence from bare `vips_shrink` / `vips_reduce`, which do not), so on an
//! RGBA / GrayA input these ops diverge from vips wholesale — well beyond ≤1
//! LSB (measured max-abs-diff 4 for `shrink 2 2` on a 4-band sRGB ramp). That
//! divergence is deliberate and OUT of this oracle class; the differential
//! suite exercises only non-alpha `grad` / `rgb` carriers accordingly.
//!
//! | command | vips | shape | notes |
//! |---|---|---|---|
//! | `shrink IN OUT HSHRINK VSHRINK`        | `shrink`   | S1 | box shrink, f64 factors |
//! | `shrinkh IN OUT HSHRINK`               | `shrinkh`  | S1 | one-axis integer box shrink |
//! | `shrinkv IN OUT VSHRINK`               | `shrinkv`  | S1 | one-axis integer box shrink |
//! | `reduce IN OUT HSHRINK VSHRINK --kernel`| `reduce`  | S1 | kernel downsample |
//! | `reduceh IN OUT HSHRINK --kernel`      | `reduceh`  | S1 | one-axis kernel reduce |
//! | `reducev IN OUT VSHRINK --kernel`      | `reducev`  | S1 | one-axis kernel reduce |
//! | `resize IN OUT SCALE --kernel --vscale`| `resize`   | S1 | scale by a factor |
//! | `affine IN OUT "a b c d" --interpolate`| `affine`   | S1 | 2x2 matrix transform |
//! | `similarity IN OUT --scale --angle --interpolate` | `similarity` | S1 | rotate + scale |
//! | `rotate IN OUT ANGLE --interpolate`    | `rotate`   | S1 | rotate by degrees |
//! | `mapim IN OUT INDEX --interpolate`     | `mapim`    | S2 | remap through a 2-band index image |
//! | `thumbnail FILENAME OUT WIDTH …`       | `thumbnail`| S1 | source-FILENAME input (not a decoded image) |
//! | `thumbnail_image IN OUT WIDTH --height`| `thumbnail_image` | S1 | decoded-image variant |
//!
//! Positional orders, flag names, enum spellings, and input bounds mirror vips
//! 8.18.4 exactly (verified against `vips <op>`): factor / scale minima follow
//! vips's own (`shrink*` factor ≥ 1, `resize` scale > 0), the `--kernel` enum is
//! the six core `VipsKernel` names (vips's core-only `mks2013` / `mks2021` are
//! NOT exposed — the CLI restricts to what the core supports), and
//! `--interpolate` is the five core interpolators (vips's `vsqbs` etc. are not
//! core-backed). `mapim`'s index is a SECOND input image at the vips positional
//! `in out index`. `thumbnail`'s first arg is a source FILENAME, not a decoded
//! raster (`thumbnail filename out width`).
//!
//! Handlers keep the §3 `load → try_op → save` shape and call only the panic-free
//! `try_*` core APIs, so a bad input becomes exit 1 rather than an abort
//! (`CLI_CONTRACT.md` §8). Every numeric range conversion (e.g. an out-of-range
//! `--height`) yields a typed exit-1 error, never an `as`-cast abort (the bands
//! B2 lesson).
//
// @doc-command:begin name=shrink about="Shrink an image by integer or fractional factors with a box filter." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=shrink
// @doc-command:begin name=shrinkh about="Shrink an image horizontally by an integer factor." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=shrinkh
// @doc-command:begin name=shrinkv about="Shrink an image vertically by an integer factor." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=shrinkv
// @doc-command:begin name=reduce about="Downsample an image with an anti-aliasing kernel." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=reduce
// @doc-command:begin name=reduceh about="Downsample an image horizontally with a kernel." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=reduceh
// @doc-command:begin name=reducev about="Downsample an image vertically with a kernel." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=reducev
// @doc-command:begin name=resize about="Resize an image by a scale factor." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=resize
// @doc-command:begin name=affine about="Apply a 2x2 affine matrix transform to an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=affine
// @doc-command:begin name=similarity about="Rotate and scale an image (similarity transform)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=similarity
// @doc-command:begin name=rotate about="Rotate an image by an arbitrary angle in degrees." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=rotate
// @doc-command:begin name=mapim about="Resample an image through a two-band coordinate index image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=mapim
// @doc-command:begin name=thumbnail about="Make a thumbnail from an image file, fitting a box." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=thumbnail
// @doc-command:begin name=thumbnail_image about="Make a thumbnail from an already-loaded image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=thumbnail_image

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::{Interpolator, ReduceKernel, ResizeOptions};

use super::{CommandMeta, OracleClass, Shape, io};

/// The six core `VipsKernel` nicknames the CLI exposes for `--kernel`. vips
/// 8.18.4 additionally lists `mks2013` / `mks2021`, which the core
/// [`ReduceKernel`] does not implement, so they are deliberately NOT offered
/// (the CLI restricts to the core-backed subset, not vips's full enum).
const KERNELS: [&str; 6] = [
    "nearest", "linear", "cubic", "mitchell", "lanczos2", "lanczos3",
];

/// The five core interpolator nicknames the CLI exposes for `--interpolate`.
/// vips has further interpolators (`vsqbs`, …) that the core [`Interpolator`]
/// does not implement; the CLI restricts to the core-backed set.
const INTERPOLATORS: [&str; 5] = ["nearest", "bilinear", "bicubic", "nohalo", "lbb"];

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6). Every
/// resample op is **BOUNDED-TOL** (≤1 LSB, the premultiply / rounding campaign
/// #406-418; see the module header).
pub fn metas() -> Vec<CommandMeta> {
    let s1 = |name| CommandMeta {
        name,
        shape: Shape::ImageToImage,
        oracle_class: OracleClass::BoundedTol,
    };
    vec![
        s1("shrink"),
        s1("shrinkh"),
        s1("shrinkv"),
        s1("reduce"),
        s1("reduceh"),
        s1("reducev"),
        s1("resize"),
        s1("affine"),
        s1("similarity"),
        s1("rotate"),
        // mapim takes a SECOND input image (the index), so it is the S2
        // N-image→image shape even though vips lays it out `in out index`.
        CommandMeta {
            name: "mapim",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        s1("thumbnail"),
        s1("thumbnail_image"),
    ]
}

/// Append the shared `--kernel` flag (default `lanczos3`, vips's own default).
fn with_kernel(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("kernel")
            .long("kernel")
            .value_name("K")
            .default_value("lanczos3")
            .value_parser(KERNELS)
            .help("Resampling kernel (nearest|linear|cubic|mitchell|lanczos2|lanczos3)"),
    )
}

/// Append the shared `--interpolate` flag (default `bilinear`, vips's own
/// interpolator default for affine / similarity / rotate / mapim).
fn with_interpolate(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("interpolate")
            .long("interpolate")
            .value_name("I")
            .default_value("bilinear")
            .value_parser(INTERPOLATORS)
            .help("Point interpolator (nearest|bilinear|bicubic|nohalo|lbb)"),
    )
}

/// The clap commands this family contributes to the assembled CLI. Every
/// command loads at least one image, so each carries the shared decode-limit
/// flags via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("shrink")
                .about("Shrink an image by integer or fractional factors with a box filter.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("HSHRINK")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Horizontal shrink factor (>= 1)"),
                )
                .arg(
                    Arg::new("VSHRINK")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Vertical shrink factor (>= 1)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("shrinkh")
                .about("Shrink an image horizontally by an integer factor.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("HSHRINK")
                        .required(true)
                        // vips's own minimum is 1 (a gint factor); reject 0 /
                        // negatives at parse time rather than deferring to core.
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Horizontal shrink factor (>= 1)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("shrinkv")
                .about("Shrink an image vertically by an integer factor.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("VSHRINK")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Vertical shrink factor (>= 1)"),
                ),
        ),
        with_kernel(io::with_decode_limit_args(
            Command::new("reduce")
                .about("Downsample an image with an anti-aliasing kernel.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("HSHRINK")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Horizontal shrink factor (>= 1)"),
                )
                .arg(
                    Arg::new("VSHRINK")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Vertical shrink factor (>= 1)"),
                ),
        )),
        with_kernel(io::with_decode_limit_args(
            Command::new("reduceh")
                .about("Downsample an image horizontally with a kernel.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("HSHRINK")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Horizontal shrink factor (>= 1)"),
                ),
        )),
        with_kernel(io::with_decode_limit_args(
            Command::new("reducev")
                .about("Downsample an image vertically with a kernel.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("VSHRINK")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Vertical shrink factor (>= 1)"),
                ),
        )),
        with_kernel(io::with_decode_limit_args(
            Command::new("resize")
                .about("Resize an image by a scale factor.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("SCALE")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Scale factor (> 0)"),
                )
                .arg(
                    Arg::new("vscale")
                        .long("vscale")
                        .value_name("S")
                        .value_parser(value_parser!(f64))
                        .help("Vertical scale factor (> 0); defaults to SCALE when omitted"),
                ),
        )),
        with_interpolate(io::with_decode_limit_args(
            Command::new("affine")
                .about("Apply a 2x2 affine matrix transform to an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("MATRIX")
                        .required(true)
                        .value_name("a b c d")
                        .help("2x2 transform matrix as one space-separated string, e.g. \"1.5 0 0 1.5\""),
                ),
        )),
        with_interpolate(io::with_decode_limit_args(
            Command::new("similarity")
                .about("Rotate and scale an image (similarity transform).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("scale")
                        .long("scale")
                        .value_name("S")
                        .default_value("1")
                        .value_parser(value_parser!(f64))
                        .help("Scale by this factor (> 0)"),
                )
                .arg(
                    Arg::new("angle")
                        .long("angle")
                        .value_name("DEG")
                        .default_value("0")
                        .value_parser(value_parser!(f64))
                        .help("Rotate clockwise by this many degrees"),
                ),
        )),
        with_interpolate(io::with_decode_limit_args(
            Command::new("rotate")
                .about("Rotate an image by an arbitrary angle in degrees.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("ANGLE")
                        .required(true)
                        .value_parser(value_parser!(f64))
                        .help("Rotate clockwise by this many degrees"),
                ),
        )),
        with_interpolate(io::with_decode_limit_args(
            Command::new("mapim")
                .about("Resample an image through a two-band coordinate index image.")
                // vips positional order is `in out index` (NOT the variadic
                // inputs-before-out idiom): OUT is the second arg, the index is
                // the third (a SECOND input image).
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("INDEX")
                        .required(true)
                        .help("Two-band coordinate index image (band 0 = source x, band 1 = source y)"),
                ),
        )),
        io::with_decode_limit_args(
            Command::new("thumbnail")
                .about("Make a thumbnail from an image file, fitting a box.")
                // vips's first arg is a source FILENAME, not a decoded image.
                .arg(
                    Arg::new("FILENAME")
                        .required(true)
                        .help("Source image file to read from"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("WIDTH")
                        .required(true)
                        // vips's own minimum is 1.
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Fit within this width (>= 1)"),
                )
                .arg(
                    Arg::new("height")
                        .long("height")
                        .value_name("PX")
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Fit within this height (>= 1); a square WIDTH box when omitted"),
                )
                .arg(
                    // vips `thumbnail --crop` takes a VipsInteresting enum value
                    // (none|centre|entropy|attention|low|high|all). The core
                    // supports only centre-crop, so this flag accepts an OPTIONAL
                    // value (`--crop` alone == `--crop centre`, matching the bare
                    // form; `--crop none` == no crop) and rejects the unsupported
                    // modes with a typed exit-1 in `run_thumbnail` rather than
                    // silently accepting a value it cannot honour. This preserves
                    // vips's literal `--crop centre` syntax instead of erroring on
                    // it as an unexpected argument.
                    Arg::new("crop")
                        .long("crop")
                        .value_name("INTERESTING")
                        .num_args(0..=1)
                        .default_missing_value("centre")
                        .help(
                            "Fill the box and centre-crop (vips --crop; core supports \
                             none|centre only — `--crop` == `--crop centre`)",
                        ),
                )
                .arg(
                    Arg::new("linear")
                        .long("linear")
                        .action(ArgAction::SetTrue)
                        .help("Reduce in linear light (square WIDTH box; not combinable with --height/--crop)"),
                )
                .arg(
                    Arg::new("export-profile")
                        .long("export-profile")
                        .value_name("NAME")
                        .help(
                            "Export through the embedded ICC profile to this output profile \
                             (only \"srgb\"; square WIDTH box; not combinable with the others)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("thumbnail_image")
                .about("Make a thumbnail from an already-loaded image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("WIDTH")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Fit within a square WIDTH box (>= 1)"),
                ),
        ),
    ]
}

/// Dispatch a matched resample subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "shrink" => run_shrink(m),
        "shrinkh" => run_shrinkh(m),
        "shrinkv" => run_shrinkv(m),
        "reduce" => run_reduce(m),
        "reduceh" => run_reduceh(m),
        "reducev" => run_reducev(m),
        "resize" => run_resize(m),
        "affine" => run_affine(m),
        "similarity" => run_similarity(m),
        "rotate" => run_rotate(m),
        "mapim" => run_mapim(m),
        "thumbnail" => run_thumbnail(m),
        "thumbnail_image" => run_thumbnail_image(m),
        other => bail!("resample family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Parse the kernel nickname (clap already restricted it to [`KERNELS`]).
fn kernel_arg(m: &ArgMatches) -> Result<ReduceKernel> {
    let name = m
        .get_one::<String>("kernel")
        .expect("clap default lanczos3");
    ReduceKernel::from_name(name).map_err(|e| anyhow!(e))
}

/// Parse the interpolator nickname (clap already restricted it to
/// [`INTERPOLATORS`]).
fn interpolate_arg(m: &ArgMatches) -> Result<Interpolator> {
    let name = m
        .get_one::<String>("interpolate")
        .expect("clap default bilinear");
    Interpolator::from_name(name).map_err(|e| anyhow!(e))
}

/// Parse a vips-style space-separated numeric vector (e.g. `"1.5 0 0 1.5"`).
fn parse_f64_vec(s: &str) -> Result<Vec<f64>> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| {
            t.parse::<f64>()
                .map_err(|e| anyhow!("value {t:?} is not a number: {e}"))
        })
        .collect::<Result<_>>()?;
    if v.is_empty() {
        bail!("expected a space-separated numeric vector, got an empty string");
    }
    Ok(v)
}

/// `shrink IN OUT HSHRINK VSHRINK` — S1 box shrink.
fn run_shrink(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let hshrink = *m.get_one::<f64>("HSHRINK").expect("required");
    let vshrink = *m.get_one::<f64>("VSHRINK").expect("required");

    // @doc-snippet:begin command=shrink slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=shrink slot=load

    // @doc-snippet:begin command=shrink slot=apply
    let out = raster.try_shrink(hshrink, vshrink)?;
    // @doc-snippet:end command=shrink slot=apply

    // @doc-snippet:begin command=shrink slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=shrink slot=save
    Ok(())
}

/// `shrinkh IN OUT HSHRINK` — S1 one-axis integer box shrink.
fn run_shrinkh(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let hshrink = *m.get_one::<u32>("HSHRINK").expect("required");

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_shrinkh(hshrink)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `shrinkv IN OUT VSHRINK` — S1 one-axis integer box shrink.
fn run_shrinkv(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let vshrink = *m.get_one::<u32>("VSHRINK").expect("required");

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_shrinkv(vshrink)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `reduce IN OUT HSHRINK VSHRINK --kernel` — S1 kernel downsample.
fn run_reduce(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let hshrink = *m.get_one::<f64>("HSHRINK").expect("required");
    let vshrink = *m.get_one::<f64>("VSHRINK").expect("required");
    let kernel = kernel_arg(m)?;

    // @doc-snippet:begin command=reduce slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=reduce slot=load

    // @doc-snippet:begin command=reduce slot=apply
    let out = raster.try_reduce(hshrink, vshrink, kernel)?;
    // @doc-snippet:end command=reduce slot=apply

    // @doc-snippet:begin command=reduce slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=reduce slot=save
    Ok(())
}

/// `reduceh IN OUT HSHRINK --kernel` — S1 one-axis kernel reduce.
fn run_reduceh(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let hshrink = *m.get_one::<f64>("HSHRINK").expect("required");
    let kernel = kernel_arg(m)?;

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_reduceh(hshrink, kernel)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `reducev IN OUT VSHRINK --kernel` — S1 one-axis kernel reduce.
fn run_reducev(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let vshrink = *m.get_one::<f64>("VSHRINK").expect("required");
    let kernel = kernel_arg(m)?;

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_reducev(vshrink, kernel)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `resize IN OUT SCALE --kernel --vscale` — S1 scale by a factor.
fn run_resize(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let scale = *m.get_one::<f64>("SCALE").expect("required");
    let kernel = kernel_arg(m)?;
    let vscale = m.get_one::<f64>("vscale").copied();

    // @doc-snippet:begin command=resize slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=resize slot=load

    // @doc-snippet:begin command=resize slot=apply
    let options = ResizeOptions::default()
        .with_vscale(vscale)
        .with_kernel(kernel);
    let out = raster.try_resize_with(scale, options)?;
    // @doc-snippet:end command=resize slot=apply

    // @doc-snippet:begin command=resize slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=resize slot=save
    Ok(())
}

/// `affine IN OUT "a b c d" --interpolate` — S1 matrix transform.
fn run_affine(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let interpolate = interpolate_arg(m)?;
    let v = parse_f64_vec(pos(m, "MATRIX"))?;
    if v.len() != 4 {
        bail!(
            "affine matrix must be exactly four numbers \"a b c d\", got {} value(s)",
            v.len()
        );
    }
    let matrix = [v[0], v[1], v[2], v[3]];

    // @doc-snippet:begin command=affine slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=affine slot=load

    // @doc-snippet:begin command=affine slot=apply
    let out = raster.try_affine(matrix, interpolate)?;
    // @doc-snippet:end command=affine slot=apply

    // @doc-snippet:begin command=affine slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=affine slot=save
    Ok(())
}

/// `similarity IN OUT --scale --angle --interpolate` — S1 rotate + scale.
fn run_similarity(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let scale = *m.get_one::<f64>("scale").expect("clap default 1");
    let angle = *m.get_one::<f64>("angle").expect("clap default 0");
    let interpolate = interpolate_arg(m)?;

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_similarity_with(angle, scale, interpolate)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `rotate IN OUT ANGLE --interpolate` — S1 rotate by degrees.
fn run_rotate(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let angle = *m.get_one::<f64>("ANGLE").expect("required");
    let interpolate = interpolate_arg(m)?;

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_rotate_with(angle, interpolate)?;
    io::save(&out, &out_path)?;
    Ok(())
}

/// `mapim IN OUT INDEX --interpolate` — S2; the index is a SECOND input image
/// (vips positional order `in out index`).
fn run_mapim(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let index_path = PathBuf::from(pos(m, "INDEX"));
    let interpolate = interpolate_arg(m)?;

    // @doc-snippet:begin command=mapim slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let index = io::load(&index_path, &limits)?;
    // @doc-snippet:end command=mapim slot=load

    // @doc-snippet:begin command=mapim slot=apply
    let out = raster.try_mapim(&index, interpolate)?;
    // @doc-snippet:end command=mapim slot=apply

    // @doc-snippet:begin command=mapim slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=mapim slot=save
    Ok(())
}

/// `thumbnail FILENAME OUT WIDTH [--height --crop --linear --export-profile]`
/// — S1; the first arg is a source FILENAME (not a decoded raster). The core
/// exposes three disjoint thumbnail entry points; the flags select between them
/// and unsupported combinations are rejected with a typed exit-1 error rather
/// than silently ignored.
fn run_thumbnail(m: &ArgMatches) -> Result<()> {
    let filename = PathBuf::from(pos(m, "FILENAME"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = m.get_one::<u32>("height").copied();
    // `--crop` carries an optional VipsInteresting value; the core supports only
    // centre-crop, so map none→false and centre→true and reject every other vips
    // mode with a typed exit-1 (never a silent accept-and-ignore).
    let crop = match m.get_one::<String>("crop").map(String::as_str) {
        None => false,
        Some("centre" | "center") => true,
        Some("none") => false,
        Some(other) => bail!(
            "--crop {other:?} is not supported: the core does centre-crop only \
             (`--crop` or `--crop centre`) or no crop (`--crop none`); vips's \
             entropy / attention / low / high / all crop modes are unavailable"
        ),
    };
    let linear = m.get_flag("linear");
    let export_profile = m.get_one::<String>("export-profile").map(String::as_str);

    let out = if let Some(profile) = export_profile {
        // The ICC export path is a square WIDTH box only; the other size / space
        // flags are not part of its core signature.
        if height.is_some() || crop || linear {
            bail!(
                "--export-profile fits a square WIDTH box and cannot be combined with \
                 --height / --crop / --linear"
            );
        }
        libviprs::Raster::try_thumbnail_with_profile(&filename, width, profile)?
    } else if linear {
        // Linear-light reduce is a square WIDTH box only (the core
        // `thumbnail_with_options` signature has no height / crop).
        if height.is_some() || crop {
            bail!("--linear fits a square WIDTH box and cannot be combined with --height / --crop");
        }
        libviprs::Raster::try_thumbnail_with_options(&filename, width, true)?
    } else {
        libviprs::Raster::try_thumbnail(&filename, width, height, crop)?
    };

    io::save(&out, &out_path)?;
    Ok(())
}

/// `thumbnail_image IN OUT WIDTH` — S1; the decoded-image variant fitting a
/// square WIDTH box.
fn run_thumbnail_image(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let width = *m.get_one::<u32>("WIDTH").expect("required");

    let raster = io::load(&in_path, &limits)?;
    let out = raster.try_thumbnail_image(width)?;
    io::save(&out, &out_path)?;
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
            13,
            "the resample family has thirteen commands"
        );
    }

    #[test]
    fn shrink_parses_positionals_in_vips_order() {
        let m = cmd("shrink")
            .try_get_matches_from(["shrink", "in.png", "out.png", "2", "3"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(*m.get_one::<f64>("HSHRINK").unwrap(), 2.0);
        assert_eq!(*m.get_one::<f64>("VSHRINK").unwrap(), 3.0);
    }

    #[test]
    fn shrinkh_rejects_a_zero_factor() {
        // vips's own minimum is 1; the `1..` value_parser rejects 0 at parse time.
        assert!(
            cmd("shrinkh")
                .try_get_matches_from(["shrinkh", "in.png", "out.png", "0"])
                .is_err(),
            "a shrink factor of 0 must be rejected"
        );
    }

    #[test]
    fn reduce_kernel_defaults_to_lanczos3_and_enforces_the_enum() {
        let m = cmd("reduce")
            .try_get_matches_from(["reduce", "in.png", "out.png", "2", "2"])
            .unwrap();
        assert_eq!(kernel_arg(&m).unwrap(), ReduceKernel::Lanczos3);
        let m = cmd("reduce")
            .try_get_matches_from(["reduce", "in.png", "out.png", "2", "2", "--kernel", "cubic"])
            .unwrap();
        assert_eq!(kernel_arg(&m).unwrap(), ReduceKernel::Cubic);
        // vips's core-only mks2013 / mks2021 are NOT exposed (no core kernel).
        assert!(
            cmd("reduce")
                .try_get_matches_from([
                    "reduce", "in.png", "out.png", "2", "2", "--kernel", "mks2013"
                ])
                .is_err(),
            "a non-core kernel must be rejected"
        );
    }

    #[test]
    fn resize_reads_scale_and_optional_vscale() {
        let m = cmd("resize")
            .try_get_matches_from(["resize", "in.png", "out.png", "0.5"])
            .unwrap();
        assert_eq!(*m.get_one::<f64>("SCALE").unwrap(), 0.5);
        assert_eq!(m.get_one::<f64>("vscale").copied(), None);
        let m = cmd("resize")
            .try_get_matches_from(["resize", "in.png", "out.png", "0.5", "--vscale", "0.75"])
            .unwrap();
        assert_eq!(m.get_one::<f64>("vscale").copied(), Some(0.75));
    }

    #[test]
    fn affine_matrix_must_have_four_values() {
        assert_eq!(parse_f64_vec("1.5 0 0 1.5").unwrap().len(), 4);
        assert!(parse_f64_vec("").is_err());
        assert!(parse_f64_vec("1 x 0 1").is_err());
    }

    #[test]
    fn affine_interpolate_defaults_to_bilinear() {
        let m = cmd("affine")
            .try_get_matches_from(["affine", "in.png", "out.png", "1 0 0 1"])
            .unwrap();
        assert_eq!(interpolate_arg(&m).unwrap(), Interpolator::Bilinear);
        let m = cmd("affine")
            .try_get_matches_from([
                "affine",
                "in.png",
                "out.png",
                "1 0 0 1",
                "--interpolate",
                "bicubic",
            ])
            .unwrap();
        assert_eq!(interpolate_arg(&m).unwrap(), Interpolator::Bicubic);
    }

    #[test]
    fn similarity_scale_and_angle_default() {
        let m = cmd("similarity")
            .try_get_matches_from(["similarity", "in.png", "out.png"])
            .unwrap();
        assert_eq!(*m.get_one::<f64>("scale").unwrap(), 1.0);
        assert_eq!(*m.get_one::<f64>("angle").unwrap(), 0.0);
    }

    #[test]
    fn mapim_reads_in_out_index_in_vips_order() {
        let m = cmd("mapim")
            .try_get_matches_from(["mapim", "in.png", "out.png", "index.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "INDEX"), "index.v");
    }

    #[test]
    fn thumbnail_reads_filename_width_and_flags() {
        // Bare `--crop` takes the default_missing_value "centre".
        let m = cmd("thumbnail")
            .try_get_matches_from(["thumbnail", "in.png", "out.png", "16", "--crop"])
            .unwrap();
        assert_eq!(pos(&m, "FILENAME"), "in.png");
        assert_eq!(*m.get_one::<u32>("WIDTH").unwrap(), 16);
        assert_eq!(
            m.get_one::<String>("crop").map(String::as_str),
            Some("centre")
        );
        assert!(!m.get_flag("linear"));
        // The literal vips syntax `--crop centre` now parses (was a clap
        // "unexpected argument" error before the optional-value arity).
        let m = cmd("thumbnail")
            .try_get_matches_from(["thumbnail", "in.png", "out.png", "16", "--crop", "centre"])
            .unwrap();
        assert_eq!(
            m.get_one::<String>("crop").map(String::as_str),
            Some("centre")
        );
    }

    #[test]
    fn thumbnail_rejects_an_unsupported_crop_mode() {
        // vips's entropy/attention/low/high/all crop modes are not core-backed;
        // passing one is a typed exit-1 error, never a silent centre-crop.
        let m = cmd("thumbnail")
            .try_get_matches_from([
                "thumbnail",
                "in.png",
                "out.png",
                "16",
                "--crop",
                "attention",
            ])
            .unwrap();
        let err = run_thumbnail(&m).unwrap_err();
        assert!(
            err.to_string().contains("--crop"),
            "expected a crop-mode error, got: {err}"
        );
    }

    #[test]
    fn thumbnail_rejects_zero_width() {
        assert!(
            cmd("thumbnail")
                .try_get_matches_from(["thumbnail", "in.png", "out.png", "0"])
                .is_err(),
            "a thumbnail width of 0 must be rejected"
        );
    }

    #[test]
    fn thumbnail_rejects_incompatible_flag_combinations() {
        // --linear cannot carry --crop; the combination is a typed exit-1 error,
        // never a silent ignore (the core linear path is a square WIDTH box).
        let m = cmd("thumbnail")
            .try_get_matches_from(["thumbnail", "in.png", "out.png", "16", "--linear", "--crop"])
            .unwrap();
        let err = run_thumbnail(&m).unwrap_err();
        assert!(
            err.to_string().contains("--linear"),
            "expected a flag-combination error, got: {err}"
        );
    }
}
