//! The `.pvec` raster commands (libviprs-cli#130, RL1.4 of EPIC R-L, ADR 0018 in libviprs/pvec):
//!
//! * `viprs pyramid <image|pdf> <out.pvec> --storage pvec --pvec-into <drawing.pvec> ...`
//! * `viprs pmtiles to-pvec <archive.pmtiles> <out.pvec> --pvec-into <drawing.pvec> ...`
//! * `viprs pvec rasters <file.pvec> [--json | --tile ID LEVEL COL ROW OUT]`
//!
//! This file holds the arguments and every refusal the command line alone decides (exit 2, with a
//! hint naming the way out). Those run in every build, so a mistake is reported the same whether or
//! not the `pvec-raster` feature is on. What needs PVec's facade lives in `imp`, behind the
//! feature; a build without it exits 1 and names the feature, as `vector build` does for its
//! provider.
//!
//! The placement comes from one of two places, never both: `--pixel-to-source` gives PVec's
//! `[a b c d tx ty]` outright, and a pixel to geographic transform (`--geo-origin`/`--geo-scale`
//! or `--affine`, or the one a PMTiles archive stores) is composed with `--source-to-geo`, the
//! drawing's calibration. `--pvec-into` is required: an artifact needs an entity (WRT-01) and ADR
//! 0018 kept scan-only sheets out of v1, so a scan always joins a drawing.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use libviprs::GeoTransform;

use crate::{PyramidArgs, StorageArg, usage_error};

#[cfg(feature = "pvec-raster")]
mod imp;

/// The tile edges PVec accepts (RASTER-01).
const TILE_EDGES: [u32; 3] = [256, 512, 1024];

/// The longest attribution, in bytes (RASTER-07).
const MAX_ATTRIBUTION: usize = 255;

/// `viprs pvec`.
#[derive(Args)]
pub struct PvecArgs {
    #[command(subcommand)]
    command: PvecCommand,
}

#[derive(Subcommand)]
enum PvecCommand {
    /// List the rasters a `.pvec` places, or write one tile out.
    Rasters(RastersArgs),
}

/// `viprs pvec rasters`.
#[derive(Args)]
struct RastersArgs {
    /// The `.pvec` to read.
    file: PathBuf,

    /// Print `{"v": 1, "rasters": [...], "skipped": [...]}` instead of text.
    ///
    /// A raster is an object with `id`, `width`, `height`, `tile_edge`,
    /// `levels`, `codec`, `pixel_format`, `bit_depth`, `band`, `opacity`,
    /// `layer`, `attribution`, `pixel_to_source` and `visible_box`. A skipped
    /// raster has its `id`, the `rule` that refused it (`RASTER-NN`) and a
    /// `reason`.
    #[arg(long, conflicts_with = "tile")]
    json: bool,

    /// Write one tile's encoded bytes, untouched, to OUT (`-` for stdout)
    /// instead of listing: the raster's ID, then the level, column and row.
    #[arg(
        long,
        num_args = 5,
        value_names = ["ID", "LEVEL", "COL", "ROW", "OUT"],
    )]
    tile: Option<Vec<String>>,
}

/// `viprs pmtiles to-pvec`.
#[derive(Args)]
pub(crate) struct ToPvecArgs {
    /// The libviprs archive to pass through. It has to be a `--layout google`
    /// pyramid with no centring and no overlap, with PNG, JPEG or WebP tiles
    /// of 256, 512 or 1024 pixels.
    archive: PathBuf,

    /// The `.pvec` to write.
    output: PathBuf,

    /// The `.pvec` drawing the raster joins. The output is a copy of it with
    /// the raster added; the drawing itself is not touched. Required: an
    /// artifact needs at least one entity (WRT-01).
    #[arg(long, value_name = "DRAWING.PVEC")]
    pvec_into: Option<PathBuf>,

    /// The drawing's calibration as "a,b,c,d,e,f", source space to geographic.
    /// Composed with the transform the archive stores. The raster and the
    /// calibration have to share one CRS; no CRS is stored or checked.
    #[arg(
        long,
        value_name = "A,B,C,D,E,F",
        conflicts_with = "pixel_to_source",
        allow_hyphen_values = true
    )]
    source_to_geo: Option<String>,

    /// The placement given directly, pixels to source space, as
    /// "a,b,c,d,tx,ty" in PVec's order. Needed for an archive that stores no
    /// transform.
    #[arg(long, value_name = "A,B,C,D,TX,TY", allow_hyphen_values = true)]
    pixel_to_source: Option<String>,

    /// Which side of the linework the raster draws on: 0 under it (the
    /// default), 1 over it.
    #[arg(long, value_name = "0|1", value_parser = clap::value_parser!(u8).range(0..=1))]
    band: Option<u8>,

    /// How opaque the raster draws, 0 to 255 (default 255).
    #[arg(long, value_name = "0..255")]
    opacity: Option<u8>,

    /// Name of a layer to put the raster on; the output gets a layer table
    /// with this one entry.
    #[arg(long, value_name = "NAME")]
    layer: Option<String>,
}

