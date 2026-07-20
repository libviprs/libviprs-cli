//! Extract / geometry-placement op family — a per-family Wave-2 lane
//! (`CLI_CONTRACT.md` §3/§6, `OP_MAP.md` extract section).
//!
//! The core `src/extract.rs` exposes 19 `pub fn` that fold to eleven base ops;
//! those become **nine** `viprs` subcommands here (`crop` is a vips-callable
//! ALIAS of `extract_area` dispatching to the same handler, and the three
//! `smartcrop*` twins collapse into one command with `--interesting` /
//! `--premultiplied` selectors, exactly as `OP_MAP.md` prescribes). Every
//! command is oracle class **EXACT** (integer-in / integer-out, decode-compare
//! tol 0):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `extract_area IN OUT LEFT TOP WIDTH HEIGHT` | `extract_area` | S1 | EXACT | rectangle copy |
//! | `crop IN OUT LEFT TOP WIDTH HEIGHT`         | `crop` (alias) | S1 | EXACT | same handler |
//! | `embed IN OUT X Y WIDTH HEIGHT --extend --background` | `embed` | S1 | EXACT | place in a larger canvas |
//! | `gravity IN OUT DIRECTION WIDTH HEIGHT --extend --background` | `gravity` | S1 | EXACT | place at a compass position |
//! | `replicate IN OUT ACROSS DOWN` | `replicate` | S1 | EXACT | tile |
//! | `zoom IN OUT XFAC YFAC`         | `zoom`      | S1 | EXACT | integer pixel-replication upscale |
//! | `subsample IN OUT XFAC YFAC`    | `subsample` | S1 | EXACT | integer point-sample downscale |
//! | `insert MAIN SUB OUT X Y --expand` | `insert` | S2 | EXACT | composite one image onto another |
//! | `smartcrop IN OUT WIDTH HEIGHT --interesting --premultiplied` | `smartcrop` | S1 | EXACT | crop the most interesting area |
//!
//! Positional orders, flag names, enum spellings, and input value bounds mirror
//! vips 8.18.4 exactly (verified against `vips <op>` usage):
//!
//! * `extract_area`/`crop`'s `LEFT`/`TOP` accept vips's full signed gint range
//!   (`-100000000..=100000000`) at parse but the core geometry is `u32`-only, so
//!   a negative coordinate becomes a **typed exit-1 error** rather than a
//!   `u32::try_from` abort (the bands B2 lesson); `WIDTH`/`HEIGHT` carry vips's
//!   `extract_area` bounds `1..=100000000` (the smaller crop-dim cap, NOT the
//!   `1e9` canvas cap embed/gravity use).
//! * `embed`'s `X`/`Y` are signed (`i32`, vips's `±1e9`) and `insert`'s `X`/`Y`
//!   are signed (`i32`, vips's tighter `±1e8`), each passed straight through;
//!   embed/gravity canvas `WIDTH`/`HEIGHT` carry vips's `1..=1000000000`.
//! * `replicate`/`zoom`/`subsample` factors carry vips's `1..` minima.
//! * `gravity`'s `DIRECTION` enum is vips's exact nine `VipsCompassDirection`
//!   nicknames; `--extend` is the six `VipsExtend` nicknames.
//! * `smartcrop`'s `--interesting` exposes exactly the six core
//!   [`SmartcropInteresting`] strategies (vips's `none` is intentionally
//!   excluded — not core-backed), default `attention` (vips's default).
//!
//! `insert` exposes `--background` (core `try_insert` fills expand/off-canvas
//! pixels with the given colour, default black, matching vips). One vips feature
//! the core does NOT back is intentionally NOT exposed (bands `lshift`/`rshift`
//! precedent — no invented parity): `subsample`'s `--point` (the core always
//! point-samples the top-left of each cell), documented as a subset in help.
//!
//! Handlers keep the §3 `load → try_op → save` shape and call only the panic-free
//! `try_*` core APIs, so a bad input becomes exit 1 rather than an abort
//! (`CLI_CONTRACT.md` §8).
//
// @doc-command:begin name=extract_area about="Extract a rectangular region from an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=extract_area
// @doc-command:begin name=crop about="Crop a rectangle out of an image (alias of extract_area)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=crop
// @doc-command:begin name=embed about="Embed an image in a larger canvas at a given position." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=embed
// @doc-command:begin name=gravity about="Place an image within a larger canvas at a compass position." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=gravity
// @doc-command:begin name=replicate about="Tile an image across and down." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=replicate
// @doc-command:begin name=zoom about="Zoom in by integer pixel replication." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=zoom
// @doc-command:begin name=subsample about="Subsample (point-decimate) an image by integer factors." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=subsample
// @doc-command:begin name=insert about="Insert one image onto another at a position." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=insert
// @doc-command:begin name=smartcrop about="Crop to the most interesting area of an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=smartcrop

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::{CompassDirection, Extend, SmartcropInteresting};

