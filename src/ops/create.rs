//! Create (image-generator) op family — a Wave-2 per-family lane
//! (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md` create section).
//!
//! The core `src/create.rs` exposes 56 `pub fn` that fold to 29 base ops; those
//! become **23** `viprs` subcommands here (the `*_seeded` noise twins collapse
//! into a `--seed` flag, `black_bands` into `--bands`, and `new_from_image` /
//! `from_matrix` are EXCLUDED helpers, exactly as `OP_MAP.md` prescribes). Every
//! creator is §3 shape **S5** (OUT first, no image input) except `buildlut`,
//! which is **S1** (its input is a matrix `.mat` file read through the shared
//! [`MatFile`](super::matfile::MatFile) loader).
//!
//! Oracle classes are MIXED per `OP_MAP.md` (`CLI_CONTRACT.md` §5):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `black OUT W H [--bands]`           | `black`      | S5 | EXACT | all-zero uchar |
//! | `xyz OUT W H`                        | `xyz`        | S5 | EXACT | coord ramp (vips uint → float `.v`, tol 0) |
//! | `eye OUT W H [--uchar]`              | `eye`        | S5 | BOUNDED-TOL | trig; f32 |
//! | `zone OUT W H`                       | `zone`       | S5 | BOUNDED-TOL | trig; float only in core |
//! | `sines OUT W H`                      | `sines`      | S5 | BOUNDED-TOL | trig; defaults only |
//! | `gaussnoise OUT W H [--sigma --mean --seed]` | `gaussnoise` | S5 | GOLDEN-ONLY | PRNG; viprs pin |
//! | `perlin OUT W H [--seed]`            | `perlin`     | S5 | GOLDEN-ONLY | PRNG; viprs pin |
//! | `worley OUT W H [--seed]`            | `worley`     | S5 | GOLDEN-ONLY | PRNG; viprs pin |
//! | `fractsurf OUT W H FRACTAL_DIM`      | `fractsurf`  | S5 | GOLDEN-ONLY | gaussnoise-derived; viprs pin |
//! | `buildlut IN.mat OUT`               | `buildlut`   | S1 | BOUNDED-TOL | matrix-file LUT (vips double → float `.v`) |
//! | `tonelut OUT`                        | `tonelut`    | S5 | BOUNDED-TOL | Gray16 tone curve; defaults only |
//! | `mask_ideal OUT W H FC [--nodc]`    | `mask_ideal` | S5 | BOUNDED-TOL | Fourier mask |
//! | `mask_ideal_ring OUT W H FC RW [--nodc]` | `mask_ideal_ring` | S5 | BOUNDED-TOL | |
//! | `mask_ideal_band OUT W H FCX FCY R` | `mask_ideal_band` | S5 | BOUNDED-TOL | no `--nodc` in core |
//! | `mask_gaussian OUT W H FC AC [--nodc]` | `mask_gaussian` | S5 | BOUNDED-TOL | exp() |
//! | `mask_gaussian_ring OUT W H FC AC RW [--nodc]` | `mask_gaussian_ring` | S5 | BOUNDED-TOL | |
//! | `mask_gaussian_band OUT W H FCX FCY R AC` | `mask_gaussian_band` | S5 | BOUNDED-TOL | no `--nodc` in core |
//! | `mask_butterworth OUT W H ORD FC AC [--nodc --optical --uchar]` | `mask_butterworth` | S5 | BOUNDED-TOL | |
//! | `mask_butterworth_ring OUT W H ORD FC AC RW [--nodc]` | `mask_butterworth_ring` | S5 | BOUNDED-TOL | core has only `--nodc` here |
//! | `mask_butterworth_band OUT W H ORD FCX FCY R AC [--uchar --optical --nodc]` | `mask_butterworth_band` | S5 | BOUNDED-TOL | |
//! | `mask_fractal OUT W H FRACTAL_DIM`  | `mask_fractal` | S5 | BOUNDED-TOL | pow() |
//! | `sdf OUT W H SHAPE [--a --b --r --corners]` | `sdf` | S5 | BOUNDED-TOL | f32 hypotf |
//! | `text OUT STRING [--dpi --width --height --wrap]` | `text` | S5 | GOLDEN-ONLY | Pango differs; viprs pin |
//!
//! Positional orders, flag names, enum spellings, and value minima mirror vips
//! 8.18.4 exactly (verified against `vips <op>`), restricted to what the core
//! `try_*` API actually supports — never an invented parity flag:
//!
//! * vips `mask_*` all carry `--uchar --optical --reject`; the core exposes only
//!   the subset each op's `try_*` signature actually takes (e.g. `mask_ideal`
//!   only `--nodc`; `mask_ideal_band` / `mask_gaussian_band` none), and `reject`
//!   is never in core, so it is never surfaced.
//! * `mask_butterworth` DOES take `--nodc --optical --uchar` in core (its
//!   `try_mask_butterworth` has all three bools), so all three are surfaced —
//!   this is the honest "expose exactly what core has" surface, wider than the
//!   terse `OP_MAP.md` row (which lists only `--nodc`) but consistent with the
//!   `mask_butterworth_band` row and the core signature. `mask_butterworth_ring`
//!   takes only `--nodc` (its `try_*` has one bool), so only that is surfaced.
//! * vips `xyz` `--csize/--dsize/--esize`, `eye`/`sines`/`perlin` `--factor` /
//!   `--hfreq` / `--vfreq` / `--cell-size` / `--uchar`, and `tonelut`'s ten tone
//!   parameters are NOT in core (defaults only) and are not surfaced.
//! * vips `text` `--font` is NOT surfaced: the core `try_text` has no font
//!   parameter (it renders with a bundled face — glyph-exact Pango parity is out
//!   of scope), so a `--font` flag could not be honoured.
//!
//! Handlers call only the panic-free `try_*` core APIs and route every numeric
//! range through a typed exit-1 error (never an `unwrap`/`as`-cast abort,
//! `CLI_CONTRACT.md` §8). Float creators write `.v`; the `--uchar` variants can
//! write `.png`.
//
// @doc-command:begin name=black about="Make an all-zero uchar image." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=black
// @doc-command:begin name=xyz about="Make an image whose pixels are their own coordinates." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=xyz
// @doc-command:begin name=eye about="Make an image of the eye's spatial-frequency response." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=eye
// @doc-command:begin name=zone about="Make a zone-plate test pattern." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=zone
// @doc-command:begin name=sines about="Make a 2D sine grating." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=sines
// @doc-command:begin name=gaussnoise about="Make a Gaussian-noise image (seeded PRNG)." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=gaussnoise
// @doc-command:begin name=perlin about="Make a Perlin-noise image (seeded PRNG)." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=perlin
// @doc-command:begin name=worley about="Make a Worley-noise image (seeded PRNG)." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=worley
// @doc-command:begin name=fractsurf about="Make a fractal surface." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=fractsurf
// @doc-command:begin name=buildlut about="Build a look-up table from a control-point matrix file." \
//     slot-order=load,create,save imports-base=decode_file,save_file
// @doc-command:end name=buildlut
// @doc-command:begin name=tonelut about="Build a print tone-mapping look-up table." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=tonelut
// @doc-command:begin name=mask_ideal about="Make an ideal frequency-domain filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_ideal
// @doc-command:begin name=mask_ideal_ring about="Make an ideal ring filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_ideal_ring
// @doc-command:begin name=mask_ideal_band about="Make an ideal band filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_ideal_band
// @doc-command:begin name=mask_gaussian about="Make a Gaussian frequency-domain filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_gaussian
// @doc-command:begin name=mask_gaussian_ring about="Make a Gaussian ring filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_gaussian_ring
// @doc-command:begin name=mask_gaussian_band about="Make a Gaussian band filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_gaussian_band
// @doc-command:begin name=mask_butterworth about="Make a Butterworth frequency-domain filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_butterworth
// @doc-command:begin name=mask_butterworth_ring about="Make a Butterworth ring filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_butterworth_ring
// @doc-command:begin name=mask_butterworth_band about="Make a Butterworth band filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_butterworth_band
// @doc-command:begin name=mask_fractal about="Make a fractal power-spectrum filter mask." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=mask_fractal
// @doc-command:begin name=sdf about="Make a signed distance field for a shape." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=sdf
// @doc-command:begin name=text about="Render text to a coverage image." \
//     slot-order=create,save imports-base=save_file
// @doc-command:end name=text

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::{Raster, SdfParams};

