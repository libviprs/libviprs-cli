//! Draw op family — the Wave-2 **draw** lane (`CLI_CONTRACT.md` §3.6/§5/§6,
//! `OP_MAP.md` draw section).
//!
//! The core `src/draw.rs` exposes 28 `pub fn` that fold to nine base ops; two
//! (`draw` the generic `DrawOp` dispatcher and `put_pixel`) are EXCLUDED, so the
//! family contributes **seven** `viprs` subcommands. Every one is oracle class
//! **GOLDEN-ONLY**: `vips draw_*` are in-place mutators whose CLI *discards* the
//! mutated image (the op returns nothing to save), so there is **no vips CLI
//! oracle** to cross-check against. Each reference is therefore generated ONCE by
//! `viprs` itself (deterministic) and committed, and the differential cell
//! (`tests/cli_draw_diff.rs`) is a **regression pin** that says so — NOT a
//! cross-implementation parity claim.
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `draw_circle IN OUT CX CY RADIUS --ink … [--fill]`      | `draw_circle` | S6 | GOLDEN-ONLY | `--fill` folds `draw_circle_filled` |
//! | `draw_rect IN OUT LEFT TOP WIDTH HEIGHT --ink … [--fill]` | `draw_rect` | S6 | GOLDEN-ONLY | `--fill` folds `draw_rect_filled` |
//! | `draw_line IN OUT X1 Y1 X2 Y2 --ink …`                  | `draw_line`   | S6 | GOLDEN-ONLY | 1px inclusive line |
//! | `draw_flood IN OUT X Y --ink … [--equal]`               | `draw_flood`  | S6 | GOLDEN-ONLY | `--equal` folds `draw_flood_blob` |
//! | `draw_mask IN OUT MASK X Y --ink …`                     | `draw_mask`   | S6 | GOLDEN-ONLY | mask is a Gray8 stencil FILE (vips order `image ink mask x y`) |
//! | `draw_smudge IN OUT LEFT TOP WIDTH HEIGHT`              | `draw_smudge` | S6 | GOLDEN-ONLY | no ink (box-blur a rect) |
//! | `draw_image IN OUT SUB X Y`                             | `draw_image`  | S6 | GOLDEN-ONLY | no ink (paste `sub`; vips's `--mode` set/add not core-backed) |
//!
//! Shape **S6** (`CLI_CONTRACT.md` §3.6, the documented deviation): a draw op
//! loads `IN`, mutates it in place, and saves the result to `OUT` — `viprs`
//! *materialises* the mutation the vips CLI throws away. Positional orders, flag
//! names, and input value bounds mirror vips 8.18.4 exactly (verified against
//! `vips draw_* --help`): the signed coordinates are clamped to vips's declared
//! `±1e9` gint range, `radius` and the flood seed to vips's `0..` minimum, and
//! `draw_image`'s `--mode` is intentionally NOT exposed (the core does a plain
//! "set"-mode paste, `OP_MAP.md`).
//!
//! **Known limitation — unbounded compute on large in-range extents (core, not
//! this lane).** To keep vips parity the parser accepts vips's full range
//! (`±1e9` coords, `0..1e9` radius), but the core shape ops iterate the *whole*
//! shape bbox and clip only per-pixel inside `put_pixel`: `Rectangle::apply`
//! (fill) is `O(width*height)`, `fill_circle`/`outline_circle` are `O(radius^2)`
//! / `O(radius)`, and `Line::apply` is `O(extent)` — so a single valid argument
//! like `draw_circle IN OUT 16 16 1000000000 --ink … --fill` does not terminate
//! in reasonable time even though almost none of the shape lands on a small
//! canvas. vips clips each scanline to the image first (`O(radius)`). The fix is
//! to clip the iterated ranges to the canvas ∩ shape bbox before the `put_pixel`
//! loops in `libviprs/src/draw.rs` (the pattern `Smudge::apply` / `Paste::apply`
//! already use). That is a **core** change, out of scope for this CLI lane
//! (which only touches `src/ops/draw.rs`); filed as a core issue (see the lane
//! report). The CLI deliberately does NOT clamp the extent itself: clamping a
//! `radius`/`width` would change *which* pixels are painted (an off-canvas arc
//! still crosses the canvas), breaking the golden pins — the honest fix is the
//! core clip-first, which is output-preserving.
//!
//! Handlers keep the §3 `load → mutate → save` shape and call only the
//! panic-free `try_draw_*` core APIs where one exists (`draw_circle`,
//! `draw_rect`, `draw_line`, `draw_mask`) or the inherently-fallible
//! `draw_flood` / `draw_flood_blob`. The two infallible ops (`draw_smudge`,
//! `draw_image`) take no ink; `draw_smudge` decodes samples as unsigned integers
//! and would panic on a float raster, so the handler REJECTS a float target with
//! a typed exit-1 error up front (never an abort — the bands B2 lesson). Ink is
//! built from the `--ink "r g b"` vector to exactly the target's pixel width; a
//! wrong channel count or an out-of-range / float channel value is a typed
//! exit-1 error, not a panic (16-bit ink byte order is `CLI_CONTRACT.md` §9
//! missing-scope).
//
// @doc-command:begin name=draw_circle about="Draw a circle outline (or a filled disc with --fill) on an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_circle
// @doc-command:begin name=draw_rect about="Draw a rectangle outline (or a filled rectangle with --fill) on an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_rect
// @doc-command:begin name=draw_line about="Draw a 1px line between two points on an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_line
// @doc-command:begin name=draw_flood about="Flood-fill a region of an image from a seed point." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_flood
// @doc-command:begin name=draw_mask about="Paint ink through a single-band 8-bit stencil onto an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_mask
// @doc-command:begin name=draw_smudge about="Box-blur a rectangular region of an image in place." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_smudge
// @doc-command:begin name=draw_image about="Paste a sub-image into an image at a given offset." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=draw_image

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use libviprs::Raster;

