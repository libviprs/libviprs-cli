//! Conversion op family — the Wave-2 **conversion** lane (`CLI_CONTRACT.md`
//! §3/§6, `OP_MAP.md` conversion section).
//!
//! The core `src/conversion.rs` exposes 53 `pub fn` that fold to 21 base ops;
//! those become **nineteen** `viprs` subcommands here (the `fliphor`/`flipver`
//! twins collapse into `flip` with a `horizontal|vertical` enum, and
//! `identity_ushort` folds into `identity --ushort`, exactly as `OP_MAP.md`
//! prescribes). Command names, positional orders, flag names, enum spellings and
//! input value bounds mirror vips 8.18.4 exactly (verified against `vips <op>`):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `copy IN OUT [--interpretation …]`      | `copy`        | S1 | EXACT | header-tweak; pixels unchanged |
//! | `cast IN OUT uchar\|ushort\|float`       | `cast`        | S1 | EXACT | core targets only (char/short/int/… → #283/#285) |
//! | `flip IN OUT horizontal\|vertical`       | `flip`        | S1 | EXACT | fliphor/flipver fold |
//! | `rot IN OUT d0\|d90\|d180\|d270`         | `rot`         | S1 | EXACT | right-angle |
//! | `rot45 IN OUT [--angle d45…]`            | `rot45`       | S1 | EXACT | odd-square only |
//! | `byteswap IN OUT`                        | `byteswap`    | S1 | EXACT | 16-bit; compare via `.v` |
//! | `msb IN OUT [--band N]`                  | `msb`         | S1 | EXACT | `--band` >= -1 (all) |
//! | `grid IN OUT TILE-HEIGHT ACROSS DOWN`    | `grid`        | S1 | EXACT | re-tile a tall stack |
//! | `flatten IN OUT [--background "c…"]`     | `flatten`     | S1 | BOUNDED-TOL ≤1 LSB | alpha blend rounding |
//! | `ifthenelse COND IN1 IN2 OUT`            | `ifthenelse`  | S2 (3 in) | EXACT | hard select |
//! | `autorot IN OUT`                         | `autorot`     | S1 | EXACT | oriented input (TIFF/`.v`) |
//! | `wrap IN OUT`                            | `wrap`        | S1 | EXACT | default w/2,h/2 shift |
//! | `gamma IN OUT [--exponent E]`            | `gamma`       | S1 | EXACT | power LUT (core #486 made it vips-exact) |
//! | `falsecolour IN OUT`                     | `falsecolour` | S1 | EXACT | fixed PET LUT |
//! | `addalpha IN OUT`                        | `addalpha`    | S1 | EXACT | append opaque alpha |
//! | `arrayjoin A B [C…] OUT [--across --shim]`| `arrayjoin`  | S2 variadic | EXACT | tile a grid |
//! | `grey OUT WIDTH HEIGHT [--uchar]`        | `grey`        | S5 | BOUNDED-TOL | ramp; float `.v` eps 1e-6 / uchar ≤1 |
//! | `identity OUT [--ushort]`                | `identity`    | S5 | EXACT | 256×1 / 65536×1 LUT |
//! | `switch A B C… OUT`                      | `switch`      | S2 variadic | EXACT | first-non-zero index image |
//!
//! Every handler keeps the §3 `load → try_op → save` shape and calls only the
//! panic-free `try_*` core APIs (or the genuinely-infallible `byteswap` /
//! `identity` constructors, which cannot fail on any input), so a bad input
//! becomes exit 1 rather than an abort (`CLI_CONTRACT.md` §8). Every numeric
//! range conversion (`--band`/`--exponent`) yields a typed exit-1 error, never an
//! `as`-cast panic (the bands B2 lesson). The two variadic commands (`arrayjoin`,
//! `switch`) go through [`io::inputs_and_out`], THE shared S2 idiom; `ifthenelse`
//! takes three FIXED inputs so it uses plain positionals instead.
//
// @doc-command:begin name=copy about="Copy an image, tweaking header fields (interpretation, resolution, offset)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=copy
// @doc-command:begin name=cast about="Cast an image to a new pixel format (uchar, ushort, or float)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=cast
// @doc-command:begin name=flip about="Flip an image horizontally or vertically." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=flip
// @doc-command:begin name=rot about="Rotate an image by a multiple of 90 degrees." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=rot
// @doc-command:begin name=rot45 about="Rotate an odd-sided square image by a multiple of 45 degrees." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=rot45
// @doc-command:begin name=byteswap about="Swap the byte order within every sample." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=byteswap
// @doc-command:begin name=msb about="Pick the most-significant byte of each sample." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=msb
// @doc-command:begin name=grid about="Re-tile a tall stack of tiles into a grid." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=grid
// @doc-command:begin name=flatten about="Flatten an alpha channel out against a background." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=flatten
// @doc-command:begin name=ifthenelse about="Select pixels from two images under a condition image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=ifthenelse
// @doc-command:begin name=autorot about="Auto-rotate an image by its EXIF orientation tag." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=autorot
// @doc-command:begin name=wrap about="Wrap the image origin to its centre, swapping quadrants." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=wrap
// @doc-command:begin name=gamma about="Apply a gamma curve to an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=gamma
// @doc-command:begin name=falsecolour about="Map an image through the libvips PET false-colour scale." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=falsecolour
// @doc-command:begin name=addalpha about="Append a fully-opaque alpha channel to an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=addalpha
// @doc-command:begin name=arrayjoin about="Tile an array of images into a grid." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=arrayjoin
// @doc-command:begin name=grey about="Create a horizontal grey ramp image." \
//     slot-order=apply,save imports-base=save_file
// @doc-command:end name=grey
// @doc-command:begin name=identity about="Create a 1-D identity look-up table image." \
//     slot-order=apply,save imports-base=save_file
// @doc-command:end name=identity
// @doc-command:begin name=switch about="Find the index of the first non-zero condition image at each pixel." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=switch

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::{Angle, Angle45, Interpretation, PixelFormat, Raster};