use super::matfile::MatFile;
use super::{CommandMeta, OracleClass, Shape, io};

/// vips's declared maximum image dimension (`VIPS_MAX_COORD`, verified against
/// vips 8.18.4: `black out 100000000 1` succeeds, `100000001` is refused).
/// Typed `i64` because clap's ranged value-parser bounds are `RangeBounds<i64>`.
const VIPS_MAX_COORD: i64 = 100_000_000;

/// vips's declared upper bound for the mask `order` / frequency-cutoff /
/// ringwidth / radius `gdouble` properties (verified against vips 8.18.4: a
/// value of `1000000` is accepted, `1000001` is refused as out-of-range).
const VIPS_MASK_PARAM_MAX: f64 = 1_000_000.0;

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    use OracleClass::{BoundedTol, Exact, GoldenOnly};
    use Shape::{Creator, ImageToImage};
    let creator = |name, oracle| CommandMeta {
        name,
        shape: Creator,
        oracle_class: oracle,
    };
    vec![
        creator("black", Exact),
        creator("xyz", Exact),
        creator("eye", BoundedTol),
        creator("zone", BoundedTol),
        creator("sines", BoundedTol),
        creator("gaussnoise", GoldenOnly),
        creator("perlin", GoldenOnly),
        creator("worley", GoldenOnly),
        creator("fractsurf", GoldenOnly),
        // buildlut's input is a matrix FILE, so it is S1 (image→image), not a
        // creator (OP_MAP.md create section).
        CommandMeta {
            name: "buildlut",
            shape: ImageToImage,
            oracle_class: BoundedTol,
        },
        creator("tonelut", BoundedTol),
        creator("mask_ideal", BoundedTol),
        creator("mask_ideal_ring", BoundedTol),
        creator("mask_ideal_band", BoundedTol),
        creator("mask_gaussian", BoundedTol),
        creator("mask_gaussian_ring", BoundedTol),
        creator("mask_gaussian_band", BoundedTol),
        creator("mask_butterworth", BoundedTol),
        creator("mask_butterworth_ring", BoundedTol),
        creator("mask_butterworth_band", BoundedTol),
        creator("mask_fractal", BoundedTol),
        creator("sdf", BoundedTol),
        creator("text", GoldenOnly),
    ]
}

// ---------------------------------------------------------------------------
// Argument builders (shared shapes across the 23 commands).
// ---------------------------------------------------------------------------

/// The leading `OUT` positional every creator shares.
fn out_arg() -> Arg {
    Arg::new("OUT").required(true).help("Output image path")
}

/// A required `WIDTH` / `HEIGHT` pixel dimension (vips range 1..=1e8, i.e.
/// `VIPS_MAX_COORD`; a value above vips's declared maximum is rejected by clap
/// exactly as vips refuses to set the out-of-range gint property).
fn dim_arg(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .required(true)
        .value_parser(value_parser!(u32).range(1..=VIPS_MAX_COORD))
        .help(help)
}

/// A required positional `f64` argument.
fn f64_pos(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .required(true)
        .value_parser(value_parser!(f64))
        .help(help)
}

/// An optional boolean flag (vips `--nodc` / `--optical` / `--uchar`).
fn bool_flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .help(help)
}

/// The `--nodc` flag shared by every mask op whose core `try_*` takes it.
fn nodc_flag() -> Arg {
    bool_flag("nodc", "Remove the DC component (vips --nodc)")
}