/// Where a pixel lands in the drawing's source space.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) enum Spec {
    /// `[a b c d tx ty]`, as given.
    Explicit([f64; 6]),
    /// The raster's own transform composed with the drawing's calibration.
    Composed {
        /// The drawing's calibration, source space to geographic.
        source_to_geo: GeoTransform,
    },
}

/// The parts of a placement besides the map.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct Looks {
    /// 0 underlay, 1 overlay.
    pub(crate) band: u8,
    /// 255 is opaque.
    pub(crate) opacity: u8,
    /// Plain text, already checked.
    pub(crate) attribution: Option<String>,
    /// A one-row layer table's name.
    pub(crate) layer: Option<String>,
}

/// What `viprs pyramid --storage pvec` has settled before it reads anything.
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct Checked {
    pub(crate) into: PathBuf,
    pub(crate) output: PathBuf,
    /// The pixel to geographic transform, for a composed placement.
    pub(crate) pixel_to_geo: Option<GeoTransform>,
    pub(crate) spec: Spec,
    pub(crate) looks: Looks,
}

/// Exit 1, naming the feature this build left out.
#[cfg(not(feature = "pvec-raster"))]
fn missing_feature(what: &str) -> ! {
    crate::operational_error(&format!(
        "{what} needs a build with the `pvec-raster` feature (rebuild with \
         `--features pvec-raster`); `viprs features` lists what this binary has"
    ));
}

/// Parse `--{flag}` as exactly six finite numbers (exit 2 otherwise, through
/// the parser `viprs geo` uses).
fn six(text: &str, flag: &str) -> [f64; 6] {
    let v = crate::pdf_geo_plan::parse_floats(text, flag, 6);
    [v[0], v[1], v[2], v[3], v[4], v[5]]
}

/// A six-number flag as the core's transform, in `GeoTransform` order.
fn geo(text: &str, flag: &str) -> GeoTransform {
    let v = six(text, flag);
    GeoTransform::new(v[0], v[1], v[2], v[3], v[4], v[5])
}

/// The attribution rule that the command line alone decides: RASTER-07's length, and no control
/// character or separator that could break a line in a viewer. The core checks the same when it
/// builds the placement.
fn check_attribution(text: &str) {
    if text.len() > MAX_ATTRIBUTION {
        usage_error(
            &format!(
                "--attribution is {} bytes, and a raster takes at most {MAX_ATTRIBUTION}",
                text.len()
            ),
            "shorten it to a plain credit line",
        );
    }
    let bad = |c: char| {
        c.is_control()
            || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    };
    if text.chars().any(bad) {
        usage_error(
            "--attribution has a control character, a line separator or a bidirectional override",
            "an attribution is plain text on one line, so a viewer can show it without escaping",
        );
    }
}

/// The placement's non-map fields, with the attribution checked.
fn looks(
    band: Option<u8>,
    opacity: Option<u8>,
    attribution: Option<&String>,
    layer: Option<&String>,
) -> Looks {
    if let Some(text) = attribution {
        check_attribution(text);
    }
    Looks {
        band: band.unwrap_or(0),
        opacity: opacity.unwrap_or(255),
        attribution: attribution.cloned(),
        layer: layer.cloned(),
    }
}

/// `--pvec-into` is required: WRT-01 refuses an artifact with no entity.
fn require_into(into: Option<&PathBuf>, command: &str) -> PathBuf {
    let Some(into) = into else {
        usage_error(
            &format!("{command} needs --pvec-into: a .pvec needs at least one entity (WRT-01)"),
            "name the drawing the raster joins, as in `--pvec-into drawing.pvec`; ADR 0018 keeps \
             scan-only sheets out of v1",
        );
    };
    into.clone()
}

/// Whether `output` can only be a directory.
fn names_a_directory(path: &Path) -> bool {
    path.is_dir() || path.to_string_lossy().ends_with(std::path::MAIN_SEPARATOR)
}