use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "extract_area",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "crop",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "embed",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "gravity",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "replicate",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "zoom",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "subsample",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "insert",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "smartcrop",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
    ]
}

/// The nine `VipsCompassDirection` nicknames, verbatim from `vips gravity`.
const COMPASS: [&str; 9] = [
    "centre",
    "north",
    "east",
    "south",
    "west",
    "north-east",
    "south-east",
    "south-west",
    "north-west",
];

/// The six `VipsExtend` nicknames, verbatim from `vips embed`.
const EXTEND: [&str; 6] = ["black", "copy", "repeat", "mirror", "white", "background"];

/// The six core [`SmartcropInteresting`] strategy names (vips's `none` is
/// intentionally excluded — not core-backed).
const INTERESTING: [&str; 6] = ["centre", "entropy", "attention", "low", "high", "all"];

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(extract_area_command("extract_area")),
        io::with_decode_limit_args(extract_area_command("crop")),
        io::with_decode_limit_args(
            Command::new("embed")
                .about("Embed an image in a larger canvas at a given position.")
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(coord_arg("X", "Left edge of the input in the output"))
                .arg(coord_arg("Y", "Top edge of the input in the output"))
                .arg(dim_arg("WIDTH", "Canvas width in pixels"))
                .arg(dim_arg("HEIGHT", "Canvas height in pixels"))
                .arg(extend_arg())
                .arg(background_arg()),
        ),
        io::with_decode_limit_args(
            Command::new("gravity")
                .about("Place an image within a larger canvas at a compass position.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("DIRECTION")
                        .required(true)
                        .value_name("centre|north|east|south|west|north-east|…")
                        .value_parser(COMPASS)
                        .help("Compass position to place the image at"),
                )
                .arg(dim_arg("WIDTH", "Canvas width in pixels"))
                .arg(dim_arg("HEIGHT", "Canvas height in pixels"))
                .arg(extend_arg())
                .arg(background_arg()),
        ),
        io::with_decode_limit_args(
            Command::new("replicate")
                .about("Tile an image across and down.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(factor_arg("ACROSS", 1_000_000, "Repeat this many times horizontally"))
                .arg(factor_arg("DOWN", 1_000_000, "Repeat this many times vertically")),
        ),
        io::with_decode_limit_args(
            Command::new("zoom")
                .about("Zoom in by integer pixel replication.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(factor_arg("XFAC", 100_000_000, "Horizontal zoom factor"))
                .arg(factor_arg("YFAC", 100_000_000, "Vertical zoom factor")),
        ),
        io::with_decode_limit_args(
            Command::new("subsample")
                .about("Subsample (point-decimate) an image by integer factors.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(factor_arg(
                    "XFAC",
                    100_000_000,
                    "Horizontal subsample factor (top-left of each cell; vips's --point \
                     averaging mode is not core-backed)",
                ))
                .arg(factor_arg("YFAC", 100_000_000, "Vertical subsample factor")),
        ),
        io::with_decode_limit_args(
            Command::new("insert")
                .about("Insert one image onto another at a position.")
                .allow_negative_numbers(true)
                .arg(Arg::new("MAIN").required(true).help("Main (background) image"))
                .arg(Arg::new("SUB").required(true).help("Sub-image to insert"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(insert_coord_arg("X", "Left edge of the sub-image in the main image"))
                .arg(insert_coord_arg("Y", "Top edge of the sub-image in the main image"))
                .arg(
                    Arg::new("expand")
                        .long("expand")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Expand the output to hold all of both inputs (new pixels use \
                             --background, default black)",
                        ),
                )
                .arg(background_arg()),
        ),
        io::with_decode_limit_args(
            Command::new("smartcrop")
                .about("Crop to the most interesting area of an image.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(crop_dim_arg("WIDTH", "Width of the crop area"))
                .arg(crop_dim_arg("HEIGHT", "Height of the crop area"))
                .arg(
                    Arg::new("interesting")
                        .long("interesting")
                        .value_name("centre|entropy|attention|low|high|all")
                        .default_value("attention")
                        .value_parser(INTERESTING)
                        .help(
                            "How to measure interestingness (vips's `none` is excluded — not \
                             core-backed); prints the attention centre `x y` to stdout",
                        ),
                )
                .arg(
                    Arg::new("premultiplied")
                        .long("premultiplied")
                        .action(ArgAction::SetTrue)
                        .help("The input's alpha is already premultiplied (skip the internal premultiply)"),
                ),
        ),
    ]
}

/// Build the `extract_area` command skeleton, reused verbatim for the `crop`
/// alias (vips's `crop` is `extract_area` under another nickname).
fn extract_area_command(name: &'static str) -> Command {
    let about = if name == "crop" {
        "Crop a rectangle out of an image (alias of extract_area)."
    } else {
        "Extract a rectangular region from an image."
    };
    Command::new(name)
        .about(about)
        // vips's LEFT/TOP admit negative values; let clap read a leading-dash
        // token as a positional number rather than an unknown flag.
        .allow_negative_numbers(true)
        .arg(Arg::new("IN").required(true).help("Input image"))
        .arg(Arg::new("OUT").required(true).help("Output image"))
        // vips's LEFT/TOP are signed gint (`-1e8..=1e8`); the core geometry is
        // u32-only, so a negative coordinate is a typed exit-1 error at the
        // conversion, not a u32::try_from abort (bands B2 lesson).
        .arg(area_coord_arg("LEFT", "Left edge of the extract area"))
        .arg(area_coord_arg("TOP", "Top edge of the extract area"))
        .arg(crop_dim_arg("WIDTH", "Width of the extract area"))
        .arg(crop_dim_arg("HEIGHT", "Height of the extract area"))
}

/// A signed `extract_area`-style coordinate (vips gint `-1e8..=1e8`), read as
/// `i64` and later converted to the core's `u32` with a typed error.
fn area_coord_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(i64).range(-100_000_000..=100_000_000))
        .help(help)
}

