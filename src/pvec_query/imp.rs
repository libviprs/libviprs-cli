//! What `viprs pvec pick` and `viprs pvec tessellate` do once the command line is settled
//! (libviprs-cli#154). Needs the `pvec-raster` feature, which brings PVec's facade in: every
//! answer is the facade's `PickIndex` or `Tessellate` for the entities of the file, at the revision
//! the core pins, so what these commands say is what PVec itself says.

use pvec_capabilities::Registry;
use pvec_facade::{
    Bounds2, Curve, EntityId, PickIndex, PickTolerance, Point2, PvecError, PvecReader, RectMode,
    Tessellate, TessellationContext, Transform2,
};
use serde_json::{Value, json};

use super::{Mode, PickArgs, TessellateArgs};
use crate::operational_error;
use crate::pvec_raster::imp::{chain, open_pvec};

/// The entities of the file, in id order.
fn entities(reader: &PvecReader, file: &std::path::Path) -> Vec<(EntityId, Curve)> {
    reader.entities().unwrap_or_else(|e| {
        operational_error(&format!(
            "reading the entities of {}: {}",
            file.display(),
            chain(&e)
        ))
    })
}

/// One entity the command could not answer for, and why.
fn skipped_json(skipped: &[(u32, String)]) -> Value {
    Value::Array(
        skipped
            .iter()
            .map(|(entity, reason)| json!({ "entity": entity, "reason": reason }))
            .collect(),
    )
}

pub(crate) fn pick(a: &PickArgs) {
    let reader = open_pvec(&a.file);
    let mut index = PickIndex::new(&reader).unwrap_or_else(|e| {
        operational_error(&format!(
            "reading the entity count of {}: {}",
            a.file.display(),
            chain(&e)
        ))
    });
    // A bare line has no bounds, so the index refuses it; the command says which ones it left out.
    let mut skipped: Vec<(u32, String)> = Vec::new();
    for (id, curve) in entities(&reader, &a.file) {
        if let Err(e) = index.insert_exact(id, curve) {
            skipped.push((id.index(), e.to_string()));
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "note: {} entities are not in the pick index (see --json for which and why)",
            skipped.len()
        );
    }

    if let Some(r) = a.rect {
        let mode = match a.mode.unwrap_or(Mode::Intersects) {
            Mode::Intersects => RectMode::Intersects,
            Mode::Contains => RectMode::Contains,
        };
        let batch = index.query_rect(
            Bounds2 {
                min: [r[0], r[1]],
                max: [r[2], r[3]],
            },
            mode,
        );
        if a.json {
            println!(
                "{}",
                json!({
                    "v": 1,
                    "query": "rect",
                    "mode": match mode {
                        RectMode::Intersects => "intersects",
                        RectMode::Contains => "contains",
                    },
                    "entities": batch.as_slice(),
                    "skipped": skipped_json(&skipped),
                })
            );
        } else {
            for id in batch.as_slice() {
                println!("{id}");
            }
        }
        return;
    }

    let (query, p, found) = if let Some([x, y]) = a.point {
        let tolerance = a.tolerance.expect("clap requires --tolerance with --point");
        let tolerance = PickTolerance::new(tolerance, &Transform2::IDENTITY).unwrap_or_else(|e| {
            operational_error(&format!("PVec refused the tolerance: {}", chain(&e)))
        });
        let p = Point2 { x, y };
        ("point", p, index.query_point(p, tolerance))
    } else {
        let [x, y] = a.nearest.expect("one query is required");
        let p = Point2 { x, y };
        (
            "nearest",
            p,
            index.query_nearest(p, a.tolerance.unwrap_or(f64::INFINITY)),
        )
    };
    let registry = Registry::v1();
    let hit = found.map(|h| {
        let name = registry.get(h.capability).map_or("unknown", |c| c.name);
        (h, name)
    });
    if a.json {
        let hit = hit.map(|(h, name)| {
            json!({
                "entity": h.entity.index(),
                "distance": h.distance,
                "param": h.param,
                "bounds": [h.bounds.min[0], h.bounds.min[1], h.bounds.max[0], h.bounds.max[1]],
                "capability": name,
            })
        });
        println!(
            "{}",
            json!({
                "v": 1,
                "query": query,
                "at": [p.x, p.y],
                "hit": hit,
                "skipped": skipped_json(&skipped),
            })
        );
    } else if let Some((h, name)) = hit {
        let param = h.param.map_or("-".to_string(), |t| t.to_string());
        println!(
            "{}\t{}\t{}\t{},{},{},{}\t{}",
            h.entity.index(),
            h.distance,
            param,
            h.bounds.min[0],
            h.bounds.min[1],
            h.bounds.max[0],
            h.bounds.max[1],
            name
        );
    } else {
        eprintln!("no hit");
    }
}

