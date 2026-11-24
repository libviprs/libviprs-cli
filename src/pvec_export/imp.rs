//! What `viprs pvec export` does once the command line is settled (libviprs-cli#154). Needs the
//! `pvec-export` feature, which brings in `pvec-interchange` at the revision the core pins: every byte
//! of an export is that crate's writer, and every report is its fidelity report, counted against the
//! number of entities the drawing has (or, for tiles, the number that reach them), so a writer that
//! drops an entity without a record is refused by the report and not shipped.

use std::collections::BTreeSet;
use std::path::Path;

use pvec_container::source::MemorySource;
use pvec_container::{ContainerReader, ReaderConfig};
use pvec_facade::{Curve, EntityId, PvecReader, Transform2};
use pvec_interchange::mvt::{MvtEntity, MvtOptions, MvtTile, Projection, TileId};
use pvec_interchange::report::Report;
use pvec_interchange::step::{StepEntity, StepOptions};
use pvec_interchange::{gml, gpkg, mvt, pmtiles, step};
use serde_json::Value;

use super::{ExportArgs, Format};
use crate::pvec_raster::imp::{chain, read_file};
use crate::{operational_error, usage_error};

/// The `last_change` stamp of a STEP file: no clock is read, so the file is reproducible.
const STEP_STAMP: &str = "1970-01-01T00:00:00";

/// The layer every entity of a tile goes to; the file's own layer table is not read here.
const TILE_LAYER: &str = "entities";

/// The map a tile is cut from is 256 pixels wide per tile.
const TILE_PX: f64 = 256.0;

/// An export: the bytes, and what the writer says it did to each entity.
struct Product {
    bytes: Vec<u8>,
    report: Report,
}

fn refused(what: &str, e: &dyn std::error::Error) -> ! {
    operational_error(&format!("{what}: {}", chain(e)))
}

/// `--affine`, or the identity.
fn transform(affine: Option<[f64; 6]>) -> Transform2 {
    affine.map_or(Transform2::IDENTITY, |m| {
        Transform2::new(m).unwrap_or_else(|e| {
            usage_error(
                &format!("--affine is refused by PVec: {}", chain(&e)),
                "give a finite matrix with an inverse",
            )
        })
    })
}

/// The entities that have a record in some tile, counted once each.
fn reached(tiles: &[MvtTile]) -> u32 {
    let ids: BTreeSet<u32> = tiles
        .iter()
        .flat_map(|t| t.ledger.ledger().records())
        .map(|r| r.entity_id().index())
        .collect();
    u32::try_from(ids.len()).expect("fewer entities than u32")
}

/// The drawing's declared bounds fitted into a square map with their top-left at the map's
/// top-left (tile `0/0/0` is the whole drawing), source y up becoming tile y down.
fn projection(reader: &PvecReader) -> Projection {
    let b = reader.manifest().bounds;
    let size = (b.max[0] - b.min[0]).max(b.max[1] - b.min[1]);
    if !(size.is_finite() && size > 0.0) {
        operational_error(
            "the drawing's declared bounds have no extent, so there is nothing to tile",
        );
    }
    let to_map = Transform2::new([1.0, 0.0, 0.0, -1.0, -b.min[0], b.max[1]])
        .unwrap_or_else(|e| refused("the drawing's bounds cannot be mapped", &e));
    Projection::new(to_map, size, TILE_PX)
        .unwrap_or_else(|e| refused("the drawing's bounds cannot be tiled", &e))
}

fn tile_entities(entities: &[(EntityId, Curve)]) -> Vec<MvtEntity> {
    entities
        .iter()
        .map(|(id, curve)| {
            MvtEntity::new(*id, TILE_LAYER, curve.clone())
                .unwrap_or_else(|e| refused("building a tile entity", &e))
        })
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    let name = path
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let done = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = done {
        let _ = std::fs::remove_file(&tmp);
        operational_error(&format!("writing {}: {e}", path.display()));
    }
}

