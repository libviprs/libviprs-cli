//! What `viprs pvec info`, `validate`, `text` and `calibrate` do once the command line is
//! settled (libviprs-cli#147). Needs the `pvec-raster` feature, which is what brings PVec's facade
//! in. Everything here reads through `PvecReader` at the revision the core pins, so what these
//! commands say about a file is what PVec itself makes of it.

use std::path::Path;

use pvec_capabilities::Registry;
use pvec_facade::PvecReader;
use pvec_facade::components::{
    HatchStream, LayerStream, LineStyleStream, ProvenanceStream, Stream, TextStream,
};
use pvec_facade::unstable::{
    Calibration, EffectiveScale, Provenance, ScaleClaim, recalibrate_file,
};
use serde_json::{Value, json};

use super::CalibrateArgs;
use crate::operational_error;
use crate::pvec_raster::imp::{chain, open_pvec, read_file};

/// The word for a scale's provenance, as `info` prints it.
fn provenance_name(p: &Provenance) -> &'static str {
    match p {
        Provenance::DeclaredUnit => "declared-unit",
        Provenance::PlanScale { .. } => "plan-scale",
        Provenance::Measured { .. } => "measured",
        Provenance::Georeference => "georeference",
        Provenance::Manual => "manual",
        _ => "other",
    }
}

/// The text records of every text stream, in file order.
fn text_records(streams: &[Stream]) -> Result<Vec<TextStream>, String> {
    streams
        .iter()
        .filter(|s| s.capability == TextStream::CAPABILITY)
        .map(|s| TextStream::decode(&s.payload).map_err(|e| format!("the text stream: {e}")))
        .collect()
}

