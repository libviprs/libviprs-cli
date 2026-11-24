//! What `viprs pvec info`, `validate`, `text` and `calibrate` do once the command line is
//! settled (libviprs-cli#147). Needs the `pvec-raster` feature, which is what brings PVec's facade
//! in. Everything here reads through PVec's own crates at the revision the core pins: the facade's
//! `PvecReader`, and for `validate` the container reader and the calibration check PVec's own
//! validator uses, so what these commands say about a file is what PVec itself makes of it.

use std::path::Path;

use pvec_capabilities::Registry;
use pvec_components::check_residual;
use pvec_container::integrity::DigestStatus;
use pvec_container::source::MemorySource;
use pvec_container::{ContainerReader, ReadError, ReaderConfig};
use pvec_facade::PvecReader;
use pvec_facade::components::{
    HatchStream, LayerStream, LineStyleStream, ProvenanceStream, SkipReason, Stream, TextStream,
};
use pvec_facade::unstable::{
    Calibration, CrsKind, EffectiveScale, FitMethod, Provenance, ScaleClaim, recalibrate_file,
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

/// The word for a CRS kind. Spelled out rather than taken from `Debug`, so a rename upstream is a
/// compile error here and not a quiet change to the JSON.
fn crs_kind_name(kind: CrsKind) -> &'static str {
    match kind {
        CrsKind::Projected => "projected",
        CrsKind::Geographic => "geographic",
        CrsKind::Engineering => "engineering",
        _ => "other",
    }
}

/// Why a stream was skipped, in words, for the same reason.
fn skip_reason_name(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::UnknownCapability => "unknown-capability",
        SkipReason::NotImplemented => "not-implemented",
        SkipReason::IncompatibleVersion => "incompatible-version",
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
                "crs_kind": crs_kind_name(g.crs_kind),
                "method": match g.method {
                    FitMethod::Declared => "declared",
                    FitMethod::Fitted => "fitted",
                    FitMethod::Manual => "manual",
                    _ => "other",
                },
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
                    "reason": skip_reason_name(s.reason),
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
            "  {} ({}) v{}: {}",
            name(s.capability),
            s.capability.get(),
            s.version,
            skip_reason_name(s.reason)
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
            "Georeference: {} ({}, {} control points)",
            g.crs_id,
            crs_kind_name(g.crs_kind),
            g.control_points.len()
        ),
        None => println!("Georeference: none"),
    }
}