use super::{CommandMeta, OracleClass, Shape, io};

/// vips 8.18.4's signed-coordinate gint bound (`min: -1e9, max: 1e9`) for the
/// `draw_circle`/`draw_rect`/`draw_line`/`draw_mask`/`draw_smudge`/`draw_image`
/// position and size arguments. Restricting the parser to vips's declared range
/// (rather than the full `i32`) keeps strict parity — no wider-than-vips extent.
const COORD_MIN: i64 = -1_000_000_000;
/// Upper end of the vips signed-coordinate gint bound.
const COORD_MAX: i64 = 1_000_000_000;
/// vips's non-negative bound (`min: 0, max: 1e9`) for `radius` and the
/// `draw_flood` seed coordinates.
const NONNEG_MAX: i64 = 1_000_000_000;

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6). Every
/// draw op is Shape::Draw (S6) and OracleClass::GoldenOnly (§5).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "draw_circle",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "draw_rect",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "draw_line",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "draw_flood",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "draw_mask",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "draw_smudge",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "draw_image",
            shape: Shape::Draw,
            oracle_class: OracleClass::GoldenOnly,
        },
    ]
}

/// A signed draw coordinate/size argument (`i32`, vips's `±1e9` gint range).
fn coord_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(i32).range(COORD_MIN..=COORD_MAX))
        .help(help)
}

/// A non-negative draw argument (`i32`, vips's `0..=1e9` range): `radius` and
/// the `draw_flood` seed coordinates.
fn nonneg_arg(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_parser(value_parser!(i32).range(0..=NONNEG_MAX))
        .help(help)
}