pub(crate) fn info(file: &Path, as_json: bool) {
    fn refused(file: &Path, what: &str, e: &dyn std::error::Error) -> ! {
        operational_error(&format!(
            "reading the {what} of {}: {}",
            file.display(),
            chain(e)
        ))
    }
    let reader = open_pvec(file);
    let manifest = reader.manifest();
    let entities = reader
        .entity_count()
        .unwrap_or_else(|e| refused(file, "entities", &e));
    let (streams, skipped) = reader
        .streams()
        .unwrap_or_else(|e| refused(file, "streams", &e));
    let layers: Vec<String> = streams
        .iter()
        .filter(|s| s.capability == LayerStream::CAPABILITY)
        .map(|s| LayerStream::decode_at(s.version, &s.payload))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| refused(file, "layer table", &e))
        .into_iter()
        .flat_map(|t| t.names)
        .collect();
    let texts: usize = text_records(&streams)
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", file.display())))
        .iter()
        .map(|t| t.records.len())
        .sum();
    let rasters = reader
        .raster_placements()
        .unwrap_or_else(|e| refused(file, "raster stream", &e))
        .len();
    let calibration = reader
        .calibration()
        .unwrap_or_else(|e| refused(file, "calibration", &e));
    let scale = reader
        .effective_scale()
        .unwrap_or_else(|e| refused(file, "calibration", &e));
    let spatial_index = reader
        .spatial_directory()
        .unwrap_or_else(|e| refused(file, "spatial directory", &e))
        .is_some();

    let registry = Registry::v1();
    let name = |id| registry.get(id).map_or("unknown", |c| c.name);
    let note = calibration
        .as_ref()
        .and_then(|c| c.scale.as_ref())
        .map(|s| s.note.clone());
    let georeference = calibration.as_ref().and_then(|c| c.georeference.as_ref());

    if as_json {
        let scale = match scale {
            EffectiveScale::Calibration {
                metres_per_source_unit,
                provenance,
                uncertainty,
            } => json!({
                "source": "calibration",
                "metres_per_unit": metres_per_source_unit,
                "provenance": provenance_name(&provenance),
                "uncertainty": uncertainty,
                "note": note,
            }),
            EffectiveScale::Manifest { metres_per_unit } => {
                json!({ "source": "manifest", "metres_per_unit": metres_per_unit })
            }
            _ => json!({ "source": "unknown" }),
        };
        let georeference = georeference.map_or(Value::Null, |g| {
            json!({
                "crs_id": g.crs_id,
                "crs_kind": format!("{:?}", g.crs_kind).to_lowercase(),
                "method": format!("{:?}", g.method).to_lowercase(),
                "source_to_crs": g.source_to_crs,
                "control_points": g.control_points.len(),
                "residual_max": g.residual_max,
            })
        });
        let streams: Vec<Value> = streams
            .iter()
            .map(|s| {
                json!({
                    "capability": s.capability.get(),
                    "name": name(s.capability),
                    "version": s.version.to_string(),
                    "bytes": s.payload.len(),
                })
            })
            .collect();
        let skipped: Vec<Value> = skipped
            .iter()
            .map(|s| {
                json!({
                    "capability": s.capability.get(),
                    "name": name(s.capability),
                    "version": s.version.to_string(),
                    "reason": format!("{:?}", s.reason),
                })
            })
            .collect();
        let capabilities: Vec<u32> = manifest.capabilities.iter().map(|c| c.get()).collect();
        println!(
            "{}",
            json!({
                "v": 1,
                "kind": "pvec",
                "path": file.display().to_string(),
                "producer_id": manifest.producer_id,
                "producer_version": manifest.producer_version,
                "metres_per_unit": manifest.metres_per_unit,
                "bounds": { "min": manifest.bounds.min, "max": manifest.bounds.max },
                "capabilities": capabilities,
                "entities": entities,
                "streams": streams,
                "skipped": skipped,
                "layers": layers,
                "text_records": texts,
                "rasters": rasters,
                "spatial_index": spatial_index,
                "scale": scale,
                "georeference": georeference,
            })
        );
        return;
    }

    println!("File: {}", file.display());
    println!(
        "Producer: {} {}",
        manifest.producer_id, manifest.producer_version
    );
    println!("Unit: {} m per unit", manifest.metres_per_unit);
    println!(
        "Bounds: {},{} to {},{}",
        manifest.bounds.min[0],
        manifest.bounds.min[1],
        manifest.bounds.max[0],
        manifest.bounds.max[1]
    );
    if manifest.capabilities.is_empty() {
        println!("Capabilities: none");
    } else {
        let ids: Vec<String> = manifest
            .capabilities
            .iter()
            .map(|c| format!("{} ({})", c.get(), name(*c)))
            .collect();
        println!("Capabilities: {}", ids.join(", "));
    }
    println!("Entities: {entities}");
    println!("Streams: {}", streams.len());
    for s in &streams {
        println!(
            "  {} ({}) v{}, {} bytes",
            name(s.capability),
            s.capability.get(),
            s.version,
            s.payload.len()
        );
    }
    println!("Skipped streams: {}", skipped.len());
    for s in &skipped {
        println!(
            "  {} ({}) v{}: {:?}",
            name(s.capability),
            s.capability.get(),
            s.version,
            s.reason
        );
    }
    if layers.is_empty() {
        println!("Layers: none");
    } else {
        println!("Layers: {}", layers.join(", "));
    }
    println!("Text records: {texts}");
    println!("Rasters: {rasters}");
    println!(
        "Spatial index: {}",
        if spatial_index { "yes" } else { "no" }
    );
    match scale {
        EffectiveScale::Calibration {
            metres_per_source_unit,
            provenance,
            ..
        } => println!(
            "Scale: {metres_per_source_unit} m per unit (calibration, {})",
            provenance_name(&provenance)
        ),
        EffectiveScale::Manifest { metres_per_unit } => {
            println!("Scale: {metres_per_unit} m per unit (manifest)");
        }
        _ => println!("Scale: unknown"),
    }
    match georeference {
        Some(g) => println!(
            "Georeference: {} ({:?}, {} control points)",
            g.crs_id,
            g.crs_kind,
            g.control_points.len()
        ),
        None => println!("Georeference: none"),
    }
}

pub(crate) fn validate(file: &Path, as_json: bool) {
    // What a pass read, so the summary can say it.
    struct Read {
        entities: usize,
        streams: usize,
        skipped: usize,
        rasters: usize,
        calibrated: bool,
    }
    let bytes = read_file(file);
    let checked = (|| -> Result<Read, String> {
        let reader = PvecReader::open(bytes).map_err(|e| chain(&e))?;
        let entities = reader.entities().map_err(|e| chain(&e))?.len();
        reader.styles().map_err(|e| chain(&e))?;
        let (streams, skipped) = reader.streams().map_err(|e| chain(&e))?;
        for s in &streams {
            let decoded = match s.capability {
                c if c == LayerStream::CAPABILITY => {
                    LayerStream::decode_at(s.version, &s.payload).map(drop)
                }
                c if c == LineStyleStream::CAPABILITY => {
                    LineStyleStream::decode(&s.payload).map(drop)
                }
                c if c == TextStream::CAPABILITY => TextStream::decode(&s.payload).map(drop),
                c if c == HatchStream::CAPABILITY => HatchStream::decode(&s.payload).map(drop),
                c if c == ProvenanceStream::CAPABILITY => {
                    ProvenanceStream::decode(&s.payload).map(drop)
                }
                _ => Ok(()),
            };
            decoded.map_err(|e| format!("stream {}: {e}", s.capability.get()))?;
        }
        let calibrated = reader.calibration().map_err(|e| chain(&e))?.is_some();
        let rasters = reader.raster_placements().map_err(|e| chain(&e))?.len();
        Ok(Read {
            entities,
            streams: streams.len(),
            skipped: skipped.len(),
            rasters,
            calibrated,
        })
    })();
    match checked {
        Ok(read) if as_json => println!(
            "{}",
            json!({
                "v": 1,
                "path": file.display().to_string(),
                "valid": true,
                "entities": read.entities,
                "streams": read.streams,
                "skipped": read.skipped,
                "rasters": read.rasters,
                "calibrated": read.calibrated,
            })
        ),
        Ok(read) => println!(
            "{}: valid, {} entities, {} streams ({} skipped), {} rasters, {}",
            file.display(),
            read.entities,
            read.streams,
            read.skipped,
            read.rasters,
            if read.calibrated {
                "calibrated"
            } else {
                "no calibration"
            }
        ),
        Err(reason) => {
            if as_json {
                println!(
                    "{}",
                    json!({
                        "v": 1,
                        "path": file.display().to_string(),
                        "valid": false,
                        "error": reason,
                    })
                );
            }
            operational_error(&format!(
                "{} is not a valid .pvec: {reason}",
                file.display()
            ));
        }
    }
}