pub(crate) fn run(a: &ExportArgs) {
    let bytes = read_file(&a.file);
    let reader = PvecReader::open(bytes.clone()).unwrap_or_else(|e| {
        operational_error(&format!(
            "{} is not a .pvec this build reads: {}",
            a.file.display(),
            chain(&e)
        ))
    });
    let digest = ContainerReader::open(MemorySource::new(bytes), ReaderConfig::default())
        .ok()
        .and_then(|c| c.manifest().artifact_digest);
    if digest.is_none() {
        eprintln!(
            "note: the file carries no artifact digest, so the report's source_digest is all zeros"
        );
    }
    let digest = digest.unwrap_or([0; 32]);
    let entities = reader.entities().unwrap_or_else(|e| {
        operational_error(&format!(
            "reading the entities of {}: {}",
            a.file.display(),
            chain(&e)
        ))
    });
    let total = u32::try_from(entities.len()).expect("fewer entities than u32");

    let product = match a.format {
        Format::Gml => {
            let crs = gml::CrsTransform::new(
                a.srs.as_deref().unwrap_or("urn:pvec:local"),
                transform(a.affine),
            )
            .unwrap_or_else(|e| refused("--srs is refused by PVec", &e));
            let budget = gml::Budget::new(a.tolerance.expect("checked"))
                .unwrap_or_else(|e| refused("--tolerance is refused by PVec", &e));
            let export = gml::export(&entities, &crs, budget)
                .unwrap_or_else(|e| refused("the GML export", &e));
            let report = export
                .report(digest, total)
                .unwrap_or_else(|e| refused("the fidelity report", &e));
            Product {
                bytes: export.xml.into_bytes(),
                report,
            }
        }
        Format::Gpkg => {
            let options = gpkg::GpkgOptions::new(a.tolerance.expect("checked"))
                .unwrap_or_else(|e| refused("--tolerance is refused by PVec", &e));
            let list: Vec<gpkg::GpkgEntity> = entities
                .iter()
                .map(|(id, curve)| gpkg::GpkgEntity {
                    entity_id: *id,
                    curve: curve.clone(),
                })
                .collect();
            let export = gpkg::export(&list, &options)
                .unwrap_or_else(|e| refused("the GeoPackage export", &e));
            let report = export
                .report(digest, total)
                .unwrap_or_else(|e| refused("the fidelity report", &e));
            Product {
                bytes: export.bytes,
                report,
            }
        }
        Format::Step => {
            let name = a
                .out
                .file_name()
                .map_or_else(|| "export.stp".into(), |n| n.to_string_lossy().into_owned());
            let rasters = reader
                .raster_placements()
                .unwrap_or_else(|e| refused("reading the raster stream", &e))
                .len();
            let options = StepOptions::new(&name, STEP_STAMP)
                .unwrap_or_else(|e| refused("the STEP header", &e))
                .with_transform(transform(a.affine))
                .with_rasters((0..u32::try_from(rasters).expect("few rasters")).collect());
            let list: Vec<StepEntity> = entities
                .iter()
                .map(|(id, curve)| StepEntity::exact(*id, curve.clone()))
                .collect();
            let export =
                step::export(&list, &options).unwrap_or_else(|e| refused("the STEP export", &e));
            let report = export
                .report(digest, total)
                .unwrap_or_else(|e| refused("the fidelity report", &e));
            Product {
                bytes: export.part21.into_bytes(),
                report,
            }
        }
        Format::Mvt | Format::Pmtiles => {
            let projection = projection(&reader);
            let mut options = MvtOptions::default();
            if let Some(px) = a.error_px {
                options.screen_tolerance_px = px;
            }
            let list = tile_entities(&entities);
            let tile_at = |z: u8, x: u32, y: u32| {
                let id = TileId::new(z, x, y).unwrap_or_else(|e| refused("the tile address", &e));
                mvt::export_tile(&list, &projection, id, &options)
                    .unwrap_or_else(|e| refused("the vector tile export", &e))
            };
            if a.format == Format::Mvt {
                let (z, x, y) = a.tile.unwrap_or((0, 0, 0));
                let tile = tile_at(z, x, y);
                if tile.bytes.is_empty() {
                    eprintln!("note: no entity reaches tile {z}/{x}/{y}, so the tile is empty");
                }
                let n = reached(std::slice::from_ref(&tile));
                let report = mvt::report(std::slice::from_ref(&tile), digest, n)
                    .unwrap_or_else(|e| refused("the fidelity report", &e));
                Product {
                    bytes: tile.bytes,
                    report,
                }
            } else {
                let (lo, hi) = a.zooms.unwrap_or((0, 0));
                let tiles: Vec<MvtTile> = (lo..=hi)
                    .flat_map(|z| {
                        let side = 1u32 << z;
                        (0..side).flat_map(move |x| (0..side).map(move |y| (z, x, y)))
                    })
                    .map(|(z, x, y)| tile_at(z, x, y))
                    .collect();
                let provenance = pmtiles::Provenance {
                    source_digest: Some(digest),
                    tessellation_tolerance_px: Some(options.screen_tolerance_px),
                    tested_zooms: Some((lo, hi)),
                    created: None,
                };
                let archive = pmtiles::PmtilesOptions {
                    name: a.out.file_stem().map(|s| s.to_string_lossy().into_owned()),
                };
                let bytes = pmtiles::write_pmtiles(&tiles, &provenance, &archive)
                    .unwrap_or_else(|e| refused("the PMTiles archive", &e));
                let report = pmtiles::report(&tiles, &provenance, reached(&tiles))
                    .unwrap_or_else(|e| refused("the fidelity report", &e));
                Product { bytes, report }
            }
        }
    };

    say_if_lossy(&product.report, a.format);
    if let Some(path) = &a.report {
        let text = if path.extension().is_some_and(|e| e == "md") {
            product.report.to_markdown()
        } else {
            product.report.to_json()
        };
        write_atomic(path, text.as_bytes());
    }
    write_atomic(&a.out, &product.bytes);
}

/// A lossy export says so on stderr, in the words of its own report: how many entities are
/// approximations (and by how much at most) and how many were left out.
fn say_if_lossy(report: &Report, format: Format) {
    let doc: Value = serde_json::from_str(&report.to_json()).expect("the report is JSON");
    let count = |kind: &str| doc["entities"]["by_fidelity"][kind].as_u64().unwrap_or(0);
    let (approximated, left_out) = (count("BoundedApproximation"), count("Unsupported"));
    if approximated + left_out == 0 {
        return;
    }
    let total = doc["entities"]["total"].as_u64().unwrap_or(0);
    let largest = doc["approximation_error"]["max"].as_f64();
    let display = matches!(format, Format::Mvt | Format::Pmtiles);
    eprintln!(
        "lossy export{}: {approximated} of {total} entities are approximations{}, {left_out} left out; \
         --report PATH writes the full record",
        if display { " (display only)" } else { "" },
        largest.map_or(String::new(), |m| format!(" (largest error {m})")),
    );
}