/// Every refusal of `viprs pyramid --storage pvec` that the command line alone decides (exit 2).
pub(crate) fn check_pyramid(args: &PyramidArgs) -> Checked {
    let Some(output) = args.output.clone() else {
        usage_error(
            "--storage pvec has no output .pvec to write",
            "name it: `viprs pyramid scan.tif out.pvec --storage pvec --pvec-into drawing.pvec ...`",
        );
    };
    if names_a_directory(&output) {
        usage_error(
            &format!(
                "`{}` is a directory, and --storage pvec writes one file",
                output.display()
            ),
            "name the .pvec to write, as in `out.pvec`",
        );
    }
    if !TILE_EDGES.contains(&args.tile_size) {
        usage_error(
            &format!("--tile-size {} is not 256, 512 or 1024", args.tile_size),
            "PVec stores tiles of those edges only (RASTER-01); pick one of them",
        );
    }
    if args.centre {
        usage_error(
            "--centre cannot be combined with --storage pvec",
            "the grid of a raster in a .pvec is anchored at pixel (0,0) and never centred \
             (ADR 0018); drop --centre",
        );
    }
    if args.overlap != 0 {
        usage_error(
            &format!(
                "--overlap {} cannot be combined with --storage pvec",
                args.overlap
            ),
            "a raster in a .pvec has no overlap between tiles (RASTER-02); drop --overlap",
        );
    }
    if args.layout.is_some() && !matches!(args.layout, Some(crate::LayoutArg::Google)) {
        usage_error(
            "--layout is fixed to google under --storage pvec",
            "a .pvec stores the un-centred Google grid and nothing else; drop --layout (or \
             spell it `--layout google`)",
        );
    }
    if matches!(args.format, crate::FormatArg::Raw) {
        usage_error(
            "--format raw cannot be stored in a .pvec",
            "raw pixels carry neither dimensions nor a pixel format, and PVec stores encoded \
             tiles (RASTER-04); use `--format png`, `--format jpeg` or `--format webp`",
        );
    }
    if args.skip_blank || args.blank_tolerance.is_some() {
        usage_error(
            "--skip-blank and --blank-tolerance write a placeholder marker, which is not a tile",
            "use `--drop-blanks`, which leaves a blank tile out as a not-stored tile",
        );
    }
    let p = &args.pipeline;
    for (given, flag) in [
        (args.resume, "--resume"),
        (args.verify, "--verify"),
        (args.dedupe_blanks, "--dedupe-blanks"),
        (args.dedupe_all, "--dedupe-all"),
        (args.manifest_emit_checksums, "--manifest-emit-checksums"),
        (p.region.is_some(), "--region"),
        (p.object_store_root.is_some(), "--object-store-root"),
    ] {
        if given {
            usage_error(
                &format!("{flag} does not apply to --storage pvec"),
                "the output is one .pvec written in one go: there is nothing to resume, verify \
                 or deduplicate, and no region",
            );
        }
    }
    if let Some(flag) = p.pmtiles_only().or_else(|| p.tree_only()) {
        usage_error(
            &format!("{flag} does not apply to --storage pvec"),
            "it configures the PMTiles writer or a tile tree; drop it",
        );
    }

    let into = require_into(args.pvec_into.as_ref(), "--storage pvec");
    let pixel_to_geo = crate::build_geo_transform(args, 0, 0);
    let spec = match (
        pixel_to_geo,
        args.source_to_geo.as_deref(),
        args.pixel_to_source.as_deref(),
    ) {
        (_, _, Some(direct)) => Spec::Explicit(six(direct, "pixel-to-source")),
        (Some(_), Some(calibration), None) => Spec::Composed {
            source_to_geo: geo(calibration, "source-to-geo"),
        },
        (Some(_), None, None) => usage_error(
            "a pixel to geographic transform needs --source-to-geo to land in the drawing",
            "add the drawing's calibration as `--source-to-geo a,b,c,d,e,f`, or give the \
             placement directly with `--pixel-to-source a,b,c,d,tx,ty`",
        ),
        (None, Some(_), None) => usage_error(
            "--source-to-geo needs the raster's own transform",
            "add `--geo-origin x,y --geo-scale sx,sy` or `--affine a,b,c,d,e,f`, or give the \
             placement directly with `--pixel-to-source a,b,c,d,tx,ty`",
        ),
        (None, None, None) => usage_error(
            "--storage pvec has no placement",
            "give `--pixel-to-source a,b,c,d,tx,ty`, or a pixel to geographic transform \
             (`--geo-origin`/`--geo-scale` or `--affine`) with `--source-to-geo a,b,c,d,e,f`",
        ),
    };
    Checked {
        into,
        output,
        pixel_to_geo,
        spec,
        looks: looks(
            args.band,
            args.opacity,
            args.attribution.as_ref(),
            args.layer.as_ref(),
        ),
    }
}