/// A creator command taking `OUT WIDTH HEIGHT` and nothing else.
fn wh_creator(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .arg(out_arg())
        .arg(dim_arg("WIDTH", "Image width in pixels (>= 1)"))
        .arg(dim_arg("HEIGHT", "Image height in pixels (>= 1)"))
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Creators register no image input, so — unlike the image→image families —
/// they do NOT carry the shared decode-limit flags. `buildlut` reads a `.mat`
/// matrix file (not an image) through its own capped loader, so it too skips
/// the decode-limit surface.
pub fn commands() -> Vec<Command> {
    vec![
        // --- basic generators ------------------------------------------------
        wh_creator("black", "Make an all-zero uchar image.").arg(
            Arg::new("bands")
                .long("bands")
                .value_name("N")
                .default_value("1")
                .value_parser(value_parser!(u32).range(1..))
                .help("Number of bands (vips --bands, default 1)"),
        ),
        wh_creator(
            "xyz",
            "Make an image whose pixels are their own coordinates.",
        ),
        wh_creator(
            "eye",
            "Make an image of the eye's spatial-frequency response.",
        )
        .arg(bool_flag(
            "uchar",
            "Output an unsigned char image (vips --uchar)",
        )),
        wh_creator("zone", "Make a zone-plate test pattern."),
        wh_creator("sines", "Make a 2D sine grating."),
        // --- noise generators (GOLDEN-ONLY) ---------------------------------
        wh_creator("gaussnoise", "Make a Gaussian-noise image (seeded PRNG).")
            .arg(
                Arg::new("sigma")
                    .long("sigma")
                    .value_name("S")
                    .default_value("30")
                    .value_parser(value_parser!(f64))
                    .help("Standard deviation of the noise (vips --sigma, default 30, >= 0)"),
            )
            .arg(
                Arg::new("mean")
                    .long("mean")
                    .value_name("M")
                    .default_value("128")
                    .value_parser(value_parser!(f64))
                    .help("Mean of the noise (vips --mean, default 128)"),
            )
            .arg(seed_arg()),
        wh_creator("perlin", "Make a Perlin-noise image (seeded PRNG).").arg(seed_arg()),
        wh_creator("worley", "Make a Worley-noise image (seeded PRNG).").arg(seed_arg()),
        wh_creator("fractsurf", "Make a fractal surface.").arg(f64_pos(
            "FRACTAL_DIMENSION",
            "Fractal dimension (vips 2..3)",
        )),
        // --- LUT / matrix builders ------------------------------------------
        Command::new("buildlut")
            .about("Build a look-up table from a control-point matrix file.")
            .arg(
                Arg::new("IN")
                    .required(true)
                    .value_name("IN.mat")
                    .help("Control-point matrix as a vips text matrix file"),
            )
            .arg(out_arg()),
        Command::new("tonelut")
            .about("Build a print tone-mapping look-up table.")
            .arg(out_arg()),
        // --- frequency-domain masks -----------------------------------------
        wh_creator("mask_ideal", "Make an ideal frequency-domain filter mask.")
            .arg(f64_pos("FREQUENCY_CUTOFF", "Frequency cutoff (vips >= 0)"))
            .arg(nodc_flag()),
        wh_creator("mask_ideal_ring", "Make an ideal ring filter mask.")
            .arg(f64_pos("FREQUENCY_CUTOFF", "Frequency cutoff (vips >= 0)"))
            .arg(f64_pos("RINGWIDTH", "Ring width (vips >= 0)"))
            .arg(nodc_flag()),
        wh_creator("mask_ideal_band", "Make an ideal band filter mask.")
            .arg(f64_pos(
                "FREQUENCY_CUTOFF_X",
                "Frequency cutoff x (vips >= 0)",
            ))
            .arg(f64_pos(
                "FREQUENCY_CUTOFF_Y",
                "Frequency cutoff y (vips >= 0)",
            ))
            .arg(f64_pos("RADIUS", "Radius of the circle (vips >= 0)")),
        wh_creator(
            "mask_gaussian",
            "Make a Gaussian frequency-domain filter mask.",
        )
        .arg(f64_pos("FREQUENCY_CUTOFF", "Frequency cutoff (vips >= 0)"))
        .arg(f64_pos("AMPLITUDE_CUTOFF", "Amplitude cutoff (vips 0..1)"))
        .arg(nodc_flag()),
        wh_creator("mask_gaussian_ring", "Make a Gaussian ring filter mask.")
            .arg(f64_pos("FREQUENCY_CUTOFF", "Frequency cutoff (vips >= 0)"))
            .arg(f64_pos("AMPLITUDE_CUTOFF", "Amplitude cutoff (vips 0..1)"))
            .arg(f64_pos("RINGWIDTH", "Ring width (vips >= 0)"))
            .arg(nodc_flag()),
        wh_creator("mask_gaussian_band", "Make a Gaussian band filter mask.")
            .arg(f64_pos(
                "FREQUENCY_CUTOFF_X",
                "Frequency cutoff x (vips >= 0)",
            ))
            .arg(f64_pos(
                "FREQUENCY_CUTOFF_Y",
                "Frequency cutoff y (vips >= 0)",
            ))
            .arg(f64_pos("RADIUS", "Radius of the circle (vips >= 0)"))
            .arg(f64_pos("AMPLITUDE_CUTOFF", "Amplitude cutoff (vips 0..1)")),
        wh_creator(
            "mask_butterworth",
            "Make a Butterworth frequency-domain filter mask.",
        )
        .arg(f64_pos("ORDER", "Filter order (vips >= 1)"))
        .arg(f64_pos("FREQUENCY_CUTOFF", "Frequency cutoff (vips >= 0)"))
        .arg(f64_pos("AMPLITUDE_CUTOFF", "Amplitude cutoff (vips 0..1)"))
        .arg(nodc_flag())
        .arg(bool_flag(
            "optical",
            "Rotate quadrants to optical space (vips --optical)",
        ))
        .arg(bool_flag(
            "uchar",
            "Output an unsigned char image (vips --uchar)",
        )),
        wh_creator(
            "mask_butterworth_ring",
            "Make a Butterworth ring filter mask.",
        )
        .arg(f64_pos("ORDER", "Filter order (vips >= 1)"))
        .arg(f64_pos("FREQUENCY_CUTOFF", "Frequency cutoff (vips >= 0)"))
        .arg(f64_pos("AMPLITUDE_CUTOFF", "Amplitude cutoff (vips 0..1)"))
        .arg(f64_pos("RINGWIDTH", "Ring width (vips >= 0)"))
        .arg(nodc_flag()),
        wh_creator(
            "mask_butterworth_band",
            "Make a Butterworth band filter mask.",
        )
        .arg(f64_pos("ORDER", "Filter order (vips >= 1)"))
        .arg(f64_pos(
            "FREQUENCY_CUTOFF_X",
            "Frequency cutoff x (vips >= 0)",
        ))
        .arg(f64_pos(
            "FREQUENCY_CUTOFF_Y",
            "Frequency cutoff y (vips >= 0)",
        ))
        .arg(f64_pos("RADIUS", "Radius of the circle (vips >= 0)"))
        .arg(f64_pos("AMPLITUDE_CUTOFF", "Amplitude cutoff (vips 0..1)"))
        .arg(bool_flag(
            "uchar",
            "Output an unsigned char image (vips --uchar)",
        ))
        .arg(bool_flag(
            "optical",
            "Rotate quadrants to optical space (vips --optical)",
        ))
        .arg(nodc_flag()),
        wh_creator("mask_fractal", "Make a fractal power-spectrum filter mask.").arg(f64_pos(
            "FRACTAL_DIMENSION",
            "Fractal dimension (vips 2..3)",
        )),
        // --- signed distance fields -----------------------------------------
        wh_creator("sdf", "Make a signed distance field for a shape.")
            .arg(
                Arg::new("SHAPE")
                    .required(true)
                    .value_name("circle|box|rounded-box|line")
                    .value_parser(["circle", "box", "rounded-box", "line"])
                    .help("Shape to render (vips VipsSdfShape)"),
            )
            .arg(
                Arg::new("a")
                    .long("a")
                    .value_name("\"x y\"")
                    .default_value("0 0")
                    .help(
                        "Point a: circle centre / box corner / line start, INTEGER \
                         \"x y\" (vips --a; core SdfParams is integer-only — a §9 \
                         subset of vips's gdouble VipsArrayDouble, so \"10.5 10.5\" \
                         is rejected where vips would accept it)",
                    ),
            )
            .arg(Arg::new("b").long("b").value_name("\"x y\"").help(
                "Point b: opposite box corner / line end, INTEGER \"x y\" \
                         (vips --b; integer-only, §9 subset of vips's gdouble)",
            ))
            .arg(
                Arg::new("r")
                    .long("r")
                    .value_name("R")
                    .default_value("50")
                    .value_parser(value_parser!(i64).range(0..))
                    .help(
                        "Circle radius, INTEGER (vips --r, default 50, >= 0; \
                         integer-only, §9 subset of vips's gdouble — \"16.5\" is \
                         rejected where vips would accept it)",
                    ),
            )
            .arg(
                Arg::new("corners")
                    .long("corners")
                    .value_name("\"c0 c1 c2 c3\"")
                    .help(
                        "rounded-box corner radii, clockwise from top-right, \
                         INTEGER (vips --corners; integer-only, §9 subset of vips's \
                         gdouble)",
                    ),
            ),
        // --- text -----------------------------------------------------------
        Command::new("text")
            .about("Render text to a coverage image.")
            .arg(out_arg())
            .arg(
                Arg::new("TEXT")
                    .required(true)
                    .value_name("STRING")
                    .help("Text to render"),
            )
            .arg(
                Arg::new("dpi")
                    .long("dpi")
                    .value_name("DPI")
                    .value_parser(value_parser!(u32).range(1..))
                    .help("Render resolution (vips --dpi, default 72)"),
            )
            .arg(
                Arg::new("width")
                    .long("width")
                    .value_name("PX")
                    .value_parser(value_parser!(u32).range(1..))
                    .help("Maximum image width in pixels; wraps lines (vips --width)"),
            )
            .arg(
                Arg::new("height")
                    .long("height")
                    .value_name("PX")
                    .value_parser(value_parser!(u32).range(1..))
                    .help("Maximum image height in pixels; enables dpi auto-fit (vips --height)"),
            )
            .arg(
                Arg::new("wrap")
                    .long("wrap")
                    .value_name("word|char|word-char|none")
                    .value_parser(["word", "char", "word-char", "none"])
                    .help("Line-wrap mode (vips --wrap, default word)"),
            ),
    ]
}

/// The `--seed` flag shared by the noise generators. vips's `--seed` is a
/// `gint` (range `-2^31..=2^31-1`) and accepts negatives, so the CLI does too:
/// the value is parsed within the vips gint range and bit-reinterpreted to the
/// `u32` the core PRNG seed takes (`seed as u32`, two's-complement — the same
/// reinterpret vips performs when it uses the signed seed as unsigned state).
/// This matches vips's acceptance surface exactly (`--seed -5` is honored, and
/// a value above `2^31-1` is refused just as vips refuses the out-of-range
/// gint), rather than the earlier `u32`-only narrowing.
fn seed_arg() -> Arg {
    Arg::new("seed")
        .long("seed")
        .value_name("N")
        .default_value("0")
        .allow_hyphen_values(true)
        .value_parser(value_parser!(i64).range(-2_147_483_648_i64..=2_147_483_647_i64))
        .help("PRNG seed (vips --seed gint, default 0; negatives honored)")
}

/// Dispatch a matched create subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "black" => run_black(m),
        "xyz" => run_xyz(m),
        "eye" => run_eye(m),
        "zone" => run_zone(m),
        "sines" => run_sines(m),
        "gaussnoise" => run_gaussnoise(m),
        "perlin" => run_perlin(m),
        "worley" => run_worley(m),
        "fractsurf" => run_fractsurf(m),
        "buildlut" => run_buildlut(m),
        "tonelut" => run_tonelut(m),
        "mask_ideal" => run_mask_ideal(m),
        "mask_ideal_ring" => run_mask_ideal_ring(m),
        "mask_ideal_band" => run_mask_ideal_band(m),
        "mask_gaussian" => run_mask_gaussian(m),
        "mask_gaussian_ring" => run_mask_gaussian_ring(m),
        "mask_gaussian_band" => run_mask_gaussian_band(m),
        "mask_butterworth" => run_mask_butterworth(m),
        "mask_butterworth_ring" => run_mask_butterworth_ring(m),
        "mask_butterworth_band" => run_mask_butterworth_band(m),
        "mask_fractal" => run_mask_fractal(m),
        "sdf" => run_sdf(m),
        "text" => run_text(m),
        other => bail!("create family has no command {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Argument accessors + validators.
// ---------------------------------------------------------------------------

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Read a required `u32` dimension (clap already bounded it to `>= 1`).
fn dim(m: &ArgMatches, id: &str) -> u32 {
    *m.get_one::<u32>(id).expect("clap guarantees the dimension")
}

/// Read a required positional `f64`.
fn f64_at(m: &ArgMatches, id: &str) -> f64 {
    *m.get_one::<f64>(id).expect("clap guarantees the argument")
}

/// The `OUT` positional as a path.
fn out_path(m: &ArgMatches) -> PathBuf {
    PathBuf::from(pos(m, "OUT"))
}

/// Require `v >= min`, else a typed exit-1 error (vips's declared minimum).
fn require_min(name: &str, v: f64, min: f64) -> Result<f64> {
    if v < min {
        bail!("{name} {v} is below the vips minimum {min}");
    }
    Ok(v)
}

/// Require `min <= v <= max`, else a typed exit-1 error.
fn require_range(name: &str, v: f64, min: f64, max: f64) -> Result<f64> {
    if v < min || v > max {
        bail!("{name} {v} is outside the vips range {min}..={max}");
    }
    Ok(v)
}

/// A frequency cutoff / radius / ring width (vips range 0..=1e6).
fn nonneg(m: &ArgMatches, id: &str) -> Result<f64> {
    require_range(id, f64_at(m, id), 0.0, VIPS_MASK_PARAM_MAX)
}

/// An amplitude cutoff (vips range 0..1).
fn amplitude(m: &ArgMatches, id: &str) -> Result<f64> {
    require_range(id, f64_at(m, id), 0.0, 1.0)
}

/// Parse a whitespace-separated `[i64; N]` vector arg (vips `VipsArrayDouble`
/// coordinates; the core `SdfParams` carries integer points).
fn parse_i64_vec(name: &str, s: &str) -> Result<Vec<i64>> {
    s.split_whitespace()
        .map(|t| {
            t.parse::<i64>()
                .map_err(|e| anyhow!("{name} value {t:?} is not an integer: {e}"))
        })
        .collect()
}

/// Parse a required `[i64; 2]` point.
fn parse_point(name: &str, s: &str) -> Result<[i64; 2]> {
    let v = parse_i64_vec(name, s)?;
    match v.as_slice() {
        [x, y] => Ok([*x, *y]),
        _ => bail!(
            "{name} must be two integers \"x y\", got {} value(s)",
            v.len()
        ),
    }
}

// ---------------------------------------------------------------------------
// Handlers.
// ---------------------------------------------------------------------------

/// `black OUT W H --bands N` — S5; all-zero uchar image.
fn run_black(m: &ArgMatches) -> Result<()> {
    let bands = *m.get_one::<u32>("bands").expect("clap default 1");
    // @doc-snippet:begin command=black slot=create
    let out_raster = Raster::try_black_bands(dim(m, "WIDTH"), dim(m, "HEIGHT"), bands)?;
    // @doc-snippet:end command=black slot=create
    // @doc-snippet:begin command=black slot=save imports=save_file
    io::save(&out_raster, &out_path(m))?;
    // @doc-snippet:end command=black slot=save
    Ok(())
}

/// `xyz OUT W H` — S5; each pixel is its own `[x, y]` coordinate.
fn run_xyz(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_xyz(dim(m, "WIDTH"), dim(m, "HEIGHT"))?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `eye OUT W H --uchar` — S5; eye spatial-frequency response.
fn run_eye(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_eye(dim(m, "WIDTH"), dim(m, "HEIGHT"), m.get_flag("uchar"))?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `zone OUT W H` — S5; zone-plate pattern (float only in core).
fn run_zone(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_zone(dim(m, "WIDTH"), dim(m, "HEIGHT"))?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `sines OUT W H` — S5; 2D sine grating (default frequencies).
fn run_sines(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_sines(dim(m, "WIDTH"), dim(m, "HEIGHT"))?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `gaussnoise OUT W H --sigma --mean --seed` — S5 GOLDEN-ONLY.
fn run_gaussnoise(m: &ArgMatches) -> Result<()> {
    let sigma = require_min("sigma", *m.get_one::<f64>("sigma").expect("default"), 0.0)?;
    let mean = *m.get_one::<f64>("mean").expect("default");
    // vips gint seed → core u32 via a two's-complement bit-reinterpret (so a
    // negative seed maps to the same unsigned state vips would use).
    let seed = *m.get_one::<i64>("seed").expect("default") as u32;
    let out_raster =
        Raster::try_gaussnoise_seeded(dim(m, "WIDTH"), dim(m, "HEIGHT"), mean, sigma, seed)?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `perlin OUT W H --seed` — S5 GOLDEN-ONLY.
fn run_perlin(m: &ArgMatches) -> Result<()> {
    // vips gint seed → core u32 via a two's-complement bit-reinterpret (so a
    // negative seed maps to the same unsigned state vips would use).
    let seed = *m.get_one::<i64>("seed").expect("default") as u32;
    let out_raster = Raster::try_perlin_seeded(dim(m, "WIDTH"), dim(m, "HEIGHT"), seed)?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `worley OUT W H --seed` — S5 GOLDEN-ONLY.
fn run_worley(m: &ArgMatches) -> Result<()> {
    // vips gint seed → core u32 via a two's-complement bit-reinterpret (so a
    // negative seed maps to the same unsigned state vips would use).
    let seed = *m.get_one::<i64>("seed").expect("default") as u32;
    let out_raster = Raster::try_worley_seeded(dim(m, "WIDTH"), dim(m, "HEIGHT"), seed)?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `fractsurf OUT W H FRACTAL_DIMENSION` — S5 GOLDEN-ONLY.
fn run_fractsurf(m: &ArgMatches) -> Result<()> {
    let fd = require_range(
        "FRACTAL_DIMENSION",
        f64_at(m, "FRACTAL_DIMENSION"),
        2.0,
        3.0,
    )?;
    let out_raster = Raster::try_fractsurf(dim(m, "WIDTH"), dim(m, "HEIGHT"), fd)?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `buildlut IN.mat OUT` — S1; piece-wise linear LUT from a matrix file.
fn run_buildlut(m: &ArgMatches) -> Result<()> {
    let in_path = PathBuf::from(pos(m, "IN"));
    // @doc-snippet:begin command=buildlut slot=load imports=decode_file
    let mat = MatFile::load(&in_path)?;
    // @doc-snippet:end command=buildlut slot=load
    let rows: Vec<Vec<f64>> = mat.as_f64_rows().iter().map(|r| r.to_vec()).collect();
    // @doc-snippet:begin command=buildlut slot=create
    let out_raster = Raster::try_buildlut(&rows)?;
    // @doc-snippet:end command=buildlut slot=create
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `tonelut OUT` — S5; print tone-mapping curve (defaults only).
fn run_tonelut(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_tonelut()?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_ideal OUT W H FC --nodc` — S5.
fn run_mask_ideal(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_ideal(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        nonneg(m, "FREQUENCY_CUTOFF")?,
        m.get_flag("nodc"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_ideal_ring OUT W H FC RW --nodc` — S5.
fn run_mask_ideal_ring(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_ideal_ring(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        nonneg(m, "FREQUENCY_CUTOFF")?,
        nonneg(m, "RINGWIDTH")?,
        m.get_flag("nodc"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_ideal_band OUT W H FCX FCY R` — S5 (no `--nodc` in core).
fn run_mask_ideal_band(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_ideal_band(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        nonneg(m, "FREQUENCY_CUTOFF_X")?,
        nonneg(m, "FREQUENCY_CUTOFF_Y")?,
        nonneg(m, "RADIUS")?,
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_gaussian OUT W H FC AC --nodc` — S5.
fn run_mask_gaussian(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_gaussian(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        nonneg(m, "FREQUENCY_CUTOFF")?,
        amplitude(m, "AMPLITUDE_CUTOFF")?,
        m.get_flag("nodc"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_gaussian_ring OUT W H FC AC RW --nodc` — S5.
fn run_mask_gaussian_ring(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_gaussian_ring(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        nonneg(m, "FREQUENCY_CUTOFF")?,
        amplitude(m, "AMPLITUDE_CUTOFF")?,
        nonneg(m, "RINGWIDTH")?,
        m.get_flag("nodc"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_gaussian_band OUT W H FCX FCY R AC` — S5 (no `--nodc` in core).
fn run_mask_gaussian_band(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_gaussian_band(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        nonneg(m, "FREQUENCY_CUTOFF_X")?,
        nonneg(m, "FREQUENCY_CUTOFF_Y")?,
        nonneg(m, "RADIUS")?,
        amplitude(m, "AMPLITUDE_CUTOFF")?,
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_butterworth OUT W H ORD FC AC --nodc --optical --uchar` — S5.
fn run_mask_butterworth(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_butterworth(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        require_range("ORDER", f64_at(m, "ORDER"), 1.0, VIPS_MASK_PARAM_MAX)?,
        nonneg(m, "FREQUENCY_CUTOFF")?,
        amplitude(m, "AMPLITUDE_CUTOFF")?,
        m.get_flag("nodc"),
        m.get_flag("optical"),
        m.get_flag("uchar"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_butterworth_ring OUT W H ORD FC AC RW --nodc` — S5 (core has only nodc).
fn run_mask_butterworth_ring(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_butterworth_ring(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        require_range("ORDER", f64_at(m, "ORDER"), 1.0, VIPS_MASK_PARAM_MAX)?,
        nonneg(m, "FREQUENCY_CUTOFF")?,
        amplitude(m, "AMPLITUDE_CUTOFF")?,
        nonneg(m, "RINGWIDTH")?,
        m.get_flag("nodc"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_butterworth_band OUT W H ORD FCX FCY R AC --uchar --optical --nodc` — S5.
fn run_mask_butterworth_band(m: &ArgMatches) -> Result<()> {
    let out_raster = Raster::try_mask_butterworth_band(
        dim(m, "WIDTH"),
        dim(m, "HEIGHT"),
        require_range("ORDER", f64_at(m, "ORDER"), 1.0, VIPS_MASK_PARAM_MAX)?,
        nonneg(m, "FREQUENCY_CUTOFF_X")?,
        nonneg(m, "FREQUENCY_CUTOFF_Y")?,
        nonneg(m, "RADIUS")?,
        amplitude(m, "AMPLITUDE_CUTOFF")?,
        m.get_flag("uchar"),
        m.get_flag("optical"),
        m.get_flag("nodc"),
    )?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `mask_fractal OUT W H FRACTAL_DIMENSION` — S5.
fn run_mask_fractal(m: &ArgMatches) -> Result<()> {
    let fd = require_range(
        "FRACTAL_DIMENSION",
        f64_at(m, "FRACTAL_DIMENSION"),
        2.0,
        3.0,
    )?;
    let out_raster = Raster::try_mask_fractal(dim(m, "WIDTH"), dim(m, "HEIGHT"), fd)?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `sdf OUT W H SHAPE --a "x y" --b "x y" --r --corners "c c c c"` — S5.
fn run_sdf(m: &ArgMatches) -> Result<()> {
    let shape = pos(m, "SHAPE");
    let a = parse_point("--a", pos(m, "a"))?;
    let b = match m.get_one::<String>("b") {
        Some(s) => Some(parse_point("--b", s)?),
        None => None,
    };
    let r = Some(*m.get_one::<i64>("r").expect("clap default 50"));
    let corners = match m.get_one::<String>("corners") {
        Some(s) => {
            let v = parse_i64_vec("--corners", s)?;
            match v.as_slice() {
                [a, b, c, d] => Some([*a, *b, *c, *d]),
                _ => bail!(
                    "--corners must be four integers \"c0 c1 c2 c3\", got {}",
                    v.len()
                ),
            }
        }
        None => None,
    };
    let params = SdfParams { a, b, r, corners };
    let out_raster = Raster::try_sdf(dim(m, "WIDTH"), dim(m, "HEIGHT"), shape, &params)?;
    io::save(&out_raster, &out_path(m))?;
    Ok(())
}

/// `text OUT STRING --dpi --width --height --wrap` — S5 GOLDEN-ONLY.
fn run_text(m: &ArgMatches) -> Result<()> {
    let text = pos(m, "TEXT");
    let dpi = m.get_one::<u32>("dpi").copied();
    let width = m.get_one::<u32>("width").copied();
    let height = m.get_one::<u32>("height").copied();
    let wrap = m.get_one::<String>("wrap").map(String::as_str);
    let out_raster = Raster::try_text(text, dpi, width, height, wrap)?;
    io::save(&out_raster, &out_path(m))?;
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
        assert_eq!(meta_names.len(), 23, "the create family has 23 commands");
    }

    #[test]
    fn black_parses_out_dims_and_bands() {
        let m = cmd("black")
            .try_get_matches_from(["black", "out.png", "8", "8", "--bands", "3"])
            .unwrap();
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(dim(&m, "WIDTH"), 8);
        assert_eq!(dim(&m, "HEIGHT"), 8);
        assert_eq!(*m.get_one::<u32>("bands").unwrap(), 3);
    }

    #[test]
    fn black_bands_defaults_to_one() {
        let m = cmd("black")
            .try_get_matches_from(["black", "out.v", "4", "4"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("bands").unwrap(), 1);
    }

    #[test]
    fn dimensions_below_one_are_rejected_by_clap() {
        // vips's minimum width/height is 1; clap rejects 0 at parse (exit 2),
        // never letting a zero-dimension request reach the core.
        assert!(
            cmd("black")
                .try_get_matches_from(["black", "out.v", "0", "8"])
                .is_err(),
            "a zero WIDTH must be rejected"
        );
    }

    #[test]
    fn eye_uchar_flag_is_off_by_default() {
        let m = cmd("eye")
            .try_get_matches_from(["eye", "out.v", "16", "16"])
            .unwrap();
        assert!(!m.get_flag("uchar"));
        let m = cmd("eye")
            .try_get_matches_from(["eye", "out.png", "16", "16", "--uchar"])
            .unwrap();
        assert!(m.get_flag("uchar"));
    }

    #[test]
    fn sdf_enforces_the_shape_enum() {
        assert!(
            cmd("sdf")
                .clone()
                .try_get_matches_from(["sdf", "out.v", "32", "32", "circle"])
                .is_ok()
        );
        assert!(
            cmd("sdf")
                .try_get_matches_from(["sdf", "out.v", "32", "32", "triangle"])
                .is_err(),
            "only circle|box|rounded-box|line are core-backed"
        );
    }

    #[test]
    fn sdf_parses_point_and_corner_vectors() {
        let m = cmd("sdf")
            .try_get_matches_from([
                "sdf",
                "out.v",
                "64",
                "64",
                "rounded-box",
                "--a",
                "10 10",
                "--b",
                "50 40",
                "--corners",
                "20 0 0 0",
                "--r",
                "16",
            ])
            .unwrap();
        assert_eq!(parse_point("--a", pos(&m, "a")).unwrap(), [10, 10]);
        assert_eq!(
            parse_point("--b", m.get_one::<String>("b").unwrap()).unwrap(),
            [50, 40]
        );
        assert_eq!(*m.get_one::<i64>("r").unwrap(), 16);
        assert_eq!(
            parse_i64_vec("--corners", m.get_one::<String>("corners").unwrap()).unwrap(),
            vec![20, 0, 0, 0]
        );
    }

    #[test]
    fn sdf_r_defaults_to_the_vips_default() {
        let m = cmd("sdf")
            .try_get_matches_from(["sdf", "out.v", "32", "32", "circle", "--a", "16 16"])
            .unwrap();
        assert_eq!(*m.get_one::<i64>("r").unwrap(), 50);
    }

    #[test]
    fn parse_point_rejects_wrong_arity() {
        assert!(parse_point("--a", "1").is_err(), "one value is not a point");
        assert!(
            parse_point("--a", "1 2 3").is_err(),
            "three values is not a point"
        );
        assert!(parse_point("--a", "1 x").is_err(), "a non-integer errors");
    }

    #[test]
    fn fractal_dimension_out_of_range_is_a_typed_error() {
        // fractsurf's fractal dimension is bounded to vips's 2..3. An out-of-range
        // value is a typed exit-1 error (validated before any raster is built),
        // never a panic. Dimensions are valid, so only the dimension check fires.
        let m = cmd("fractsurf")
            .try_get_matches_from(["fractsurf", "out.v", "16", "16", "5.0"])
            .unwrap();
        let err = run_fractsurf(&m).unwrap_err();
        assert!(
            err.to_string().contains("outside the vips range"),
            "expected a range error, got: {err}"
        );
    }

    #[test]
    fn amplitude_cutoff_above_one_is_a_typed_error() {
        let m = cmd("mask_gaussian")
            .try_get_matches_from(["mask_gaussian", "out.v", "16", "16", "0.5", "2.0"])
            .unwrap();
        let err = run_mask_gaussian(&m).unwrap_err();
        assert!(
            err.to_string().contains("outside the vips range"),
            "expected an amplitude range error, got: {err}"
        );
    }

    #[test]
    fn negative_frequency_cutoff_is_rejected() {
        // A leading-negative positional (`-0.5`) reads to clap as an unknown
        // flag, so it is rejected at parse time (exit 2) before the handler — a
        // clean rejection, never a panic. The handler's own `require_min(>= 0)`
        // guard (see `nonneg`) is the belt-and-suspenders floor for any value
        // that does slip through as a positive-looking token.
        assert!(
            cmd("mask_ideal")
                .try_get_matches_from(["mask_ideal", "out.v", "16", "16", "-0.5"])
                .is_err(),
            "a negative frequency cutoff must be rejected"
        );
    }

    #[test]
    fn butterworth_order_below_one_is_a_typed_error() {
        // The Butterworth order's vips minimum is 1; a positive-but-too-small
        // order (0.5, which clap admits as a value) becomes a typed exit-1 error
        // in the handler, never a panic. Uses a valid frequency/amplitude so
        // only the order check fires.
        let m = cmd("mask_butterworth")
            .try_get_matches_from(["mask_butterworth", "out.v", "16", "16", "0.5", "0.5", "0.5"])
            .unwrap();
        let err = run_mask_butterworth(&m).unwrap_err();
        assert!(
            err.to_string().contains("outside the vips range"),
            "expected an order range error, got: {err}"
        );
    }

    #[test]
    fn text_parses_string_and_optional_flags() {
        let m = cmd("text")
            .try_get_matches_from(["text", "out.png", "Hello", "--dpi", "150", "--wrap", "char"])
            .unwrap();
        assert_eq!(pos(&m, "TEXT"), "Hello");
        assert_eq!(*m.get_one::<u32>("dpi").unwrap(), 150);
        assert_eq!(m.get_one::<String>("wrap").unwrap(), "char");
        assert!(m.get_one::<u32>("width").is_none());
    }

    #[test]
    fn text_enforces_the_wrap_enum() {
        assert!(
            cmd("text")
                .try_get_matches_from(["text", "out.png", "hi", "--wrap", "banana"])
                .is_err(),
            "only word|char|word-char|none are core-backed"
        );
    }

    #[test]
    fn seed_defaults_to_zero() {
        let m = cmd("gaussnoise")
            .try_get_matches_from(["gaussnoise", "out.v", "16", "16"])
            .unwrap();
        assert_eq!(*m.get_one::<i64>("seed").unwrap(), 0);
        assert_eq!(*m.get_one::<f64>("sigma").unwrap(), 30.0);
        assert_eq!(*m.get_one::<f64>("mean").unwrap(), 128.0);
    }

    #[test]
    fn negative_seed_is_accepted_and_bit_reinterpreted() {
        // vips's `--seed` is a gint and accepts negatives; the CLI honors them
        // (parity with vips), parsing within the gint range and reinterpreting
        // to the core u32 seed via a two's-complement cast. Regression guard for
        // the earlier u32-only narrowing that rejected `--seed -5` at exit 2.
        for op in ["gaussnoise", "perlin", "worley"] {
            let m = cmd(op)
                .try_get_matches_from([op, "out.v", "16", "16", "--seed", "-5"])
                .unwrap_or_else(|e| panic!("{op} must accept a negative seed: {e}"));
            let seed = *m.get_one::<i64>("seed").unwrap();
            assert_eq!(seed, -5);
            // -5 as u32 is 0xFFFF_FFFB (two's complement), the unsigned state.
            assert_eq!(seed as u32, 4_294_967_291);
        }
    }

    #[test]
    fn seed_above_gint_max_is_rejected() {
        // Above vips's gint maximum (2^31-1) the CLI refuses the seed exactly as
        // vips refuses to set the out-of-range gint property (was: accepted up to
        // u32::MAX, an over-wide surface vs vips).
        assert!(
            cmd("perlin")
                .try_get_matches_from(["perlin", "out.v", "16", "16", "--seed", "2147483648"])
                .is_err(),
            "a seed above the vips gint max must be rejected"
        );
    }

    #[test]
    fn dimension_above_vips_max_coord_is_rejected() {
        // vips's VIPS_MAX_COORD is 1e8; a larger dimension is refused by clap
        // exactly as vips refuses the out-of-range gint (finding 6).
        assert!(
            cmd("black")
                .try_get_matches_from(["black", "out.v", "100000001", "8"])
                .is_err(),
            "a WIDTH above vips's 1e8 maximum must be rejected"
        );
        assert!(
            cmd("black")
                .try_get_matches_from(["black", "out.v", "100000000", "8"])
                .is_ok(),
            "the 1e8 maximum itself is accepted"
        );
    }

    #[test]
    fn mask_param_above_vips_max_is_a_typed_error() {
        // Frequency cutoff / order share vips's 1e6 gdouble maximum; a larger
        // value is a typed exit-1 error, matching vips's out-of-range refusal.
        let m = cmd("mask_ideal")
            .try_get_matches_from(["mask_ideal", "out.v", "16", "16", "1000001"])
            .unwrap();
        let err = run_mask_ideal(&m).unwrap_err();
        assert!(
            err.to_string().contains("outside the vips range"),
            "expected a frequency-cutoff range error, got: {err}"
        );
    }
}