/// A signed `embed` position (`i32`; vips's `embed` `x`/`y` gint range is
/// `±1e9`), passed straight to the core.
fn coord_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(i32).range(-1_000_000_000..=1_000_000_000))
        .help(help)
}

/// A signed `insert` position (`i32`; vips's `insert` `x`/`y` gint range is the
/// tighter `±1e8`, NOT embed's `±1e9`), passed straight to the core.
fn insert_coord_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(i32).range(-100_000_000..=100_000_000))
        .help(help)
}

/// An `embed`/`gravity` canvas dimension (`u32`, vips's canvas bounds
/// `1..=1e9`).
fn dim_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(u32).range(1..=1_000_000_000))
        .help(help)
}

/// An `extract_area`/`crop`/`smartcrop` crop dimension (`u32`, vips's
/// `extract_area`/`smartcrop` bounds `1..=1e8` — the smaller crop-dim cap, NOT
/// the `1e9` canvas cap [`dim_arg`] uses for embed/gravity).
fn crop_dim_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(u32).range(1..=100_000_000))
        .help(help)
}

/// An integer tiling factor (`u32`, vips minimum `1`, per-op maximum).
fn factor_arg(id: &'static str, max: i64, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(u32).range(1..=max))
        .help(help)
}

/// The shared `--extend` enum flag (`VipsExtend`), default `black`.
fn extend_arg() -> Arg {
    Arg::new("extend")
        .long("extend")
        .value_name("black|copy|repeat|mirror|white|background")
        .default_value("black")
        .value_parser(EXTEND)
        .help("How to fill the pixels around the input")
}