/// PVec's verdict on a file, the one its own `pvec validate` gives (`pvec-conformance`'s
/// `commands::validate` at the revision the core pins): the container's structure and limits, every
/// block checksum and the artifact digest decide whether the file conforms, and a file that fails
/// one is exit 1. A raster whose tiles the reader refuses and a calibration it refuses, the CAL-11
/// residual check included (only a validator recomputes it), are reported with their rule and leave
/// the file conforming, because the geometry does not depend on either. On top of PVec's checks this
/// decodes every component stream viprs reads and reports one that does not decode; PVec's
/// validator does not read them, so that does not change the verdict either.
pub(crate) fn validate(file: &Path, as_json: bool) {
    let bytes = read_file(file);
    let opened = ContainerReader::open(MemorySource::new(bytes.clone()), ReaderConfig::default())
        .and_then(|reader| {
            let checked = reader.validate()?;
            let digest = reader.verify_artifact_digest()?;
            Ok((reader, checked, digest))
        });
    let (reader, checked, digest) = match opened {
        Ok(parts) => parts,
        Err(e) => {
            let (rule, reason) = (e.rule(), e.to_string());
            if as_json {
                println!(
                    "{}",
                    json!({
                        "v": 1,
                        "kind": "pvec",
                        "path": file.display().to_string(),
                        "valid": false,
                        "rule": rule,
                        "error": reason,
                    })
                );
            }
            operational_error(&format!(
                "{} is not a valid .pvec ({rule}): {reason}",
                file.display()
            ));
        }
    };
    let digest = match digest {
        DigestStatus::Verified => "verified",
        DigestStatus::NotWritten => "not_written",
    };
    let rasters: Vec<(u32, Option<ReadError>)> = reader
        .rasters()
        .into_iter()
        .map(|id| (id, reader.validate_raster(id).err()))
        .collect();
    // Status, rule, message and the recomputed residual.
    let (cal_status, cal_rule, cal_message, residual) = match reader.calibration() {
        Ok(None) => ("absent", None, None, None),
        Ok(Some(c)) => match c.georeference.as_ref().map(check_residual).transpose() {
            Ok(residual) => ("ok", None, None, residual),
            Err(e) => (
                "refused",
                Some(e.rule().to_string()),
                Some(e.to_string()),
                None,
            ),
        },
        Err(e) => ("refused", Some(e.rule()), Some(e.to_string()), None),
    };

    let registry = Registry::v1();
    let name = |id| registry.get(id).map_or("unknown", |c| c.name);
    // The streams viprs reads, decoded: capability (none when the stream table itself does not
    // read), name and what the decoder said.
    let mut undecoded: Vec<(Option<u32>, &str, String)> = Vec::new();
    let (streams, skipped_streams) = match PvecReader::open(bytes).and_then(|r| r.streams()) {
        Ok((streams, skipped)) => {
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
                if let Err(e) = decoded {
                    undecoded.push((Some(s.capability.get()), name(s.capability), e.to_string()));
                }
            }
            (streams.len(), skipped.len())
        }
        Err(e) => {
            undecoded.push((None, "components", chain(&e)));
            (0, 0)
        }
    };

    if as_json {
        let rasters: Vec<Value> = rasters
            .iter()
            .map(|(id, e)| {
                json!({
                    "raster": id,
                    "status": if e.is_some() { "skipped" } else { "ok" },
                    "rule": e.as_ref().map(ReadError::rule),
                    "message": e.as_ref().map(ToString::to_string),
                })
            })
            .collect();
        let undecoded: Vec<Value> = undecoded
            .iter()
            .map(|(capability, name, message)| {
                json!({ "capability": capability, "name": name, "message": message })
            })
            .collect();
        println!(
            "{}",
            json!({
                "v": 1,
                "kind": "pvec",
                "path": file.display().to_string(),
                "valid": true,
                "blocks": checked.blocks,
                "entities": checked.entities,
                "skipped_blocks": checked.skipped.len(),
                "artifact_digest": digest,
                "streams": streams,
                "skipped_streams": skipped_streams,
                "undecoded_streams": undecoded,
                "rasters": rasters,
                "calibration": {
                    "status": cal_status,
                    "rule": cal_rule,
                    "message": cal_message,
                    "residual": residual,
                },
            })
        );
        return;
    }

    let refused_rasters = rasters.iter().filter(|(_, e)| e.is_some()).count();
    println!(
        "{}: valid, {} blocks, {} entities, {streams} streams ({skipped_streams} skipped), {} \
         rasters ({refused_rasters} skipped), artifact digest {}",
        file.display(),
        checked.blocks,
        checked.entities,
        rasters.len(),
        digest.replace('_', " ")
    );
    for (id, e) in rasters
        .iter()
        .filter_map(|(id, e)| e.as_ref().map(|e| (id, e)))
    {
        println!("raster {id} skipped ({}): {e}", e.rule());
    }
    match (cal_status, cal_rule, cal_message) {
        ("refused", Some(rule), Some(message)) => {
            println!("calibration refused ({rule}): {message}");
        }
        ("ok", ..) => println!("calibration: ok"),
        _ => println!("calibration: none"),
    }
    for (capability, name, message) in &undecoded {
        match capability {
            Some(id) => println!("stream {id} ({name}) does not decode: {message}"),
            None => println!("the component streams do not read: {message}"),
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
        println!("{}", json!({ "v": 1, "kind": "pvec", "text": text }));
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
    // A calibration with no record cannot be stored (CAL-09), so the external id goes with the last one.
    let dropped_id = (scale.is_none() && georeference.is_none() && !external_id.is_empty())
        .then(|| external_id.clone());
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
    if let Some(id) = dropped_id {
        eprintln!(
            "Note: the external id {id:?} went with it; a calibration with no scale and no \
             georeference cannot be stored (CAL-09)"
        );
    }
}