/// A flag of the `--storage pvec` family on a run that is not going to a `.pvec` is a mistake,
/// not something to ignore.
pub(crate) fn refuse_stray_flags(args: &PyramidArgs) {
    if args.storage == StorageArg::Pvec {
        return;
    }
    for (given, flag) in [
        (args.pvec_into.is_some(), "--pvec-into"),
        (args.source_to_geo.is_some(), "--source-to-geo"),
        (args.pixel_to_source.is_some(), "--pixel-to-source"),
        (args.band.is_some(), "--band"),
        (args.opacity.is_some(), "--opacity"),
        (args.attribution.is_some(), "--attribution"),
        (args.layer.is_some(), "--layer"),
    ] {
        if given {
            usage_error(
                &format!("{flag} only applies to --storage pvec"),
                "add `--storage pvec`, or drop the flag",
            );
        }
    }
}

/// `viprs pyramid --storage pvec`.
pub(crate) fn run_pyramid(args: PyramidArgs, cancel: libviprs::CancelToken) {
    let checked = check_pyramid(&args);
    #[cfg(feature = "pvec-raster")]
    imp::pyramid(args, checked, cancel);
    #[cfg(not(feature = "pvec-raster"))]
    {
        let _ = (checked, cancel);
        missing_feature("`viprs pyramid --storage pvec`");
    }
}

/// What `viprs pmtiles to-pvec` has settled before it opens the archive.
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct CheckedToPvec {
    pub(crate) into: PathBuf,
    /// `Some` when `--pixel-to-source` gave the map outright.
    pub(crate) explicit: Option<[f64; 6]>,
    pub(crate) source_to_geo: Option<GeoTransform>,
    pub(crate) looks: Looks,
}

/// Every refusal of `viprs pmtiles to-pvec` that the command line alone decides (exit 2).
pub(crate) fn check_to_pvec(args: &ToPvecArgs) -> CheckedToPvec {
    if names_a_directory(&args.output) {
        usage_error(
            &format!(
                "`{}` is a directory, and to-pvec writes one file",
                args.output.display()
            ),
            "name the .pvec to write, as in `out.pvec`",
        );
    }
    let into = require_into(args.pvec_into.as_ref(), "`viprs pmtiles to-pvec`");
    let explicit = args
        .pixel_to_source
        .as_deref()
        .map(|t| six(t, "pixel-to-source"));
    let source_to_geo = args
        .source_to_geo
        .as_deref()
        .map(|t| geo(t, "source-to-geo"));
    if explicit.is_none() && source_to_geo.is_none() {
        usage_error(
            "to-pvec has no way to place the raster",
            "give the drawing's calibration as `--source-to-geo a,b,c,d,e,f` (composed with the \
             transform the archive stores), or `--pixel-to-source a,b,c,d,tx,ty`",
        );
    }
    CheckedToPvec {
        into,
        explicit,
        source_to_geo,
        looks: looks(args.band, args.opacity, None, args.layer.as_ref()),
    }
}

/// `viprs pmtiles to-pvec`.
pub(crate) fn run_to_pvec(args: ToPvecArgs) {
    let checked = check_to_pvec(&args);
    #[cfg(feature = "pvec-raster")]
    imp::to_pvec(&args.archive, &args.output, checked);
    #[cfg(not(feature = "pvec-raster"))]
    {
        let _ = checked;
        missing_feature("`viprs pmtiles to-pvec`");
    }
}

/// `viprs pvec ...`.
pub fn run(args: PvecArgs) {
    match args.command {
        PvecCommand::Rasters(a) => {
            let tile = a.tile.as_ref().map(|t| {
                let n = |i: usize, name: &str| -> u32 {
                    t[i].parse().unwrap_or_else(|_| {
                        usage_error(
                            &format!("--tile {name} {:?} is not a whole number", t[i]),
                            "write it as `--tile ID LEVEL COL ROW OUT`, the first four numbers",
                        )
                    })
                };
                TileAt {
                    raster: n(0, "ID"),
                    level: n(1, "LEVEL"),
                    col: n(2, "COL"),
                    row: n(3, "ROW"),
                    out: t[4].clone(),
                }
            });
            #[cfg(feature = "pvec-raster")]
            imp::rasters(&a.file, a.json, tile);
            #[cfg(not(feature = "pvec-raster"))]
            {
                let _ = (tile, a.json, a.file);
                missing_feature("`viprs pvec rasters`");
            }
        }
    }
}

/// One tile of one raster, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(feature = "pvec-raster"), allow(dead_code))]
pub(crate) struct TileAt {
    pub(crate) raster: u32,
    pub(crate) level: u32,
    pub(crate) col: u32,
    pub(crate) row: u32,
    /// A path, or `-` for stdout.
    pub(crate) out: String,
}