/// The shared `--background` vector flag (read only by `--extend background`).
fn background_arg() -> Arg {
    Arg::new("background")
        .long("background")
        .value_name("\"r g b\"")
        .help("Background colour for --extend background (space-separated, one value or one per band)")
}

/// Dispatch a matched extract subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        // `crop` is a vips-callable alias of `extract_area` (same handler).
        "extract_area" | "crop" => run_extract_area(m),
        "embed" => run_embed(m),
        "gravity" => run_gravity(m),
        "replicate" => run_replicate(m),
        "zoom" => run_zoom(m),
        "subsample" => run_subsample(m),
        "insert" => run_insert(m),
        "smartcrop" => run_smartcrop(m),
        other => bail!("extract family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Convert a clap-parsed signed coordinate to the core's `u32`, yielding a typed
/// exit-1 error for a negative value rather than a `u32::try_from` abort
/// (`CLI_CONTRACT.md` §8, bands B2 lesson). The clap `1e8` upper bound already
/// keeps the value inside `u32`.
fn area_coord(m: &ArgMatches, id: &str) -> Result<u32> {
    let v = *m.get_one::<i64>(id).expect("required positional");
    u32::try_from(v).map_err(|_| {
        anyhow!("{id} {v} must be >= 0 (the core extract geometry has no negative-coordinate form)")
    })
}

/// Parse a vips-style space-separated numeric vector (e.g. `"128"` / `"10 20 30"`).
fn parse_f64_vec(s: &str) -> Result<Vec<f64>> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| {
            t.parse::<f64>()
                .map_err(|e| anyhow!("background value {t:?} is not a number: {e}"))
        })
        .collect::<Result<_>>()?;
    if v.is_empty() {
        bail!("--background expected at least one value (a space-separated vector like \"128\")");
    }
    Ok(v)
}

/// Map the `--extend` string to the core [`Extend`] enum. clap restricts the
/// value to [`EXTEND`], so the fallthrough is unreachable in practice.
fn extend_of(m: &ArgMatches) -> Result<Extend> {
    Ok(match m.get_one::<String>("extend").map(String::as_str) {
        Some("black") => Extend::Black,
        Some("copy") => Extend::Copy,
        Some("repeat") => Extend::Repeat,
        Some("mirror") => Extend::Mirror,
        Some("white") => Extend::White,
        Some("background") => Extend::Background,
        other => bail!("unknown extend mode {other:?}"),
    })
}

/// The optional `--background` vector, parsed once.
fn background_of(m: &ArgMatches) -> Result<Option<Vec<f64>>> {
    match m.get_one::<String>("background") {
        Some(s) => Ok(Some(parse_f64_vec(s)?)),
        None => Ok(None),
    }
}

/// `extract_area / crop IN OUT LEFT TOP WIDTH HEIGHT` — S1 rectangle copy.
fn run_extract_area(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let left = area_coord(m, "LEFT")?;
    let top = area_coord(m, "TOP")?;
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");

    // @doc-snippet:begin command=extract_area slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=extract_area slot=load

    // @doc-snippet:begin command=extract_area slot=apply
    // @doc-test: extract.rs::extract_area_copies_the_rectangle:1232 repo=libviprs
    let out = raster.try_extract_area(left, top, width, height)?;
    // @doc-snippet:end command=extract_area slot=apply

    // @doc-snippet:begin command=extract_area slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=extract_area slot=save
    Ok(())
}

/// `embed IN OUT X Y WIDTH HEIGHT --extend --background` — S1 canvas placement.
fn run_embed(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let x = *m.get_one::<i32>("X").expect("required");
    let y = *m.get_one::<i32>("Y").expect("required");
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");
    let extend = extend_of(m)?;
    let background = background_of(m)?;

    // @doc-snippet:begin command=embed slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=embed slot=load

    // @doc-snippet:begin command=embed slot=apply
    // @doc-test: extract.rs::embed_black_places_and_fills:1297 repo=libviprs
    let out = raster.try_embed(x, y, width, height, extend, background.as_deref())?;
    // @doc-snippet:end command=embed slot=apply

    // @doc-snippet:begin command=embed slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=embed slot=save
    Ok(())
}