pub(crate) fn tessellate(a: &TessellateArgs) {
    let reader = open_pvec(&a.file);
    let max_segments = usize::try_from(a.max_segments).unwrap_or(usize::MAX);
    let ctx = match (a.error, a.error_px, a.units_per_pixel) {
        (Some(error), _, _) => TessellationContext {
            source_to_map: Transform2::IDENTITY,
            map_to_screen: Transform2::IDENTITY,
            pixel_ratio: 1.0,
            max_screen_error_px: error,
            max_world_error: error,
            max_segments,
        },
        (None, Some(px), Some(per_pixel)) => TessellationContext {
            source_to_map: Transform2::IDENTITY,
            map_to_screen: Transform2::scale(1.0 / per_pixel, 1.0 / per_pixel).unwrap_or_else(
                |e| {
                    operational_error(&format!(
                        "--units-per-pixel {per_pixel} is not a usable scale: {}",
                        chain(&e)
                    ))
                },
            ),
            pixel_ratio: 1.0,
            max_screen_error_px: px,
            max_world_error: f64::INFINITY,
            max_segments,
        },
        _ => unreachable!("clap requires --error, or --error-px with --units-per-pixel"),
    };

    let mut rows = Vec::new();
    let (mut unmet, mut skipped) = (Vec::new(), Vec::new());
    let (mut worst_px, mut worst_world) = (0.0f64, 0.0f64);
    for (id, curve) in entities(&reader, &a.file) {
        let n = id.index();
        match curve.tessellate(&ctx) {
            Ok(t) => {
                worst_px = worst_px.max(t.achieved_screen_error_px);
                worst_world = worst_world.max(t.achieved_world_error);
                rows.push(json!({
                    "entity": n,
                    "status": "ok",
                    "vertices": t.vertices.len(),
                    "achieved_screen_error_px": t.achieved_screen_error_px,
                    "achieved_world_error": t.achieved_world_error,
                }));
            }
            Err(e @ PvecError::ErrorBudgetUnmet { .. }) => {
                unmet.push(format!(
                    "entity {n}: the error budget cannot be met within {} segments: {e}",
                    a.max_segments
                ));
                rows.push(json!({
                    "entity": n,
                    "status": "budget-unmet",
                    "reason": e.to_string(),
                }));
            }
            Err(e) => {
                skipped.push((n, e.to_string()));
                rows.push(json!({
                    "entity": n,
                    "status": "not-tessellated",
                    "reason": e.to_string(),
                }));
            }
        }
    }

    if a.json {
        println!(
            "{}",
            json!({
                "v": 1,
                "ok": unmet.is_empty(),
                "budget": {
                    "error": a.error,
                    "error_px": a.error_px,
                    "units_per_pixel": a.units_per_pixel,
                    "max_segments": a.max_segments,
                },
                "max_achieved_screen_error_px": worst_px,
                "max_achieved_world_error": worst_world,
                "entities": rows,
                "skipped": skipped_json(&skipped),
            })
        );
    } else {
        for row in rows.iter().filter(|r| r["status"] == "ok") {
            println!(
                "{}\t{}\t{}\t{}",
                row["entity"],
                row["vertices"],
                row["achieved_screen_error_px"],
                row["achieved_world_error"]
            );
        }
        for (entity, why) in &skipped {
            eprintln!("entity {entity}: not tessellated: {why}");
        }
    }
    if unmet.is_empty() {
        eprintln!(
            "tessellated {} entities, largest error {worst_px} px ({worst_world} in source units)",
            rows.len() - skipped.len()
        );
    } else {
        operational_error(&unmet.join("\n"));
    }
}
