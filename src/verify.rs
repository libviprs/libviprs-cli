//! `viprs verify <pyramid-or-archive>` (libviprs-cli#66).
//!
//! The core has three ways to check a finished pyramid and this command is a
//! thin route onto them, picked by what the path is:
//!
//! * **an archive** goes to `verify::pyramid_verify`, which checks the
//!   archive's own account of itself against the plan, walks its directories,
//!   probes every planned tile and counts what it addresses. That proves every
//!   tile is present and non-empty and nothing else is there. It does not read
//!   a payload's contents, so a flipped byte inside a tile sails through, and
//!   this command then decodes every tile as well, which is what catches it.
//! * **a tile tree** is walked here for presence (every planned tile is a
//!   non-empty file), then goes to `checksum::verify_output`, which re-hashes
//!   every tile against the per-tile digests `viprs pyramid --checksum`
//!   recorded. `pyramid_verify` cannot take a tree: its last check counts the
//!   tiles the storage addresses, and `DirectoryPyramidReader` refuses to
//!   count, so the presence half is done by walking the plan instead.
//! * **a tile tree with `--source`** also goes to
//!   `stream_verify::verify_from_strip_source`, which re-renders the pyramid
//!   from the input and compares. It is the only check that can tell a tree
//!   was made from a different image, and only for `--format raw` trees does
//!   it compare bytes: an encoded tile cannot be re-encoded bit for bit, so for
//!   PNG and JPEG it falls back to the manifest digests above.
//!
//! Every failure names the tile. A verify that says "corrupt" and not where is
//! a verify somebody has to rerun by hand to use.

use std::path::{Path, PathBuf};
use std::process;

use libviprs::checksum::verify_output;
use libviprs::observe::NoopObserver;
use libviprs::sink::BLANK_TILE_MARKER;
use libviprs::sink_pmtiles::tile_coord_to_zxy;
use libviprs::stream_verify::verify_from_strip_source;
use libviprs::streaming::RasterStripSource;
use libviprs::verify::pyramid_verify;
use libviprs::{
    EngineConfig, EngineError, FsSink, Manifest, PmTilesPyramidReader, PyramidPlan, PyramidPlanner,
    PyramidReader, TileCoord, TileFormat,
};

use crate::{operational_error, usage_error};

/// Arguments of `viprs verify`.
#[derive(clap::Parser)]
pub(crate) struct VerifyArgs {
    /// A `.pmtiles` archive, or a tile tree written with a manifest
    /// (`viprs pyramid ... --storage directory --checksum`).
    pub(crate) path: PathBuf,

    /// Re-render from this input and compare it against the tree.
    ///
    /// Tile trees only. Compares bytes for `--format raw` trees; for encoded
    /// tiles the manifest digests are the check.
    #[arg(long, value_name = "FILE")]
    pub(crate) source: Option<PathBuf>,
}

/// `viprs verify`: exit 0 and a summary on stdout, or exit 1 naming what is
/// wrong on stderr.
pub(crate) fn run(args: VerifyArgs) {
    if args.path.is_dir() {
        verify_tree(&args.path, args.source.as_deref());
    } else if args.path.is_file() {
        if args.source.is_some() {
            usage_error(
                "--source applies to a tile tree, not an archive",
                "an archive is checked by reading every tile back and decoding it; drop --source",
            );
        }
        verify_archive(&args.path);
    } else {
        operational_error(&format!("{} does not exist", args.path.display()));
    }
}

/// Exit 1 listing every problem, one per line.
fn fail(problems: &[String]) -> ! {
    for p in problems {
        eprintln!("Error: {p}");
    }
    process::exit(1);
}

fn plan_for(
    what: &Path,
    width: u32,
    height: u32,
    tile_size: u32,
    overlap: u32,
    layout: libviprs::Layout,
) -> PyramidPlan {
    match PyramidPlanner::new(width, height, tile_size, overlap, layout) {
        Ok(p) => p.plan(),
        Err(e) => operational_error(&format!(
            "{} records a pyramid no plan can describe: {e}",
            what.display()
        )),
    }
}

fn verify_archive(path: &Path) {
    let reader = PmTilesPyramidReader::try_open(path)
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", path.display())));
    let desc = reader
        .describe()
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", path.display())));
    let (Some(w), Some(h), Some(tile_size), Some(layout)) = (
        desc.source_width,
        desc.source_height,
        desc.tile_size,
        desc.layout,
    ) else {
        operational_error(&format!(
            "{} does not say how it was generated (no vnd.libviprs metadata), so there is no \
             plan to verify it against. `viprs pmtiles verify` checks its structure alone",
            path.display()
        ));
    };
    let plan = plan_for(path, w, h, tile_size, desc.overlap.unwrap_or(0), layout);

    if let Err(e) = pyramid_verify(&reader, &plan, desc.format, &NoopObserver) {
        fail(&[format!("{}: {e}", path.display())]);
    }

    // pyramid_verify proves presence and length. The bytes are this pass.
    let mut problems = Vec::new();
    for coord in plan.tile_coords() {
        let name = zxy_name(coord);
        match reader.tile(coord) {
            Ok(Some(bytes)) if bytes.as_slice() == [BLANK_TILE_MARKER] => {}
            Ok(Some(bytes)) => {
                if let Err(e) = libviprs::decode_bytes(&bytes) {
                    problems.push(format!("tile {name} does not decode: {e}"));
                }
            }
            Ok(None) => problems.push(format!("tile {name} is missing")),
            Err(e) => problems.push(format!("tile {name} cannot be read: {e}")),
        }
    }
    if !problems.is_empty() {
        fail(&problems);
    }

    println!("Verified: {}", path.display());
    println!("Tiles: {}", plan.total_tile_count());
    println!("Checks: plan, structure, every tile present and decoded");
}