/// `gravity IN OUT DIRECTION WIDTH HEIGHT --extend --background` — S1 placement.
fn run_gravity(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let direction: CompassDirection = pos(m, "DIRECTION")
        .parse()
        .map_err(|e| anyhow!("invalid direction: {e}"))?;
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");
    let extend = extend_of(m)?;
    let background = background_of(m)?;

    // @doc-snippet:begin command=gravity slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=gravity slot=load

    // @doc-snippet:begin command=gravity slot=apply
    // @doc-test: extract.rs::gravity_places_at_all_nine_positions:1416 repo=libviprs
    let out = raster.try_gravity(direction, width, height, extend, background.as_deref())?;
    // @doc-snippet:end command=gravity slot=apply

    // @doc-snippet:begin command=gravity slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=gravity slot=save
    Ok(())
}

/// `replicate IN OUT ACROSS DOWN` — S1 tiling.
fn run_replicate(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let across = *m.get_one::<u32>("ACROSS").expect("required");
    let down = *m.get_one::<u32>("DOWN").expect("required");

    // @doc-snippet:begin command=replicate slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=replicate slot=load

    // @doc-snippet:begin command=replicate slot=apply
    // @doc-test: extract.rs::replicate_tiles_across_and_down:1487 repo=libviprs
    let out = raster.try_replicate(across, down)?;
    // @doc-snippet:end command=replicate slot=apply

    // @doc-snippet:begin command=replicate slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=replicate slot=save
    Ok(())
}

/// `zoom IN OUT XFAC YFAC` — S1 integer upscale.
fn run_zoom(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let xfac = *m.get_one::<u32>("XFAC").expect("required");
    let yfac = *m.get_one::<u32>("YFAC").expect("required");

    // @doc-snippet:begin command=zoom slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=zoom slot=load

    // @doc-snippet:begin command=zoom slot=apply
    // @doc-test: extract.rs::zoom_replicates_pixels:1599 repo=libviprs
    let out = raster.try_zoom(xfac, yfac)?;
    // @doc-snippet:end command=zoom slot=apply

    // @doc-snippet:begin command=zoom slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=zoom slot=save
    Ok(())
}

/// `subsample IN OUT XFAC YFAC` — S1 integer point-decimation.
fn run_subsample(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let xfac = *m.get_one::<u32>("XFAC").expect("required");
    let yfac = *m.get_one::<u32>("YFAC").expect("required");

    // @doc-snippet:begin command=subsample slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=subsample slot=load

    // @doc-snippet:begin command=subsample slot=apply
    // @doc-test: extract.rs::subsample_takes_the_top_left_of_each_cell:1625 repo=libviprs
    let out = raster.try_subsample(xfac, yfac)?;
    // @doc-snippet:end command=subsample slot=apply

    // @doc-snippet:begin command=subsample slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=subsample slot=save
    Ok(())
}