pub(crate) fn text(file: &Path, as_json: bool) {
    let reader = open_pvec(file);
    let (streams, _) = reader.streams().unwrap_or_else(|e| {
        operational_error(&format!(
            "reading the streams of {}: {}",
            file.display(),
            chain(&e)
        ))
    });
    let tables = text_records(&streams)
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", file.display())));
    let records = tables.iter().flat_map(|t| &t.records);
    if as_json {
        let text: Vec<Value> = records
            .map(|r| {
                json!({
                    "entity": r.entity.index(),
                    "x": r.x,
                    "y": r.y,
                    "height": r.height,
                    "rotation": r.rotation,
                    "text": r.text,
                })
            })
            .collect();
        println!("{}", json!({ "v": 1, "text": text }));
        return;
    }
    // One record a line, so a tab, a line break or a backslash inside the text is escaped the
    // way a TSV reader expects.
    for r in records {
        let escaped = r
            .text
            .replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        println!(
            "{}\t{}\t{}\t{}\t{}\t{escaped}",
            r.entity.index(),
            r.x,
            r.y,
            r.height,
            r.rotation
        );
    }
}

pub(crate) fn calibrate(args: &CalibrateArgs) {
    let file = &args.file;
    // The calibration the file has now, so a georeference survives a new scale.
    let current = open_pvec(file).calibration().unwrap_or_else(|e| {
        operational_error(&format!(
            "reading the calibration of {}: {}",
            file.display(),
            chain(&e)
        ))
    });
    let (georeference, external_id) = current
        .map(|c| (c.georeference, c.external_id))
        .unwrap_or_default();
    let claim = |metres_per_source_unit, provenance| ScaleClaim {
        metres_per_source_unit,
        provenance,
        uncertainty: args.uncertainty,
        note: args.note.clone().unwrap_or_default(),
    };
    // clap's group makes exactly one of these present, and each one's companion with it.
    let scale = match (
        args.scale,
        args.plan_scale,
        args.paper_unit,
        args.measured,
        args.distance,
    ) {
        (Some(metres), ..) => Some(claim(metres, Provenance::Manual)),
        (None, Some((numerator, denominator)), Some(paper), ..) => Some(claim(
            paper * f64::from(denominator) / f64::from(numerator),
            Provenance::PlanScale {
                numerator,
                denominator,
                paper_metres_per_unit: paper,
            },
        )),
        (None, None, _, Some([x0, y0, x1, y1]), Some(distance)) => Some(claim(
            distance / (x1 - x0).hypot(y1 - y0),
            Provenance::Measured {
                p0: [x0, y0],
                p1: [x1, y1],
                distance_metres: distance,
            },
        )),
        _ => None,
    };
    let said = scale.as_ref().map(|s| {
        format!(
            "{} m per unit ({})",
            s.metres_per_source_unit,
            provenance_name(&s.provenance)
        )
    });
    let calibration = (scale.is_some() || georeference.is_some()).then_some(Calibration {
        scale,
        georeference,
        external_id,
    });
    recalibrate_file(file, calibration.as_ref()).unwrap_or_else(|e| {
        operational_error(&format!("calibrating {}: {}", file.display(), chain(&e)))
    });
    match said {
        Some(said) => println!("Calibrated {}: {said}", file.display()),
        None => println!("Removed the scale record from {}", file.display()),
    }
}