/// `z/x/y` for an archive tile, which is how PMTiles names it.
fn zxy_name(coord: TileCoord) -> String {
    match tile_coord_to_zxy(coord) {
        Ok((z, x, y)) => format!("{z}/{x}/{y}"),
        Err(_) => format!("{}/{}/{}", coord.level, coord.col, coord.row),
    }
}

/// The manifest beside or inside a tree, in the order `FsSink` writes them.
fn read_manifest(dir: &Path) -> Option<Manifest> {
    let mut candidates = Vec::new();
    if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
        let mut sibling = name.to_os_string();
        sibling.push(".manifest.json");
        candidates.push(parent.join(sibling));
    }
    candidates.push(dir.join("manifest.json"));
    let path = candidates.into_iter().find(|p| p.is_file())?;
    match Manifest::read_from(&path) {
        Ok(m) => Some(m),
        Err(e) => operational_error(&format!("reading {}: {e}", path.display())),
    }
}

fn verify_tree(dir: &Path, source: Option<&Path>) {
    let Some(manifest) = read_manifest(dir) else {
        operational_error(&format!(
            "{} has no manifest.json, so there is no plan to verify it against. Write the tree \
             with `viprs pyramid ... --storage directory --checksum`",
            dir.display()
        ));
    };
    let m = manifest.as_v1();
    let g = &m.generation;
    let format: TileFormat = g.format;
    let plan = plan_for(
        dir,
        m.source.width,
        m.source.height,
        g.tile_size,
        g.overlap,
        g.layout,
    );

    let mut problems = Vec::new();
    for coord in plan.tile_coords() {
        let Some(rel) = plan.tile_path(coord, format.extension()) else {
            continue;
        };
        match std::fs::metadata(dir.join(&rel)) {
            Ok(meta) if meta.len() > 0 => {}
            Ok(_) => problems.push(format!("tile {rel} is empty")),
            Err(_) => problems.push(format!("tile {rel} is missing")),
        }
    }
    if !problems.is_empty() {
        fail(&problems);
    }

    let report = match verify_output(dir) {
        Ok(r) => r,
        Err(e) => fail(&[format!("{}: {e}", dir.display())]),
    };
    let mut problems: Vec<String> = report
        .tiles_mismatched
        .iter()
        .map(|p| format!("tile {} does not match its checksum", p.display()))
        .collect();
    problems.extend(
        report
            .tiles_missing
            .iter()
            .map(|p| format!("tile {} is missing", p.display())),
    );
    if !problems.is_empty() {
        fail(&problems);
    }

    let mut checks = vec!["plan", "every tile present"];
    if report.tiles_checked > 0 {
        checks.push("every tile against its checksum");
    }

    if let Some(source) = source {
        rerender(dir, source, &plan, format, m);
        checks.push("a re-render of the source");
    }

    println!("Verified: {}", dir.display());
    println!("Tiles: {}", plan.total_tile_count());
    println!("Checks: {}", checks.join(", "));
    if report.tiles_checked == 0 {
        println!(
            "Note: the manifest records no per-tile checksums, so tile contents were not \
             checked. Write the tree with --checksum to get them."
        );
    }
}

/// Re-render the pyramid from `source` and compare it against the tree.
fn rerender(
    dir: &Path,
    source: &Path,
    plan: &PyramidPlan,
    format: TileFormat,
    m: &libviprs::ManifestV1,
) {
    let raster = libviprs::decode_file(source)
        .unwrap_or_else(|e| operational_error(&format!("decoding {}: {e}", source.display())));
    if (raster.width(), raster.height()) != (plan.image_width, plan.image_height) {
        fail(&[format!(
            "{} is {}x{}, and the tree was made from a {}x{} image",
            source.display(),
            raster.width(),
            raster.height(),
            plan.image_width,
            plan.image_height
        )]);
    }
    let strips = RasterStripSource::new(&raster);
    let sink = FsSink::new(dir, plan.clone()).with_format(format);
    let mut config = EngineConfig::default().with_blank_tile_strategy(m.generation.blank_strategy);
    config.background_rgb = m.generation.background_rgb;
    match verify_from_strip_source(&strips, plan, &sink, &config, &NoopObserver) {
        Ok(_) => {}
        Err(EngineError::ChecksumMismatch { tile, .. }) => {
            let name = plan
                .tile_path(tile, format.extension())
                .unwrap_or_else(|| format!("{tile:?}"));
            fail(&[format!(
                "tile {name} does not match a re-render of {}",
                source.display()
            )]);
        }
        Err(e) => fail(&[format!("{}: {e}", dir.display())]),
    }
}