/// `insert MAIN SUB OUT X Y --expand` — S2 (two fixed inputs) composite.
fn run_insert(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let main_path = PathBuf::from(pos(m, "MAIN"));
    let sub_path = PathBuf::from(pos(m, "SUB"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let x = *m.get_one::<i32>("X").expect("required");
    let y = *m.get_one::<i32>("Y").expect("required");
    let expand = m.get_flag("expand");
    let background = background_of(m)?;

    // @doc-snippet:begin command=insert slot=load imports=decode_file
    let main = io::load(&main_path, &limits)?;
    let sub = io::load(&sub_path, &limits)?;
    // @doc-snippet:end command=insert slot=load

    // @doc-snippet:begin command=insert slot=apply
    // @doc-test: extract.rs::insert_without_expand_keeps_the_main_size:1514 repo=libviprs
    let out = main.try_insert(&sub, x, y, expand, background.as_deref())?;
    // @doc-snippet:end command=insert slot=apply

    // @doc-snippet:begin command=insert slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=insert slot=save
    Ok(())
}

/// `smartcrop IN OUT WIDTH HEIGHT --interesting --premultiplied` — S1 crop with
/// a printed attention centre (`x y` to stdout, mirroring vips's `attention-x` /
/// `attention-y` outputs).
fn run_smartcrop(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");
    let interesting = interesting_of(m)?;
    let premultiplied = m.get_flag("premultiplied");

    // @doc-snippet:begin command=smartcrop slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=smartcrop slot=load

    // @doc-snippet:begin command=smartcrop slot=apply
    // @doc-test: extract.rs::smartcrop_centre_low_high_are_pure_geometry:1664 repo=libviprs
    let (out, ax, ay) = raster.try_smartcrop(width, height, interesting, premultiplied)?;
    // @doc-snippet:end command=smartcrop slot=apply

    // @doc-snippet:begin command=smartcrop slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=smartcrop slot=save
    // The attention centre (0 0 for every non-attention strategy), mirroring
    // vips's optional attention-x / attention-y outputs.
    println!("{ax} {ay}");
    Ok(())
}

/// Map the `--interesting` string to the core [`SmartcropInteresting`] enum.
/// clap restricts the value to [`INTERESTING`], so the fallthrough is
/// unreachable in practice.
fn interesting_of(m: &ArgMatches) -> Result<SmartcropInteresting> {
    Ok(
        match m.get_one::<String>("interesting").map(String::as_str) {
            Some("centre") => SmartcropInteresting::Centre,
            Some("entropy") => SmartcropInteresting::Entropy,
            Some("attention") => SmartcropInteresting::Attention,
            Some("low") => SmartcropInteresting::Low,
            Some("high") => SmartcropInteresting::High,
            Some("all") => SmartcropInteresting::All,
            other => bail!("unknown interesting strategy {other:?}"),
        },
    )
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
        assert_eq!(meta_names.len(), 9, "the extract family has nine commands");
    }

    #[test]
    fn extract_area_parses_positionals_in_vips_order() {
        let m = cmd("extract_area")
            .try_get_matches_from(["extract_area", "in.png", "out.png", "3", "4", "5", "6"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(area_coord(&m, "LEFT").unwrap(), 3);
        assert_eq!(area_coord(&m, "TOP").unwrap(), 4);
        assert_eq!(*m.get_one::<u32>("WIDTH").unwrap(), 5);
        assert_eq!(*m.get_one::<u32>("HEIGHT").unwrap(), 6);
    }

    #[test]
    fn crop_is_registered_as_its_own_command() {
        // `crop` is a distinct clap command (vips-callable alias) sharing the
        // extract_area positional layout and handler.
        let m = cmd("crop")
            .try_get_matches_from(["crop", "in.png", "out.png", "1", "2", "3", "4"])
            .unwrap();
        assert_eq!(area_coord(&m, "LEFT").unwrap(), 1);
        assert_eq!(*m.get_one::<u32>("HEIGHT").unwrap(), 4);
    }

    #[test]
    fn extract_area_negative_coord_is_error_not_panic() {
        // vips's LEFT/TOP admit the signed gint range at parse; the core geometry
        // is u32-only, so a negative coordinate must become a typed exit-1 error,
        // NEVER a u32::try_from panic/abort (exit 101) — CLI_CONTRACT.md §8. The
        // coordinate is range-checked before any image load, so a dummy path is
        // fine.
        let m = cmd("extract_area")
            .try_get_matches_from(["extract_area", "in.png", "out.png", "-1", "0", "2", "2"])
            .unwrap();
        let err = run_extract_area(&m).unwrap_err();
        assert!(
            err.to_string().contains("must be >= 0"),
            "expected a typed negative-coordinate error, got: {err}"
        );
    }

    #[test]
    fn extract_area_coord_below_vips_minimum_is_rejected() {
        // vips's minimum LEFT is -1e8; the value_parser rejects anything lower at
        // parse time (strict parity — no sub-`-1e8` extension).
        assert!(
            cmd("extract_area")
                .try_get_matches_from([
                    "extract_area",
                    "in.png",
                    "out.png",
                    "-100000001",
                    "0",
                    "2",
                    "2",
                ])
                .is_err(),
            "a LEFT below -1e8 must be rejected at parse"
        );
    }

    #[test]
    fn crop_dims_reject_above_vips_maximum() {
        // vips caps extract_area/crop/smartcrop WIDTH/HEIGHT at 1e8 (NOT the 1e9
        // canvas cap embed/gravity use); the value_parser rejects 1e8+1 at parse
        // (strict parity — no 10x over-permissive leak).
        for (name, args) in [
            (
                "extract_area",
                [
                    "extract_area",
                    "in.png",
                    "out.png",
                    "0",
                    "0",
                    "100000001",
                    "2",
                ],
            ),
            (
                "smartcrop",
                ["smartcrop", "in.png", "out.png", "100000001", "8", "x", "y"],
            ),
        ] {
            // smartcrop takes only WIDTH HEIGHT (4 tokens); truncate its args.
            let argv: Vec<&str> = if name == "smartcrop" {
                vec!["smartcrop", "in.png", "out.png", "100000001", "8"]
            } else {
                args.to_vec()
            };
            assert!(
                cmd(name).try_get_matches_from(argv).is_err(),
                "{name}: a WIDTH above 1e8 must be rejected at parse"
            );
        }
    }

    #[test]
    fn embed_canvas_dim_allows_above_extract_maximum() {
        // The 1e9 canvas cap is embed/gravity-only: a WIDTH of 1e8+1 that
        // extract_area/smartcrop reject is still valid for embed's canvas.
        let m = cmd("embed")
            .try_get_matches_from(["embed", "in.png", "out.png", "0", "0", "100000001", "24"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("WIDTH").unwrap(), 100_000_001);
    }

    #[test]
    fn insert_position_rejects_beyond_vips_bounds() {
        // vips's insert x/y are the tighter ±1e8 (NOT embed's ±1e9); the
        // value_parser rejects anything outside at parse.
        for x in ["-100000001", "100000001", "500000000"] {
            assert!(
                cmd("insert")
                    .try_get_matches_from(["insert", "main.png", "sub.png", "out.png", x, "0",])
                    .is_err(),
                "insert X of {x} must be rejected (outside vips's ±1e8)"
            );
        }
        // embed's ±1e9 must still accept the same magnitude that insert rejects.
        assert!(
            cmd("embed")
                .try_get_matches_from(["embed", "in.png", "out.png", "500000000", "0", "4", "4",])
                .is_ok(),
            "embed X of 5e8 is inside embed's ±1e9 and must be accepted"
        );
    }

    #[test]
    fn dimensions_reject_zero() {
        // vips's WIDTH/HEIGHT minimum is 1; clap rejects 0 at parse.
        assert!(
            cmd("extract_area")
                .try_get_matches_from(["extract_area", "in.png", "out.png", "0", "0", "0", "2"])
                .is_err(),
            "a zero WIDTH must be rejected"
        );
    }

    #[test]
    fn factors_reject_zero() {
        for (name, args) in [
            ("replicate", ["replicate", "in.png", "out.png", "0", "1"]),
            ("zoom", ["zoom", "in.png", "out.png", "1", "0"]),
            ("subsample", ["subsample", "in.png", "out.png", "0", "1"]),
        ] {
            assert!(
                cmd(name).try_get_matches_from(args).is_err(),
                "{name}: a zero factor must be rejected"
            );
        }
    }

    #[test]
    fn embed_parses_signed_position_and_flags() {
        let m = cmd("embed")
            .try_get_matches_from([
                "embed", "in.png", "out.png", "-2", "3", "24", "24", "--extend", "copy",
            ])
            .unwrap();
        assert_eq!(*m.get_one::<i32>("X").unwrap(), -2);
        assert_eq!(*m.get_one::<i32>("Y").unwrap(), 3);
        assert_eq!(*m.get_one::<u32>("WIDTH").unwrap(), 24);
        assert_eq!(extend_of(&m).unwrap(), Extend::Copy);
        assert!(background_of(&m).unwrap().is_none());
    }

    #[test]
    fn embed_extend_defaults_to_black_and_enforces_the_enum() {
        let m = cmd("embed")
            .try_get_matches_from(["embed", "in.png", "out.png", "0", "0", "4", "4"])
            .unwrap();
        assert_eq!(extend_of(&m).unwrap(), Extend::Black);
        assert!(
            cmd("embed")
                .try_get_matches_from([
                    "embed", "in.png", "out.png", "0", "0", "4", "4", "--extend", "bogus"
                ])
                .is_err(),
            "only the six VipsExtend nicknames are accepted"
        );
    }

    #[test]
    fn embed_background_vector_is_parsed() {
        let m = cmd("embed")
            .try_get_matches_from([
                "embed",
                "in.png",
                "out.png",
                "0",
                "0",
                "4",
                "4",
                "--extend",
                "background",
                "--background",
                "10 20 30",
            ])
            .unwrap();
        assert_eq!(background_of(&m).unwrap(), Some(vec![10.0, 20.0, 30.0]));
    }

    #[test]
    fn gravity_enforces_the_compass_enum() {
        let m = cmd("gravity")
            .try_get_matches_from(["gravity", "in.png", "out.png", "south-east", "24", "24"])
            .unwrap();
        let dir: CompassDirection = pos(&m, "DIRECTION").parse().unwrap();
        assert_eq!(dir, CompassDirection::SouthEast);
        assert!(
            cmd("gravity")
                .try_get_matches_from(["gravity", "in.png", "out.png", "middle", "24", "24"])
                .is_err(),
            "only the nine VipsCompassDirection nicknames are accepted"
        );
    }

    #[test]
    fn insert_parses_three_paths_and_signed_position() {
        let m = cmd("insert")
            .try_get_matches_from([
                "insert", "main.png", "sub.png", "out.png", "-4", "5", "--expand",
            ])
            .unwrap();
        assert_eq!(pos(&m, "MAIN"), "main.png");
        assert_eq!(pos(&m, "SUB"), "sub.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(*m.get_one::<i32>("X").unwrap(), -4);
        assert_eq!(*m.get_one::<i32>("Y").unwrap(), 5);
        assert!(m.get_flag("expand"));
    }

    #[test]
    fn insert_expand_defaults_to_false() {
        let m = cmd("insert")
            .try_get_matches_from(["insert", "main.png", "sub.png", "out.png", "0", "0"])
            .unwrap();
        assert!(!m.get_flag("expand"));
    }

    #[test]
    fn smartcrop_interesting_defaults_to_attention_and_enforces_the_enum() {
        let m = cmd("smartcrop")
            .try_get_matches_from(["smartcrop", "in.png", "out.png", "8", "8"])
            .unwrap();
        assert_eq!(interesting_of(&m).unwrap(), SmartcropInteresting::Attention);
        assert!(!m.get_flag("premultiplied"));
        // vips's `none` strategy is intentionally excluded (not core-backed).
        assert!(
            cmd("smartcrop")
                .try_get_matches_from([
                    "smartcrop",
                    "in.png",
                    "out.png",
                    "8",
                    "8",
                    "--interesting",
                    "none"
                ])
                .is_err(),
            "the non-core-backed `none` strategy must be rejected"
        );
    }

    #[test]
    fn smartcrop_parses_all_core_strategies() {
        for (s, want) in [
            ("centre", SmartcropInteresting::Centre),
            ("entropy", SmartcropInteresting::Entropy),
            ("attention", SmartcropInteresting::Attention),
            ("low", SmartcropInteresting::Low),
            ("high", SmartcropInteresting::High),
            ("all", SmartcropInteresting::All),
        ] {
            let m = cmd("smartcrop")
                .try_get_matches_from([
                    "smartcrop",
                    "in.png",
                    "out.png",
                    "8",
                    "8",
                    "--interesting",
                    s,
                ])
                .unwrap();
            assert_eq!(interesting_of(&m).unwrap(), want, "strategy {s}");
        }
    }

    #[test]
    fn parse_f64_vec_reads_a_space_separated_vector() {
        assert_eq!(parse_f64_vec("128").unwrap(), vec![128.0]);
        assert_eq!(parse_f64_vec("10 20 30").unwrap(), vec![10.0, 20.0, 30.0]);
        assert!(parse_f64_vec("   ").is_err(), "an empty vector is an error");
        assert!(parse_f64_vec("10 x").is_err(), "a non-number is an error");
    }
}