use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    use OracleClass::{BoundedTol, Exact};
    use Shape::{Creator, ImageToImage, NImageToImage};
    vec![
        meta("copy", ImageToImage, Exact),
        meta("cast", ImageToImage, Exact),
        meta("flip", ImageToImage, Exact),
        meta("rot", ImageToImage, Exact),
        meta("rot45", ImageToImage, Exact),
        meta("byteswap", ImageToImage, Exact),
        meta("msb", ImageToImage, Exact),
        meta("grid", ImageToImage, Exact),
        // BOUNDED-TOL ≤1 LSB: the alpha blend rounds; vips and the core agree to
        // within one least-significant bit (OP_MAP.md conversion section).
        meta("flatten", ImageToImage, BoundedTol),
        // 3 FIXED inputs (cond in1 in2) + out — an S2 shape, but not variadic.
        meta("ifthenelse", NImageToImage, Exact),
        meta("autorot", ImageToImage, Exact),
        meta("wrap", ImageToImage, Exact),
        // EXACT (tol 0): core issue #486 rebuilt the gamma power LUT to be
        // vips-exact, so the full 0..255 domain matches vips 8.18.4 bit-for-bit
        // (previously a ≤1-LSB divergence from independent LUT rounding).
        // Verified against the oracle 2026-07-19.
        meta("gamma", ImageToImage, Exact),
        meta("falsecolour", ImageToImage, Exact),
        meta("addalpha", ImageToImage, Exact),
        meta("arrayjoin", NImageToImage, Exact),
        // BOUNDED-TOL: float ramp path via `.v` eps 1e-6, uchar path ≤1 LSB.
        meta("grey", Creator, BoundedTol),
        meta("identity", Creator, Exact),
        meta("switch", NImageToImage, Exact),
    ]
}

/// Terser [`CommandMeta`] constructor for the [`metas`] table.
const fn meta(name: &'static str, shape: Shape, oracle_class: OracleClass) -> CommandMeta {
    CommandMeta {
        name,
        shape,
        oracle_class,
    }
}

/// The exact vips 8.18.4 `VipsInterpretation` enum spellings accepted by
/// `copy --interpretation` (`vips copy` usage), in vips order.
const INTERPRETATIONS: &[&str] = &[
    "multiband",
    "b-w",
    "histogram",
    "xyz",
    "lab",
    "cmyk",
    "labq",
    "rgb",
    "cmc",
    "lch",
    "labs",
    "srgb",
    "yxy",
    "fourier",
    "rgb16",
    "grey16",
    "matrix",
    "scrgb",
    "hsv",
    "oklab",
    "oklch",
];