/// The shared `--ink "r g b"` flag: the pixel value to paint, one value per band
/// of the target image (`CLI_CONTRACT.md` §3.6). Required for every ink-bearing
/// draw op.
fn ink_arg() -> Arg {
    Arg::new("ink")
        .long("ink")
        .required(true)
        .value_name("\"r g b\"")
        .help("Colour to paint, space-separated, one value per band of IN (e.g. \"255 0 0\")")
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads an image, so each carries the shared decode-limit flags
/// via [`io::with_decode_limit_args`]. `IN OUT` lead (S6 materialises the
/// mutated image to `OUT`); the op-specific coordinates follow in vips order.
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("draw_circle")
                .about("Draw a circle outline (or a filled disc with --fill) on an image.")
                // vips's CX/CY admit negative values (±1e9); let clap read a
                // leading-dash coordinate as a value rather than a flag.
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(coord_arg("CX", "Centre x coordinate"))
                .arg(coord_arg("CY", "Centre y coordinate"))
                .arg(nonneg_arg("RADIUS", "Radius in pixels (>= 0)"))
                .arg(ink_arg())
                .arg(
                    Arg::new("fill")
                        .long("fill")
                        .action(ArgAction::SetTrue)
                        .help("Fill the disc instead of drawing only the outline"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("draw_rect")
                .about("Draw a rectangle outline (or a filled rectangle with --fill) on an image.")
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(coord_arg("LEFT", "Left edge x coordinate"))
                .arg(coord_arg("TOP", "Top edge y coordinate"))
                .arg(coord_arg("WIDTH", "Rectangle width in pixels"))
                .arg(coord_arg("HEIGHT", "Rectangle height in pixels"))
                .arg(ink_arg())
                .arg(
                    Arg::new("fill")
                        .long("fill")
                        .action(ArgAction::SetTrue)
                        .help("Fill the rectangle instead of drawing only the outline"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("draw_line")
                .about("Draw a 1px line between two points on an image.")
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(coord_arg("X1", "Start x coordinate"))
                .arg(coord_arg("Y1", "Start y coordinate"))
                .arg(coord_arg("X2", "End x coordinate"))
                .arg(coord_arg("Y2", "End y coordinate"))
                .arg(ink_arg()),
        ),
        io::with_decode_limit_args(
            Command::new("draw_flood")
                .about("Flood-fill a region of an image from a seed point.")
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(nonneg_arg("X", "Seed x coordinate (>= 0)"))
                .arg(nonneg_arg("Y", "Seed y coordinate (>= 0)"))
                .arg(ink_arg())
                .arg(
                    Arg::new("equal")
                        .long("equal")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Flood the region of pixels EQUAL to the seed value \
                             (vips draw_flood_blob) instead of flooding until the \
                             ink-coloured boundary",
                        ),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("draw_mask")
                .about("Paint ink through a single-band 8-bit stencil onto an image.")
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(Arg::new("MASK").required(true).value_name("MASK.png").help(
                    "Single-band 8-bit stencil image (each pixel is the ink opacity 0..=255)",
                ))
                .arg(coord_arg("X", "Target x of the mask's left edge"))
                .arg(coord_arg("Y", "Target y of the mask's top edge"))
                .arg(ink_arg()),
        ),
        io::with_decode_limit_args(
            Command::new("draw_smudge")
                .about("Box-blur a rectangular region of an image in place.")
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(coord_arg("LEFT", "Left edge x coordinate"))
                .arg(coord_arg("TOP", "Top edge y coordinate"))
                .arg(coord_arg("WIDTH", "Rectangle width in pixels"))
                .arg(coord_arg("HEIGHT", "Rectangle height in pixels")),
        ),
        io::with_decode_limit_args(
            Command::new("draw_image")
                .about("Paste a sub-image into an image at a given offset.")
                .allow_negative_numbers(true)
                .arg(Arg::new("IN").required(true).help("Input image"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("SUB")
                        .required(true)
                        .value_name("SUB.png")
                        .help("Sub-image to paste (must share IN's pixel format)"),
                )
                .arg(coord_arg("X", "Target x of the sub-image's left edge"))
                .arg(coord_arg("Y", "Target y of the sub-image's top edge")),
        ),
    ]
}

/// Dispatch a matched draw subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "draw_circle" => run_draw_circle(m),
        "draw_rect" => run_draw_rect(m),
        "draw_line" => run_draw_line(m),
        "draw_flood" => run_draw_flood(m),
        "draw_mask" => run_draw_mask(m),
        "draw_smudge" => run_draw_smudge(m),
        "draw_image" => run_draw_image(m),
        other => bail!("draw family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Read a required `i32` coordinate/size argument.
fn coord(m: &ArgMatches, id: &str) -> i32 {
    *m.get_one::<i32>(id)
        .expect("clap guarantees a required i32 argument is present")
}

/// Build the ink byte vector for `raster` from the `--ink "r g b"` flag.
///
/// The core paints ink as one whole pixel of
/// [`bytes_per_pixel`](libviprs::PixelFormat::bytes_per_pixel) bytes, so the
/// vector must carry exactly one value per band, each encoded to the target's
/// channel width. A wrong band count, an out-of-range channel value, or a
/// non-8/16-bit (float) target is a typed exit-1 error rather than a panic or a
/// silently-corrupted channel (the core `check_ink` would otherwise reject a
/// mis-sized vector, and a shorter ink would have cycled) — see
/// `CLI_CONTRACT.md` §8. 16-bit ink byte order is `CLI_CONTRACT.md` §9
/// missing-scope; this encodes native-endian and says so.
///
/// # Errors
///
/// Errors if a value is not an integer, the count does not equal the raster's
/// band count, a value is out of range for the channel width, or the target is a
/// float raster (unsupported ink model).
fn build_ink(raster: &Raster, ink_str: &str) -> Result<Vec<u8>> {
    let fmt = raster.format();
    if fmt.is_float() {
        bail!(
            "draw --ink is not supported on the float raster {:?}; the ink model paints \
             unsigned 8/16-bit channels — cast to an unsigned integer format first",
            fmt
        );
    }
    let channels = fmt.channels();
    let bpc = fmt.bytes_per_channel();

    let values: Vec<i64> = ink_str
        .split_whitespace()
        .map(|t| {
            t.parse::<i64>()
                .map_err(|e| anyhow!("ink value {t:?} is not an integer: {e}"))
        })
        .collect::<Result<_>>()?;
    if values.len() != channels {
        bail!(
            "--ink has {} value(s) but the input image has {} band(s); supply exactly one \
             ink value per band",
            values.len(),
            channels
        );
    }

    let mut ink = Vec::with_capacity(channels * bpc);
    for &v in &values {
        match bpc {
            1 => {
                let byte = u8::try_from(v).map_err(|_| {
                    anyhow!("ink value {v} is out of range for an 8-bit band (0..=255)")
                })?;
                ink.push(byte);
            }
            2 => {
                let word = u16::try_from(v).map_err(|_| {
                    anyhow!("ink value {v} is out of range for a 16-bit band (0..=65535)")
                })?;
                // Native-endian: 16-bit ink byte order is CLI_CONTRACT.md §9
                // missing-scope (documented deviation), so this matches the
                // native-endian sample layout the decoder produces.
                ink.extend_from_slice(&word.to_ne_bytes());
            }
            other => bail!(
                "draw --ink does not support a {other}-byte channel width; only unsigned \
                 8/16-bit rasters are supported"
            ),
        }
    }
    Ok(ink)
}

/// `draw_circle IN OUT CX CY RADIUS --ink … [--fill]` — S6 GOLDEN-ONLY.
fn run_draw_circle(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let (cx, cy, radius) = (coord(m, "CX"), coord(m, "CY"), coord(m, "RADIUS"));
    let fill = m.get_flag("fill");

    // @doc-snippet:begin command=draw_circle slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=draw_circle slot=load

    let ink = build_ink(&raster, pos(m, "ink"))?;

    // @doc-snippet:begin command=draw_circle slot=apply
    if fill {
        raster.try_draw_circle_filled(&ink, cx, cy, radius)?;
    } else {
        raster.try_draw_circle(&ink, cx, cy, radius)?;
    }
    // @doc-snippet:end command=draw_circle slot=apply

    // @doc-snippet:begin command=draw_circle slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_circle slot=save
    Ok(())
}

/// `draw_rect IN OUT LEFT TOP WIDTH HEIGHT --ink … [--fill]` — S6 GOLDEN-ONLY.
fn run_draw_rect(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let (left, top) = (coord(m, "LEFT"), coord(m, "TOP"));
    let (width, height) = (coord(m, "WIDTH"), coord(m, "HEIGHT"));
    let fill = m.get_flag("fill");

    // @doc-snippet:begin command=draw_rect slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=draw_rect slot=load

    let ink = build_ink(&raster, pos(m, "ink"))?;

    // @doc-snippet:begin command=draw_rect slot=apply
    if fill {
        raster.try_draw_rect_filled(&ink, left, top, width, height)?;
    } else {
        raster.try_draw_rect(&ink, left, top, width, height)?;
    }
    // @doc-snippet:end command=draw_rect slot=apply

    // @doc-snippet:begin command=draw_rect slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_rect slot=save
    Ok(())
}

/// `draw_line IN OUT X1 Y1 X2 Y2 --ink …` — S6 GOLDEN-ONLY.
fn run_draw_line(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let (x1, y1) = (coord(m, "X1"), coord(m, "Y1"));
    let (x2, y2) = (coord(m, "X2"), coord(m, "Y2"));

    // @doc-snippet:begin command=draw_line slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=draw_line slot=load

    let ink = build_ink(&raster, pos(m, "ink"))?;

    // @doc-snippet:begin command=draw_line slot=apply
    raster.try_draw_line(&ink, x1, y1, x2, y2)?;
    // @doc-snippet:end command=draw_line slot=apply

    // @doc-snippet:begin command=draw_line slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_line slot=save
    Ok(())
}

/// `draw_flood IN OUT X Y --ink … [--equal]` — S6 GOLDEN-ONLY.
fn run_draw_flood(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let (x, y) = (coord(m, "X"), coord(m, "Y"));
    let equal = m.get_flag("equal");

    // @doc-snippet:begin command=draw_flood slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=draw_flood slot=load

    let ink = build_ink(&raster, pos(m, "ink"))?;

    // @doc-snippet:begin command=draw_flood slot=apply
    // The flood wrappers are inherently fallible (ink width + off-canvas seed);
    // `--equal` selects the "flood while equal to the seed value" blob variant.
    if equal {
        raster.draw_flood_blob(&ink, x, y)?;
    } else {
        raster.draw_flood(&ink, x, y)?;
    }
    // @doc-snippet:end command=draw_flood slot=apply

    // @doc-snippet:begin command=draw_flood slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_flood slot=save
    Ok(())
}

/// `draw_mask IN OUT MASK X Y --ink …` — S6 GOLDEN-ONLY.
fn run_draw_mask(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let mask_path = PathBuf::from(pos(m, "MASK"));
    let (x, y) = (coord(m, "X"), coord(m, "Y"));

    // @doc-snippet:begin command=draw_mask slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    let mask = io::load(&mask_path, &limits)?;
    // @doc-snippet:end command=draw_mask slot=load

    // The core `Mask::apply` paints NOTHING for any mask that is not a
    // single-band 8-bit stencil (`channels()!=1 || bytes_per_channel()!=1` is a
    // documented no-op — libviprs/src/draw.rs). Left unchecked, a 3-band or
    // 16-bit mask would write an UNCHANGED image and exit 0, a silent
    // wrong-output that also diverges from vips (`draw_mask_direct: image must
    // one band`). Reject it up front with a typed exit-1 error, mirroring
    // `run_draw_image`'s format-mismatch guard rather than silently no-op'ing.
    let mfmt = mask.format();
    if mfmt.channels() != 1 || mfmt.bytes_per_channel() != 1 {
        bail!(
            "draw_mask MASK must be a single-band 8-bit stencil but has format {:?}; the core \
             paints nothing for any other mask format — convert MASK to Gray8 first",
            mfmt
        );
    }

    let ink = build_ink(&raster, pos(m, "ink"))?;

    // @doc-snippet:begin command=draw_mask slot=apply
    // `try_draw_mask` rejects a float target with a typed error (the blend
    // decodes unsigned 8/16-bit samples); the single-band 8-bit stencil is
    // validated above (a non-conforming mask is a documented core no-op).
    raster.try_draw_mask(&ink, &mask, x, y)?;
    // @doc-snippet:end command=draw_mask slot=apply

    // @doc-snippet:begin command=draw_mask slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_mask slot=save
    Ok(())
}

/// `draw_smudge IN OUT LEFT TOP WIDTH HEIGHT` — S6 GOLDEN-ONLY (no ink).
fn run_draw_smudge(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let (left, top) = (coord(m, "LEFT"), coord(m, "TOP"));
    let (width, height) = (coord(m, "WIDTH"), coord(m, "HEIGHT"));

    // @doc-snippet:begin command=draw_smudge slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=draw_smudge slot=load

    // `draw_smudge` is infallible in the core but decodes samples as unsigned
    // integers: it would PANIC on a float raster (the sample-decode helper has no
    // float arm). Reject a float target with a typed exit-1 error up front rather
    // than letting the core abort (bands B2 — no panic on user input).
    if raster.format().is_float() {
        bail!(
            "draw_smudge is not supported on the float raster {:?}; the box-blur decodes \
             unsigned 8/16-bit samples — cast to an unsigned integer format first",
            raster.format()
        );
    }

    // @doc-snippet:begin command=draw_smudge slot=apply
    raster.draw_smudge(left, top, width, height);
    // @doc-snippet:end command=draw_smudge slot=apply

    // @doc-snippet:begin command=draw_smudge slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_smudge slot=save
    Ok(())
}

/// `draw_image IN OUT SUB X Y` — S6 GOLDEN-ONLY (no ink; paste `sub`).
fn run_draw_image(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let sub_path = PathBuf::from(pos(m, "SUB"));
    let (x, y) = (coord(m, "X"), coord(m, "Y"));

    // @doc-snippet:begin command=draw_image slot=load imports=decode_file
    let mut raster = io::load(&in_path, &limits)?;
    let sub = io::load(&sub_path, &limits)?;
    // @doc-snippet:end command=draw_image slot=load

    // The core paste is a documented no-op if `sub` does not share `IN`'s pixel
    // format (it leaves conversion to the caller). Surface that as a typed
    // exit-1 error rather than silently writing an unchanged image.
    if sub.format() != raster.format() {
        bail!(
            "draw_image SUB has pixel format {:?} but IN has {:?}; the paste requires an \
             identical format (band count + bit depth) — convert SUB first",
            sub.format(),
            raster.format()
        );
    }

    // @doc-snippet:begin command=draw_image slot=apply
    raster.draw_image(&sub, x, y);
    // @doc-snippet:end command=draw_image slot=apply

    // @doc-snippet:begin command=draw_image slot=save imports=save_file
    io::save(&raster, &out_path)?;
    // @doc-snippet:end command=draw_image slot=save
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
        for name in &meta_names {
            assert!(
                cmd_names.iter().any(|c| c == name),
                "meta {name} has no command"
            );
        }
        assert_eq!(cmd_names.len(), meta_names.len());
        assert_eq!(meta_names.len(), 7, "the draw family has seven commands");
    }

    #[test]
    fn every_meta_is_draw_golden() {
        for meta in metas() {
            assert_eq!(meta.shape, Shape::Draw, "{} must be Shape::Draw", meta.name);
            assert_eq!(
                meta.oracle_class,
                OracleClass::GoldenOnly,
                "{} must be GOLDEN-ONLY (draw ops have no vips oracle)",
                meta.name
            );
        }
    }

    #[test]
    fn draw_circle_parses_positionals_in_vips_order() {
        let m = cmd("draw_circle")
            .try_get_matches_from([
                "draw_circle",
                "in.png",
                "out.png",
                "10",
                "12",
                "5",
                "--ink",
                "255 0 0",
            ])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(coord(&m, "CX"), 10);
        assert_eq!(coord(&m, "CY"), 12);
        assert_eq!(coord(&m, "RADIUS"), 5);
        assert_eq!(pos(&m, "ink"), "255 0 0");
        assert!(!m.get_flag("fill"));
    }

    #[test]
    fn draw_circle_fill_flag_parses() {
        let m = cmd("draw_circle")
            .try_get_matches_from([
                "draw_circle",
                "in.png",
                "out.png",
                "10",
                "12",
                "5",
                "--ink",
                "255",
                "--fill",
            ])
            .unwrap();
        assert!(m.get_flag("fill"));
    }

    #[test]
    fn draw_circle_requires_ink() {
        // --ink is REQUIRED for the ink-bearing draw ops (S6).
        assert!(
            cmd("draw_circle")
                .try_get_matches_from(["draw_circle", "in.png", "out.png", "10", "12", "5"])
                .is_err(),
            "draw_circle must require --ink"
        );
    }

    #[test]
    fn draw_circle_rejects_negative_radius() {
        // vips's radius minimum is 0; the 0.. value_parser rejects a negative
        // radius at parse time (strict parity — no wider-than-vips extent).
        assert!(
            cmd("draw_circle")
                .try_get_matches_from([
                    "draw_circle",
                    "in.png",
                    "out.png",
                    "10",
                    "12",
                    "-1",
                    "--ink",
                    "255",
                ])
                .is_err(),
            "a negative RADIUS must be rejected"
        );
    }

    #[test]
    fn draw_line_accepts_negative_coordinates() {
        // vips's line endpoints admit negative coordinates (±1e9); a leading-dash
        // value must be read as a coordinate, not a flag (allow_negative_numbers).
        let m = cmd("draw_line")
            .try_get_matches_from([
                "draw_line",
                "in.png",
                "out.png",
                "-5",
                "-5",
                "40",
                "40",
                "--ink",
                "0",
            ])
            .unwrap();
        assert_eq!(coord(&m, "X1"), -5);
        assert_eq!(coord(&m, "Y1"), -5);
        assert_eq!(coord(&m, "X2"), 40);
    }

    #[test]
    fn draw_flood_rejects_negative_seed() {
        // vips's draw_flood seed minimum is 0.
        assert!(
            cmd("draw_flood")
                .try_get_matches_from([
                    "draw_flood",
                    "in.png",
                    "out.png",
                    "-1",
                    "0",
                    "--ink",
                    "255",
                ])
                .is_err(),
            "a negative seed X must be rejected"
        );
    }

    #[test]
    fn draw_coords_reject_beyond_vips_bound() {
        // vips's signed-coord max is 1e9; 1e9+1 is rejected at parse time.
        assert!(
            cmd("draw_rect")
                .try_get_matches_from([
                    "draw_rect",
                    "in.png",
                    "out.png",
                    "1000000001",
                    "0",
                    "5",
                    "5",
                    "--ink",
                    "255",
                ])
                .is_err(),
            "a LEFT beyond vips's 1e9 bound must be rejected"
        );
    }

    #[test]
    fn draw_mask_parses_mask_file_before_xy() {
        // vips order is `image ink mask x y`; the CLI puts MASK before X Y.
        let m = cmd("draw_mask")
            .try_get_matches_from([
                "draw_mask",
                "in.png",
                "out.png",
                "mask.png",
                "3",
                "4",
                "--ink",
                "255",
            ])
            .unwrap();
        assert_eq!(pos(&m, "MASK"), "mask.png");
        assert_eq!(coord(&m, "X"), 3);
        assert_eq!(coord(&m, "Y"), 4);
    }

    #[test]
    fn draw_smudge_takes_no_ink() {
        // draw_smudge has no --ink; supplying one is an unknown-argument error.
        assert!(
            cmd("draw_smudge")
                .try_get_matches_from([
                    "draw_smudge",
                    "in.png",
                    "out.png",
                    "1",
                    "1",
                    "4",
                    "4",
                    "--ink",
                    "255",
                ])
                .is_err(),
            "draw_smudge must not accept --ink"
        );
        assert!(
            cmd("draw_smudge")
                .try_get_matches_from(["draw_smudge", "in.png", "out.png", "1", "1", "4", "4"])
                .is_ok()
        );
    }

    #[test]
    fn draw_image_takes_no_ink_and_reads_sub() {
        let m = cmd("draw_image")
            .try_get_matches_from(["draw_image", "in.png", "out.png", "sub.png", "2", "3"])
            .unwrap();
        assert_eq!(pos(&m, "SUB"), "sub.png");
        assert_eq!(coord(&m, "X"), 2);
        assert_eq!(coord(&m, "Y"), 3);
    }

    // --- build_ink -----------------------------------------------------------

    fn raster(channels: usize, bpc: usize) -> Raster {
        Raster::zeroed(2, 2, PixelFormat::with_channels(channels, bpc).unwrap()).unwrap()
    }

    #[test]
    fn build_ink_encodes_one_byte_per_8bit_band() {
        let r = raster(3, 1);
        assert_eq!(build_ink(&r, "255 0 128").unwrap(), vec![255, 0, 128]);
    }

    #[test]
    fn build_ink_encodes_16bit_native_endian() {
        let r = raster(1, 2);
        let ink = build_ink(&r, "4096").unwrap();
        assert_eq!(ink, 4096u16.to_ne_bytes().to_vec());
    }

    #[test]
    fn build_ink_rejects_wrong_band_count() {
        let r = raster(3, 1);
        let err = build_ink(&r, "255 0").unwrap_err().to_string();
        assert!(err.contains("band"), "got: {err}");
    }

    #[test]
    fn build_ink_rejects_out_of_range_8bit_value() {
        // 256 does not fit an 8-bit band — a typed error, never a panic/abort.
        let r = raster(1, 1);
        let err = build_ink(&r, "256").unwrap_err().to_string();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn build_ink_rejects_negative_value() {
        let r = raster(1, 1);
        assert!(
            build_ink(&r, "-1").is_err(),
            "a negative ink value is rejected"
        );
    }

    #[test]
    fn build_ink_rejects_non_integer() {
        let r = raster(1, 1);
        let err = build_ink(&r, "12x").unwrap_err().to_string();
        assert!(err.contains("not an integer"), "got: {err}");
    }

    #[test]
    fn build_ink_rejects_float_target() {
        // A float raster has no unsigned-integer ink model: typed error.
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::zeroed(2, 2, fmt).unwrap();
        let err = build_ink(&r, "1").unwrap_err().to_string();
        assert!(err.contains("float"), "got: {err}");
    }
}