/// Map a vips interpretation spelling to the core [`Interpretation`]. The set is
/// pinned by the [`INTERPRETATIONS`] `value_parser`, so an unknown value is a
/// clap usage error (exit 2) before this is reached; the catch-all is defensive.
fn interpretation_from_str(s: &str) -> Result<Interpretation> {
    Ok(match s {
        "multiband" => Interpretation::Multiband,
        "b-w" => Interpretation::Bw,
        "histogram" => Interpretation::Histogram,
        "xyz" => Interpretation::Xyz,
        "lab" => Interpretation::Lab,
        "cmyk" => Interpretation::Cmyk,
        "labq" => Interpretation::Labq,
        "rgb" => Interpretation::Rgb,
        "cmc" => Interpretation::Cmc,
        "lch" => Interpretation::Lch,
        "labs" => Interpretation::Labs,
        "srgb" => Interpretation::Srgb,
        "yxy" => Interpretation::Yxy,
        "fourier" => Interpretation::Fourier,
        "rgb16" => Interpretation::Rgb16,
        "grey16" => Interpretation::Grey16,
        "matrix" => Interpretation::Matrix,
        "scrgb" => Interpretation::ScRgb,
        "hsv" => Interpretation::Hsv,
        "oklab" => Interpretation::OkLab,
        "oklch" => Interpretation::OkLch,
        other => bail!("unknown interpretation {other:?}"),
    })
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every image-loading command carries the shared decode-limit flags via
/// [`io::with_decode_limit_args`]; the two pure creators (`grey`, `identity`) do
/// not decode an input and so omit them (`CLI_CONTRACT.md` §2).
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("copy")
                .about(
                    "Copy an image, tweaking header fields (interpretation, resolution, offset).",
                )
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("interpretation")
                        .long("interpretation")
                        .value_name("SPACE")
                        .value_parser(INTERPRETATIONS.to_vec())
                        .help("Set the colour interpretation tag"),
                )
                .arg(
                    Arg::new("xres")
                        .long("xres")
                        .value_name("PPMM")
                        .value_parser(value_parser!(f64))
                        .help("Horizontal resolution in pixels/mm"),
                )
                .arg(
                    Arg::new("yres")
                        .long("yres")
                        .value_name("PPMM")
                        .value_parser(value_parser!(f64))
                        .help("Vertical resolution in pixels/mm"),
                )
                .arg(
                    Arg::new("xoffset")
                        .long("xoffset")
                        .value_name("PX")
                        .value_parser(value_parser!(i32))
                        .help("Horizontal offset of the image origin"),
                )
                .arg(
                    Arg::new("yoffset")
                        .long("yoffset")
                        .value_name("PX")
                        .value_parser(value_parser!(i32))
                        .help("Vertical offset of the image origin"),
                )
                .arg(
                    // `--orientation` is a DELIBERATE viprs-only extension, NOT a
                    // vips `copy` flag (vips 8.18.4 `copy` exposes only width/
                    // height/bands/format/coding/interpretation/xres/yres/xoffset/
                    // yoffset). It is hidden from the help/parity surface via
                    // `.hide(true)` so it is not counted as vips parity, and exists
                    // solely to mint the `autorot` oriented `.v` fixture (the only
                    // orientation channel the libviprs decoder reads back). The
                    // deviation is recorded in OP_MAP.md and CLI_CONTRACT.md
                    // (adversarial-review conversion finding 4).
                    Arg::new("orientation")
                        .long("orientation")
                        .value_name("N")
                        .hide(true)
                        .value_parser(value_parser!(u8))
                        .help(
                            "viprs-only (hidden, non-vips): set the EXIF-style \
                             orientation tag (1..=8)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("cast")
                .about("Cast an image to a new pixel format (uchar, ushort, or float).")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("FORMAT")
                        .required(true)
                        .value_name("uchar|ushort|float")
                        // Core supports only these three width families (band
                        // count preserved). vips's char/short/int/uint/complex/
                        // double/dpcomplex targets need signed/complex carriers
                        // libviprs lacks (#283/#285) — rejected as a usage error.
                        .value_parser(["uchar", "ushort", "float"])
                        .help(
                            "Target format: uchar (8-bit), ushort (16-bit), or float (f32). \
                             char/short/int/uint/complex/double/dpcomplex are not core-backed \
                             (#283/#285).",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("flip")
                .about("Flip an image horizontally or vertically.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("DIRECTION")
                        .required(true)
                        .value_name("horizontal|vertical")
                        .value_parser(["horizontal", "vertical"])
                        .help("Direction to flip the image"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("rot")
                .about("Rotate an image by a multiple of 90 degrees.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("ANGLE")
                        .required(true)
                        .value_name("d0|d90|d180|d270")
                        .value_parser(["d0", "d90", "d180", "d270"])
                        .help("Angle to rotate clockwise"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("rot45")
                .about("Rotate an odd-sided square image by a multiple of 45 degrees.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (odd-sided square)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("angle")
                        .long("angle")
                        .value_name("d0|d45|…|d315")
                        .default_value("d45")
                        .value_parser(["d0", "d45", "d90", "d135", "d180", "d225", "d270", "d315"])
                        .help("Angle to rotate clockwise (vips default d45)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("byteswap")
                .about("Swap the byte order within every sample.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("msb")
                .about("Pick the most-significant byte of each sample.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("band")
                        .long("band")
                        .value_name("N")
                        .default_value("-1")
                        // vips's own minimum is -1 (= all bands); reject anything
                        // below it rather than silently folding it into "all".
                        .value_parser(value_parser!(i64).range(-1..))
                        .help("Band to take the MSB of (>= -1; -1 = all bands, the default)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("grid")
                .about("Re-tile a tall stack of tiles into a grid.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (a vertical tile stack)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("TILE-HEIGHT")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Height of each tile in pixels (>= 1)"),
                )
                .arg(
                    Arg::new("ACROSS")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Number of tiles across the output grid (>= 1)"),
                )
                .arg(
                    Arg::new("DOWN")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Number of tiles down the output grid (>= 1)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("flatten")
                .about("Flatten an alpha channel out against a background.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image with an alpha band"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("background")
                        .long("background")
                        .value_name("c…")
                        .help(
                            "Background value: a single constant for every band, or one \
                             space-separated value per output band (e.g. \"255 0 0\"). \
                             Default black.",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("ifthenelse")
                .about("Select pixels from two images under a condition image.")
                .arg(
                    Arg::new("COND")
                        .required(true)
                        .help("Condition image (non-zero = pick IN1)"),
                )
                .arg(
                    Arg::new("IN1")
                        .required(true)
                        .help("Source for TRUE (non-zero) pixels"),
                )
                .arg(
                    Arg::new("IN2")
                        .required(true)
                        .help("Source for FALSE (zero) pixels"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("autorot")
                .about("Auto-rotate an image by its EXIF orientation tag.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (orientation from TIFF/.v metadata)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("wrap")
                .about("Wrap the image origin to its centre, swapping quadrants.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image")),
        ),
        io::with_decode_limit_args(
            Command::new("gamma")
                .about("Apply a gamma curve to an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("exponent")
                        .long("exponent")
                        .value_name("E")
                        .value_parser(value_parser!(f64))
                        .help(
                            "Gamma factor (finite, in 1e-6..=1000; vips default \
                             0.416667 = 1/2.4)",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("falsecolour")
                .about("Map an image through the libvips PET false-colour scale.")
                .arg(
                    Arg::new("IN")
                        .required(true)
                        .help("Input image (band 0 is used)"),
                )
                .arg(Arg::new("OUT").required(true).help("Output sRGB image")),
        ),
        io::with_decode_limit_args(
            Command::new("addalpha")
                .about("Append a fully-opaque alpha channel to an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(
                    Arg::new("OUT")
                        .required(true)
                        .help("Output image with an added alpha band"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("arrayjoin")
                .about("Tile an array of images into a grid.")
                .arg(
                    Arg::new("INPUTS")
                        .required(true)
                        .num_args(2..)
                        .value_name("A B [C…] OUT")
                        .help("Two or more input images followed by the output path"),
                )
                .arg(
                    Arg::new("across")
                        .long("across")
                        .value_name("N")
                        .value_parser(value_parser!(u32).range(1..))
                        .help("Number of images across the grid (>= 1; default all in one row)"),
                )
                .arg(
                    Arg::new("shim")
                        .long("shim")
                        .value_name("PX")
                        .default_value("0")
                        .value_parser(value_parser!(u32))
                        .help("Pixels of black gap between cells (default 0)"),
                ),
        ),
        Command::new("grey")
            .about("Create a horizontal grey ramp image.")
            .arg(Arg::new("OUT").required(true).help("Output image"))
            .arg(
                Arg::new("WIDTH")
                    .required(true)
                    .value_parser(value_parser!(u32).range(1..))
                    .help("Image width in pixels (>= 1)"),
            )
            .arg(
                Arg::new("HEIGHT")
                    .required(true)
                    .value_parser(value_parser!(u32).range(1..))
                    .help("Image height in pixels (>= 1)"),
            )
            .arg(
                Arg::new("uchar")
                    .long("uchar")
                    .action(ArgAction::SetTrue)
                    .help("Output an 8-bit Gray8 ramp instead of the default float ramp"),
            ),
        Command::new("identity")
            .about("Create a 1-D identity look-up table image.")
            .arg(Arg::new("OUT").required(true).help("Output image"))
            .arg(
                Arg::new("ushort")
                    .long("ushort")
                    .action(ArgAction::SetTrue)
                    .help("Create a 16-bit (65536×1 Gray16) LUT instead of the 8-bit (256×1) LUT"),
            ),
        io::with_decode_limit_args(
            Command::new("switch")
                .about("Find the index of the first non-zero condition image at each pixel.")
                .arg(
                    Arg::new("INPUTS")
                        .required(true)
                        .num_args(2..)
                        .value_name("A B [C…] OUT")
                        .help(
                            "Two or more single-band condition images followed by the output path",
                        ),
                ),
        ),
    ]
}

/// Dispatch a matched conversion subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "copy" => run_copy(m),
        "cast" => run_cast(m),
        "flip" => run_flip(m),
        "rot" => run_rot(m),
        "rot45" => run_rot45(m),
        "byteswap" => run_byteswap(m),
        "msb" => run_msb(m),
        "grid" => run_grid(m),
        "flatten" => run_flatten(m),
        "ifthenelse" => run_ifthenelse(m),
        "autorot" => run_autorot(m),
        "wrap" => run_wrap(m),
        "gamma" => run_gamma(m),
        "falsecolour" => run_falsecolour(m),
        "addalpha" => run_addalpha(m),
        "arrayjoin" => run_arrayjoin(m),
        "grey" => run_grey(m),
        "identity" => run_identity(m),
        "switch" => run_switch(m),
        other => bail!("conversion family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Parse a vips-style space-separated numeric vector (e.g. `"255 0 0"`).
fn parse_f64_vec(s: &str) -> Result<Vec<f64>> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| {
            t.parse::<f64>()
                .map_err(|e| anyhow!("value {t:?} is not a number: {e}"))
        })
        .collect::<Result<_>>()?;
    if v.is_empty() {
        bail!("expected at least one value (a space-separated vector like \"255 0 0\")");
    }
    Ok(v)
}

/// vips clamps `gamma`'s exponent to `[1e-6, 1000]` and REJECTS anything outside
/// it (a GObject property range). Mirror that surface: an out-of-range or
/// non-finite exponent is a typed usage error (exit 1), never silently accepted
/// (adversarial-review conversion finding 6).
fn check_gamma_exponent(e: f64) -> Result<()> {
    if !e.is_finite() || !(1e-6..=1000.0).contains(&e) {
        bail!("--exponent {e} out of range (vips accepts a finite value in 1e-6..=1000)");
    }
    Ok(())
}

/// `copy IN OUT [--interpretation … --xres … --xoffset … --orientation …]` — S1
/// header-tweak; the pixels are copied verbatim and only metadata changes.
fn run_copy(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=copy slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=copy slot=load

    // @doc-snippet:begin command=copy slot=apply
    let mut builder = raster.copy();
    if let Some(s) = m.get_one::<String>("interpretation") {
        builder = builder.interpretation(interpretation_from_str(s)?);
    }
    if let Some(&v) = m.get_one::<f64>("xres") {
        builder = builder.xres(v);
    }
    if let Some(&v) = m.get_one::<f64>("yres") {
        builder = builder.yres(v);
    }
    if let Some(&v) = m.get_one::<i32>("xoffset") {
        builder = builder.xoffset(v);
    }
    if let Some(&v) = m.get_one::<i32>("yoffset") {
        builder = builder.yoffset(v);
    }
    if let Some(&v) = m.get_one::<u8>("orientation") {
        builder = builder.orientation(v);
    }
    let out = builder.build();
    // @doc-snippet:end command=copy slot=apply

    // @doc-snippet:begin command=copy slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=copy slot=save
    Ok(())
}

/// `cast IN OUT uchar|ushort|float` — S1; reinterpret the samples at a new depth.
fn run_cast(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let fmt_name = pos(m, "FORMAT");

    // @doc-snippet:begin command=cast slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=cast slot=load

    // @doc-snippet:begin command=cast slot=apply
    // The target keeps the input's band count (try_cast rejects a band-count
    // change); only the per-channel byte width changes.
    let channels = raster.format().channels();
    let bytes = match fmt_name {
        "uchar" => 1,
        "ushort" => 2,
        "float" => 4,
        other => bail!("unsupported cast format {other:?} (expected uchar|ushort|float)"),
    };
    let target = PixelFormat::with_channels(channels, bytes)
        .ok_or_else(|| anyhow!("no {fmt_name} format exists for a {channels}-band image"))?;
    let out = raster.try_cast(target)?;
    // @doc-snippet:end command=cast slot=apply

    // @doc-snippet:begin command=cast slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=cast slot=save
    Ok(())
}

/// `flip IN OUT horizontal|vertical` — S1 enum.
fn run_flip(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let dir = pos(m, "DIRECTION");

    // @doc-snippet:begin command=flip slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=flip slot=load

    // @doc-snippet:begin command=flip slot=apply
    let out = match dir {
        "horizontal" => raster.try_fliphor()?,
        "vertical" => raster.try_flipver()?,
        other => bail!("unknown flip direction {other:?} (expected horizontal|vertical)"),
    };
    // @doc-snippet:end command=flip slot=apply

    // @doc-snippet:begin command=flip slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=flip slot=save
    Ok(())
}

/// `rot IN OUT d0|d90|d180|d270` — S1 right-angle rotation.
fn run_rot(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let angle = match pos(m, "ANGLE") {
        "d0" => Angle::D0,
        "d90" => Angle::D90,
        "d180" => Angle::D180,
        "d270" => Angle::D270,
        other => bail!("unknown rotation angle {other:?} (expected d0|d90|d180|d270)"),
    };

    // @doc-snippet:begin command=rot slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=rot slot=load

    // @doc-snippet:begin command=rot slot=apply
    let out = raster.try_rot(angle)?;
    // @doc-snippet:end command=rot slot=apply

    // @doc-snippet:begin command=rot slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=rot slot=save
    Ok(())
}

/// `rot45 IN OUT [--angle d45…]` — S1; odd-sided square only (core enforces).
fn run_rot45(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let angle = match pos(m, "angle") {
        "d0" => Angle45::D0,
        "d45" => Angle45::D45,
        "d90" => Angle45::D90,
        "d135" => Angle45::D135,
        "d180" => Angle45::D180,
        "d225" => Angle45::D225,
        "d270" => Angle45::D270,
        "d315" => Angle45::D315,
        other => bail!("unknown rot45 angle {other:?}"),
    };

    // @doc-snippet:begin command=rot45 slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=rot45 slot=load

    // @doc-snippet:begin command=rot45 slot=apply
    let out = raster.try_rot45(angle)?;
    // @doc-snippet:end command=rot45 slot=apply

    // @doc-snippet:begin command=rot45 slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=rot45 slot=save
    Ok(())
}

/// `byteswap IN OUT` — S1; swap the byte order within every sample.
fn run_byteswap(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=byteswap slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=byteswap slot=load

    // @doc-snippet:begin command=byteswap slot=apply
    // `byteswap` is infallible (it can never fail on any input): a straight
    // per-sample byte reversal, so there is no `try_byteswap` to call.
    let out = raster.byteswap();
    // @doc-snippet:end command=byteswap slot=apply

    // @doc-snippet:begin command=byteswap slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=byteswap slot=save
    Ok(())
}

/// `msb IN OUT [--band N]` — S1; take the most-significant byte of each sample.
fn run_msb(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    // vips default band is -1 (= all bands). With the `-1..` value_parser the
    // only negative clap admits is -1, which maps to the core's `None`. A
    // positive band beyond `u32::MAX` is a typed exit-1 error — NOT a
    // `u32::try_from` panic/abort (exit 101), per CLI_CONTRACT.md §8.
    let band_raw = *m.get_one::<i64>("band").expect("clap default -1");
    let band: Option<u32> = if band_raw < 0 {
        None
    } else {
        Some(
            u32::try_from(band_raw)
                .map_err(|_| anyhow!("--band {band_raw} out of range (max {})", u32::MAX))?,
        )
    };

    // @doc-snippet:begin command=msb slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=msb slot=load

    // @doc-snippet:begin command=msb slot=apply
    let out = raster.try_msb(band)?;
    // @doc-snippet:end command=msb slot=apply

    // @doc-snippet:begin command=msb slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=msb slot=save
    Ok(())
}

/// `grid IN OUT TILE-HEIGHT ACROSS DOWN` — S1; re-tile a vertical stack.
fn run_grid(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let tile_height = *m.get_one::<u32>("TILE-HEIGHT").expect("required");
    let across = *m.get_one::<u32>("ACROSS").expect("required");
    let down = *m.get_one::<u32>("DOWN").expect("required");

    // @doc-snippet:begin command=grid slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=grid slot=load

    // @doc-snippet:begin command=grid slot=apply
    let out = raster.try_grid(tile_height, across, down)?;
    // @doc-snippet:end command=grid slot=apply

    // @doc-snippet:begin command=grid slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=grid slot=save
    Ok(())
}

/// `flatten IN OUT [--background "c…"]` — S1; blend alpha against a background.
fn run_flatten(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let background = match m.get_one::<String>("background") {
        Some(s) => Some(parse_f64_vec(s)?),
        None => None,
    };

    // @doc-snippet:begin command=flatten slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=flatten slot=load

    // @doc-snippet:begin command=flatten slot=apply
    let out = raster.try_flatten(background.as_deref())?;
    // @doc-snippet:end command=flatten slot=apply

    // @doc-snippet:begin command=flatten slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=flatten slot=save
    Ok(())
}

/// `ifthenelse COND IN1 IN2 OUT` — S2 with three FIXED inputs; hard select.
fn run_ifthenelse(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let cond_path = PathBuf::from(pos(m, "COND"));
    let in1_path = PathBuf::from(pos(m, "IN1"));
    let in2_path = PathBuf::from(pos(m, "IN2"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=ifthenelse slot=load imports=decode_file
    let cond = io::load(&cond_path, &limits)?;
    let then = io::load(&in1_path, &limits)?;
    let otherwise = io::load(&in2_path, &limits)?;
    // @doc-snippet:end command=ifthenelse slot=load

    // @doc-snippet:begin command=ifthenelse slot=apply
    let out = cond.try_ifthenelse(&then, &otherwise)?;
    // @doc-snippet:end command=ifthenelse slot=apply

    // @doc-snippet:begin command=ifthenelse slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=ifthenelse slot=save
    Ok(())
}

/// `autorot IN OUT` — S1; rotate by the EXIF orientation tag, then clear it.
fn run_autorot(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=autorot slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=autorot slot=load

    // @doc-snippet:begin command=autorot slot=apply
    let out = raster.try_autorot()?;
    // @doc-snippet:end command=autorot slot=apply

    // @doc-snippet:begin command=autorot slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=autorot slot=save
    Ok(())
}

/// `wrap IN OUT` — S1; toroidally shift the origin to the centre.
fn run_wrap(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=wrap slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=wrap slot=load

    // @doc-snippet:begin command=wrap slot=apply
    let out = raster.try_wrap()?;
    // @doc-snippet:end command=wrap slot=apply

    // @doc-snippet:begin command=wrap slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=wrap slot=save
    Ok(())
}

/// `gamma IN OUT [--exponent E]` — S1 power curve.
fn run_gamma(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    // A missing flag maps to the core default (1/2.4). vips clamps gamma's
    // exponent to [1e-6, 1000] and REJECTS anything outside that (a GObject
    // property range); mirror that surface exactly so an out-of-range or
    // non-finite exponent is a typed usage error (exit 1) rather than silently
    // accepted (adversarial-review conversion finding 6). The core independently
    // rejects the non-positive / non-finite end too, but catching it here keeps
    // the whole admissible range in lock-step with vips.
    let exponent = m.get_one::<f64>("exponent").copied();
    exponent.map(check_gamma_exponent).transpose()?;

    // @doc-snippet:begin command=gamma slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=gamma slot=load

    // @doc-snippet:begin command=gamma slot=apply
    let out = raster.try_gamma(exponent)?;
    // @doc-snippet:end command=gamma slot=apply

    // @doc-snippet:begin command=gamma slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=gamma slot=save
    Ok(())
}

/// `falsecolour IN OUT` — S1; map band 0 through the PET false-colour LUT.
fn run_falsecolour(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=falsecolour slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=falsecolour slot=load

    // @doc-snippet:begin command=falsecolour slot=apply
    let out = raster.try_falsecolour()?;
    // @doc-snippet:end command=falsecolour slot=apply

    // @doc-snippet:begin command=falsecolour slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=falsecolour slot=save
    Ok(())
}

/// `addalpha IN OUT` — S1; append a fully-opaque alpha band.
fn run_addalpha(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=addalpha slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=addalpha slot=load

    // @doc-snippet:begin command=addalpha slot=apply
    let out = raster.try_addalpha()?;
    // @doc-snippet:end command=addalpha slot=apply

    // @doc-snippet:begin command=addalpha slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=addalpha slot=save
    Ok(())
}

/// `arrayjoin A B [C…] OUT [--across N --shim N]` — S2 variadic; tile a grid.
fn run_arrayjoin(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let (inputs, out_path) = io::inputs_and_out(m, "INPUTS")?;
    let across = m.get_one::<u32>("across").copied();
    let shim = m.get_one::<u32>("shim").copied();

    // @doc-snippet:begin command=arrayjoin slot=load imports=decode_file
    let rasters: Vec<Raster> = inputs
        .iter()
        .map(|p| io::load(p, &limits))
        .collect::<Result<_>>()?;
    // @doc-snippet:end command=arrayjoin slot=load

    // @doc-snippet:begin command=arrayjoin slot=apply
    let refs: Vec<&Raster> = rasters.iter().collect();
    let out = Raster::try_arrayjoin(&refs, across, shim)?;
    // @doc-snippet:end command=arrayjoin slot=apply

    // @doc-snippet:begin command=arrayjoin slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=arrayjoin slot=save
    Ok(())
}

/// `grey OUT WIDTH HEIGHT [--uchar]` — S5 creator.
fn run_grey(m: &ArgMatches) -> Result<()> {
    let out_path = PathBuf::from(pos(m, "OUT"));
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");
    let uchar = m.get_flag("uchar");

    // @doc-snippet:begin command=grey slot=apply
    let out = Raster::try_grey(width, height, uchar)?;
    // @doc-snippet:end command=grey slot=apply

    // @doc-snippet:begin command=grey slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=grey slot=save
    Ok(())
}

/// `identity OUT [--ushort]` — S5 creator; the 8- or 16-bit identity LUT.
fn run_identity(m: &ArgMatches) -> Result<()> {
    let out_path = PathBuf::from(pos(m, "OUT"));
    let ushort = m.get_flag("ushort");

    // @doc-snippet:begin command=identity slot=apply
    // Both constructors are infallible (fixed-size LUTs), so there is no
    // `try_identity` to call.
    let out = if ushort {
        Raster::identity_ushort()
    } else {
        Raster::identity()
    };
    // @doc-snippet:end command=identity slot=apply

    // @doc-snippet:begin command=identity slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=identity slot=save
    Ok(())
}

/// `switch A B C… OUT` — S2 variadic; first-non-zero index image.
fn run_switch(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let (inputs, out_path) = io::inputs_and_out(m, "INPUTS")?;

    // @doc-snippet:begin command=switch slot=load imports=decode_file
    let rasters: Vec<Raster> = inputs
        .iter()
        .map(|p| io::load(p, &limits))
        .collect::<Result<_>>()?;
    // @doc-snippet:end command=switch slot=load

    // @doc-snippet:begin command=switch slot=apply
    let refs: Vec<&Raster> = rasters.iter().collect();
    let out = Raster::try_switch(&refs)?;
    // @doc-snippet:end command=switch slot=apply

    // @doc-snippet:begin command=switch slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=switch slot=save
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
            19,
            "the conversion family has nineteen commands"
        );
    }

    #[test]
    fn cast_enforces_the_core_format_set() {
        // uchar/ushort/float are accepted; vips's non-core targets are rejected
        // at parse time (usage error, exit 2) — no invented signed/complex parity.
        assert!(
            cmd("cast")
                .clone()
                .try_get_matches_from(["cast", "in.png", "out.png", "ushort"])
                .is_ok()
        );
        for bad in [
            "char",
            "short",
            "int",
            "uint",
            "complex",
            "double",
            "dpcomplex",
            "notset",
        ] {
            assert!(
                cmd("cast")
                    .clone()
                    .try_get_matches_from(["cast", "in.png", "out.png", bad])
                    .is_err(),
                "cast target {bad:?} must be rejected (not core-backed)"
            );
        }
    }

    #[test]
    fn flip_and_rot_enforce_their_enums() {
        assert!(
            cmd("flip")
                .clone()
                .try_get_matches_from(["flip", "in.png", "out.png", "diagonal"])
                .is_err(),
            "only horizontal|vertical are valid flip directions"
        );
        assert!(
            cmd("rot")
                .clone()
                .try_get_matches_from(["rot", "in.png", "out.png", "d45"])
                .is_err(),
            "d45 is not a right-angle rotation (rot45 owns it)"
        );
        for good in ["d0", "d90", "d180", "d270"] {
            assert!(
                cmd("rot")
                    .clone()
                    .try_get_matches_from(["rot", "in.png", "out.png", good])
                    .is_ok(),
                "rot must accept {good}"
            );
        }
    }

    #[test]
    fn rot45_angle_defaults_to_d45() {
        let m = cmd("rot45")
            .try_get_matches_from(["rot45", "in.png", "out.png"])
            .unwrap();
        assert_eq!(pos(&m, "angle"), "d45");
    }

    #[test]
    fn msb_band_defaults_to_all_and_rejects_below_minus_one() {
        let m = cmd("msb")
            .clone()
            .try_get_matches_from(["msb", "in.png", "out.png"])
            .unwrap();
        assert_eq!(*m.get_one::<i64>("band").unwrap(), -1);
        assert!(
            cmd("msb")
                .try_get_matches_from(["msb", "in.png", "out.png", "--band", "-2"])
                .is_err(),
            "a --band below -1 must be rejected (vips minimum is -1)"
        );
    }

    #[test]
    fn msb_huge_band_is_error_not_panic() {
        // A positive --band beyond u32::MAX parses (the `-1..` bound only floors)
        // but must convert to a typed exit-1 error, NEVER a u32::try_from panic
        // (exit 101) — CLI_CONTRACT.md §8. Range-checked before any load, so a
        // dummy path is fine.
        let m = cmd("msb")
            .try_get_matches_from(["msb", "in.png", "out.png", "--band", "4294967296"])
            .unwrap();
        let err = run_msb(&m).unwrap_err();
        assert!(
            err.to_string().contains("out of range"),
            "expected a typed 'out of range' error, got: {err}"
        );
    }

    #[test]
    fn gamma_exponent_out_of_range_is_typed_error_not_silent() {
        // vips REJECTS a gamma exponent outside [1e-6, 1000] (and non-finite);
        // viprs must match with a typed exit-1 error rather than silently
        // accepting it (adversarial-review conversion finding 6). The range is
        // checked before any load, so a dummy path is fine.
        // The `--exponent=<v>` form lets a negative value through clap (a bare
        // `-1` would look like a flag); the range guard is what must reject it.
        for bad in ["5000", "1001", "0", "-1", "-0.5"] {
            let m = cmd("gamma")
                .clone()
                .try_get_matches_from(["gamma", "in.png", "out.png", &format!("--exponent={bad}")])
                .unwrap();
            let err = run_gamma(&m).unwrap_err();
            assert!(
                err.to_string().contains("out of range"),
                "gamma --exponent {bad} must be a typed range error, got: {err}"
            );
        }
        // In-range values (incl. the vips endpoints) pass the guard and fail
        // later on the missing input path — NOT on the range check.
        for good in ["0.000001", "1000", "2.0"] {
            let m = cmd("gamma")
                .clone()
                .try_get_matches_from(["gamma", "in.png", "out.png", "--exponent", good])
                .unwrap();
            let err = run_gamma(&m).unwrap_err();
            assert!(
                !err.to_string().contains("out of range"),
                "gamma --exponent {good} is in range; must not be a range error, got: {err}"
            );
        }
    }

    #[test]
    fn grid_rejects_zero_dimensions() {
        // vips minima are 1 for tile-height/across/down.
        assert!(
            cmd("grid")
                .try_get_matches_from(["grid", "in.png", "out.png", "0", "2", "2"])
                .is_err(),
            "a zero tile-height must be rejected"
        );
    }

    #[test]
    fn grid_parses_positionals_in_vips_order() {
        let m = cmd("grid")
            .try_get_matches_from(["grid", "in.png", "out.png", "4", "2", "3"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(*m.get_one::<u32>("TILE-HEIGHT").unwrap(), 4);
        assert_eq!(*m.get_one::<u32>("ACROSS").unwrap(), 2);
        assert_eq!(*m.get_one::<u32>("DOWN").unwrap(), 3);
    }

    #[test]
    fn ifthenelse_takes_three_fixed_inputs() {
        let m = cmd("ifthenelse")
            .try_get_matches_from(["ifthenelse", "c.png", "a.png", "b.png", "out.png"])
            .unwrap();
        assert_eq!(pos(&m, "COND"), "c.png");
        assert_eq!(pos(&m, "IN1"), "a.png");
        assert_eq!(pos(&m, "IN2"), "b.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
    }

    #[test]
    fn arrayjoin_and_switch_split_variadic_into_inputs_and_out() {
        let m = cmd("arrayjoin")
            .try_get_matches_from(["arrayjoin", "a.png", "b.png", "c.png", "out.png"])
            .unwrap();
        let (inputs, out) = io::inputs_and_out(&m, "INPUTS").unwrap();
        assert_eq!(inputs.len(), 3);
        assert_eq!(out, PathBuf::from("out.png"));

        // arrayjoin/switch need at least two positionals (one input + the out).
        assert!(
            cmd("switch")
                .try_get_matches_from(["switch", "only.png"])
                .is_err(),
            "num_args(2..) must reject a single positional"
        );
    }

    #[test]
    fn arrayjoin_shim_defaults_to_zero_and_across_is_optional() {
        let m = cmd("arrayjoin")
            .try_get_matches_from(["arrayjoin", "a.png", "b.png", "out.png"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("shim").unwrap(), 0);
        assert_eq!(m.get_one::<u32>("across"), None);
    }

    #[test]
    fn grey_creator_rejects_zero_dimensions() {
        assert!(
            cmd("grey")
                .try_get_matches_from(["grey", "out.v", "0", "16"])
                .is_err(),
            "grey width must be >= 1 (vips minimum)"
        );
    }

    #[test]
    fn grey_uchar_flag_parses() {
        let m = cmd("grey")
            .clone()
            .try_get_matches_from(["grey", "out.png", "16", "16", "--uchar"])
            .unwrap();
        assert!(m.get_flag("uchar"));
        let m = cmd("grey")
            .try_get_matches_from(["grey", "out.v", "16", "16"])
            .unwrap();
        assert!(!m.get_flag("uchar"));
    }

    #[test]
    fn identity_ushort_flag_parses() {
        let m = cmd("identity")
            .clone()
            .try_get_matches_from(["identity", "out.png", "--ushort"])
            .unwrap();
        assert!(m.get_flag("ushort"));
    }

    #[test]
    fn interpretation_maps_every_vips_spelling() {
        // Every value the copy value_parser admits must map to a core variant.
        for name in INTERPRETATIONS {
            assert!(
                interpretation_from_str(name).is_ok(),
                "interpretation {name:?} must map to a core Interpretation"
            );
        }
        assert!(interpretation_from_str("nonsense").is_err());
    }

    #[test]
    fn copy_accepts_header_flags() {
        let m = cmd("copy")
            .try_get_matches_from([
                "copy",
                "in.png",
                "out.png",
                "--interpretation",
                "srgb",
                "--xoffset",
                "3",
            ])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(m.get_one::<String>("interpretation").unwrap(), "srgb");
        assert_eq!(*m.get_one::<i32>("xoffset").unwrap(), 3);
    }

    #[test]
    fn parse_f64_vec_reads_a_space_separated_vector() {
        assert_eq!(parse_f64_vec("255 0 0").unwrap(), vec![255.0, 0.0, 0.0]);
        assert!(parse_f64_vec("   ").is_err());
        assert!(parse_f64_vec("255 x").is_err());
    }
}
