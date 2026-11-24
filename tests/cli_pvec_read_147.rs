//! EPIC CV (#147), phase 1: the `.pvec` read side. `viprs pvec info`, `validate`, `text`
//! and `calibrate` (CV1.1 to CV1.4). The cells check what PVec's own reader sees, or the
//! numbers the command prints against what went into the file, never an exit code on its own.
//!
//! The refusals the command line decides by itself run in every build. What needs PVec's facade
//! is behind `pvec-raster`, and a build without it has to say which feature it is missing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn viprs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
}

fn run(args: &[&str]) -> Output {
    viprs().args(args).output().expect("viprs must spawn")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the process must exit normally")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-cv1-147-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Exit 2 with every needle on stderr, and the file left exactly as it was.
#[track_caller]
fn usage(args: &[&str], needles: &[&str], file: &Path) {
    let before = std::fs::read(file).unwrap();
    let out = run(args);
    let err = stderr(&out);
    assert_eq!(code(&out), 2, "{args:?}:\n{err}");
    for needle in needles {
        assert!(err.contains(needle), "{args:?}: {needle:?} not in:\n{err}");
    }
    assert_eq!(
        std::fs::read(file).unwrap(),
        before,
        "a usage error must not touch the file"
    );
}

/// A file the refusals never get far enough to read.
fn placeholder(dir: &Path) -> PathBuf {
    let path = dir.join("placeholder.pvec");
    std::fs::write(&path, b"never opened").unwrap();
    path
}

#[test]
fn calibrate_needs_exactly_one_mode() {
    let dir = scratch("mode");
    let f = placeholder(&dir);
    usage(&["pvec", "calibrate", s(&f)], &["--scale"], &f);
    usage(
        &["pvec", "calibrate", s(&f), "--scale", "0.001", "--remove"],
        &["--remove"],
        &f,
    );
    usage(
        &[
            "pvec",
            "calibrate",
            s(&f),
            "--scale",
            "0.001",
            "--measured",
            "0,0,3,4",
            "--distance",
            "5",
        ],
        &["--measured"],
        &f,
    );
}

#[test]
fn calibrate_refuses_a_scale_that_is_not_a_positive_number() {
    let dir = scratch("scale");
    let f = placeholder(&dir);
    for bad in ["0", "-1", "nan", "inf", "metre"] {
        usage(
            &["pvec", "calibrate", s(&f), "--scale", bad],
            &["--scale"],
            &f,
        );
    }
}

#[test]
fn calibrate_plan_scale_needs_its_paper_unit_and_two_whole_numbers() {
    let dir = scratch("plan");
    let f = placeholder(&dir);
    usage(
        &["pvec", "calibrate", s(&f), "--plan-scale", "1:100"],
        &["--paper-unit"],
        &f,
    );
    usage(
        &["pvec", "calibrate", s(&f), "--paper-unit", "0.001"],
        &["--plan-scale"],
        &f,
    );
    for bad in ["1-100", "0:100", "1:0", "1:", "a:b", "1.5:100"] {
        usage(
            &[
                "pvec",
                "calibrate",
                s(&f),
                "--plan-scale",
                bad,
                "--paper-unit",
                "0.001",
            ],
            &["--plan-scale"],
            &f,
        );
    }
}

#[test]
fn calibrate_measured_needs_two_distinct_points_and_a_distance() {
    let dir = scratch("measured");
    let f = placeholder(&dir);
    usage(
        &["pvec", "calibrate", s(&f), "--measured", "0,0,3,4"],
        &["--distance"],
        &f,
    );
    usage(
        &["pvec", "calibrate", s(&f), "--distance", "5"],
        &["--measured"],
        &f,
    );
    for bad in ["0,0,3", "0,0,3,4,5", "0,0,x,4", "1,1,1,1"] {
        usage(
            &[
                "pvec",
                "calibrate",
                s(&f),
                "--measured",
                bad,
                "--distance",
                "5",
            ],
            &["--measured"],
            &f,
        );
    }
    usage(
        &[
            "pvec",
            "calibrate",
            s(&f),
            "--measured",
            "0,0,3,4",
            "--distance",
            "0",
        ],
        &["--distance"],
        &f,
    );
}

#[test]
fn calibrate_note_and_uncertainty_need_a_scale_to_belong_to() {
    let dir = scratch("note");
    let f = placeholder(&dir);
    usage(
        &["pvec", "calibrate", s(&f), "--remove", "--note", "survey"],
        &["--note"],
        &f,
    );
    usage(
        &[
            "pvec",
            "calibrate",
            s(&f),
            "--remove",
            "--uncertainty",
            "0.01",
        ],
        &["--uncertainty"],
        &f,
    );
    usage(
        &[
            "pvec",
            "calibrate",
            s(&f),
            "--scale",
            "1",
            "--uncertainty",
            "-0.5",
        ],
        &["--uncertainty"],
        &f,
    );
}

#[cfg(not(feature = "pvec-raster"))]
#[test]
fn a_build_without_pvec_raster_names_the_feature_for_every_read_command() {
    let dir = scratch("nofeature");
    let f = placeholder(&dir);
    for args in [
        vec!["pvec", "info", s(&f)],
        vec!["pvec", "validate", s(&f)],
        vec!["pvec", "text", s(&f)],
        vec!["pvec", "calibrate", s(&f), "--scale", "0.001"],
    ] {
        let out = run(&args);
        let err = stderr(&out);
        assert_eq!(code(&out), 1, "{args:?}:\n{err}");
        assert!(err.contains("pvec-raster"), "{args:?}:\n{err}");
    }
    assert_eq!(std::fs::read(&f).unwrap(), b"never opened");
}

#[cfg(feature = "pvec-raster")]
mod with_facade {
    use super::*;

    use pvec_capabilities::Registry;
    use pvec_components::{LayerStream, TextRecord, TextStream};
    use pvec_facade::components::Stream;
    use pvec_facade::unstable::{
        Calibration, ControlPoint, CrsKind, EffectiveScale, FitMethod, Georeference, Provenance,
        ScaleClaim,
    };
    use pvec_facade::{Bounds2, Curve, EntityId, Manifest, PvecReader, PvecWriter};
    use pvec_geometry::{Point2, Polyline};
    use serde_json::Value;

    #[track_caller]
    fn ok(args: &[&str]) -> Output {
        let out = run(args);
        assert_eq!(code(&out), 0, "{args:?}:\n{}", stderr(&out));
        out
    }

    fn stdout(out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn json(out: &Output) -> Value {
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{}", stdout(out)))
    }

    fn id(n: u32) -> EntityId {
        EntityId::new(n).unwrap()
    }

    fn manual_claim(metres: f64) -> ScaleClaim {
        ScaleClaim {
            metres_per_source_unit: metres,
            provenance: Provenance::Manual,
            uncertainty: None,
            note: String::new(),
        }
    }

    fn manual(metres: f64, note: &str) -> Calibration {
        Calibration {
            scale: Some(ScaleClaim {
                metres_per_source_unit: metres,
                provenance: Provenance::Manual,
                uncertainty: None,
                note: note.into(),
            }),
            georeference: None,
            external_id: String::new(),
        }
    }

    /// Two walls in millimetres on two layers, with two text records and, when asked, a calibration.
    fn drawing(
        dir: &Path,
        name: &str,
        with_streams: bool,
        calibration: Option<&Calibration>,
    ) -> PathBuf {
        let mut w = PvecWriter::new(Manifest {
            metres_per_unit: 0.001,
            bounds: Bounds2 {
                min: [0.0, 0.0],
                max: [4000.0, 4000.0],
            },
            capabilities: Vec::new(),
            producer_id: "viprs-cli-147".into(),
            producer_version: "1".into(),
        })
        .unwrap();
        for (n, y) in [(0u32, 10.0), (1, 20.0)] {
            let line = Polyline::new(vec![
                Point2 { x: 0.0, y },
                Point2 { x: 400.0, y },
                Point2 {
                    x: 400.0,
                    y: y + 300.0,
                },
            ])
            .unwrap();
            w.push(id(n), &Curve::Polyline(line)).unwrap();
        }
        if with_streams {
            let registry = Registry::v1();
            let layers = LayerStream {
                names: vec!["walls".into(), "doors".into()],
                flags: vec![0, 0],
                assignments: vec![(id(0), 0), (id(1), 1)],
            };
            w.components(layers.into_stream(&registry).unwrap())
                .unwrap();
            let text = TextStream {
                records: vec![
                    TextRecord {
                        entity: id(0),
                        x: 100.0,
                        y: 250.0,
                        height: 2.5,
                        rotation: 0.0,
                        text: "ROOM 101".into(),
                    },
                    TextRecord {
                        entity: id(1),
                        x: 40.0,
                        y: 60.0,
                        height: 1.75,
                        rotation: 90.0,
                        text: "Stair B".into(),
                    },
                ],
            };
            w.components(text.into_stream(&registry).unwrap()).unwrap();
        }
        if let Some(c) = calibration {
            w.calibration(c).unwrap();
        }
        let path = dir.join(name);
        std::fs::write(&path, w.finish().unwrap()).unwrap();
        path
    }

    fn open(path: &Path) -> PvecReader {
        PvecReader::open(std::fs::read(path).unwrap()).expect("PVec's reader must open the file")
    }

    /// PVec's own verdict: `pvec validate` from PVec's command line tool (the `pvec-conformance`
    /// crate), called as a library. It is PVec's code and not the calls viprs makes, so it is an
    /// oracle and not a mirror. Returns whether the file conforms, and PVec's JSON report.
    fn pvec_says(bytes: &[u8]) -> (bool, Value) {
        let outcome = pvec_conformance::commands::validate(bytes, "probe.pvec");
        let report = serde_json::from_str(&outcome.report.render()).expect("PVec's report is JSON");
        (outcome.exit == pvec_conformance::exit::CONFORMS, report)
    }

    /// A georeference to EPSG:2056 stated outright, with no control points.
    fn declared() -> Georeference {
        Georeference {
            source_to_crs: [0.5, 0.0, 0.0, 0.5, 2_600_000.0, 1_200_000.0],
            crs_kind: CrsKind::Projected,
            method: FitMethod::Declared,
            crs_id: "EPSG:2056".into(),
            definition: None,
            control_points: Vec::new(),
            residual_max: 0.0,
        }
    }

    /// A georeference fitted from three points the identity maps exactly, so its true residual
    /// is 0, storing `residual_max` as given: anything but 0 is the CAL-11 mismatch.
    fn fitted(residual_max: f64) -> Georeference {
        let point = |x: f64, y: f64| ControlPoint {
            source: [x, y],
            world: [x, y],
        };
        Georeference {
            source_to_crs: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            crs_kind: CrsKind::Engineering,
            method: FitMethod::Fitted,
            crs_id: "LOCAL:site".into(),
            definition: None,
            control_points: vec![point(0.0, 0.0), point(10.0, 0.0), point(0.0, 10.0)],
            residual_max,
        }
    }

    fn georeferenced(scale: Option<ScaleClaim>, georeference: Georeference) -> Calibration {
        Calibration {
            scale,
            georeference: Some(georeference),
            external_id: "survey-17".into(),
        }
    }

    /// A one-line drawing with a single text record holding `text`.
    fn drawing_with_text(dir: &Path, text: &str) -> PathBuf {
        let mut w = PvecWriter::new(Manifest {
            metres_per_unit: 1.0,
            bounds: Bounds2 {
                min: [0.0, 0.0],
                max: [10.0, 10.0],
            },
            capabilities: Vec::new(),
            producer_id: "viprs-cli-148".into(),
            producer_version: "1".into(),
        })
        .unwrap();
        let line =
            Polyline::new(vec![Point2 { x: 0.0, y: 0.0 }, Point2 { x: 4.0, y: 3.0 }]).unwrap();
        w.push(id(0), &Curve::Polyline(line)).unwrap();
        let record = TextStream {
            records: vec![TextRecord {
                entity: id(0),
                x: 1.0,
                y: 2.0,
                height: 0.5,
                rotation: 0.0,
                text: text.into(),
            }],
        };
        w.components(record.into_stream(&Registry::v1()).unwrap())
            .unwrap();
        let path = dir.join("text.pvec");
        std::fs::write(&path, w.finish().unwrap()).unwrap();
        path
    }

    // CV1.1: pvec info

    #[test]
    fn info_json_reports_the_manifest_streams_layers_text_and_the_manifest_scale() {
        let dir = scratch("info-json");
        let f = drawing(&dir, "plan.pvec", true, None);
        let v = json(&ok(&["pvec", "info", s(&f), "--json"]));
        assert_eq!(v["v"], 1);
        assert_eq!(v["kind"], "pvec");
        assert_eq!(v["producer_id"], "viprs-cli-147");
        assert_eq!(v["producer_version"], "1");
        assert_eq!(v["metres_per_unit"], 0.001);
        assert_eq!(v["bounds"]["min"], serde_json::json!([0.0, 0.0]));
        assert_eq!(v["bounds"]["max"], serde_json::json!([4000.0, 4000.0]));
        // PVec's writer declares the capability of every stream it carries, so the manifest lists
        // layer (14) and text (16) though the drawing asked for none: what the reader says is
        // what info has to print.
        let declared: Vec<u32> = open(&f)
            .manifest()
            .capabilities
            .iter()
            .map(|c| c.get())
            .collect();
        assert_eq!(declared, [14, 16]);
        assert_eq!(v["capabilities"], serde_json::json!(declared));
        assert_eq!(v["entities"], 2);
        let names: Vec<&str> = v["streams"]
            .as_array()
            .expect("streams is an array")
            .iter()
            .map(|st| st["name"].as_str().expect("every stream has a name"))
            .collect();
        assert_eq!(names, ["layer", "text"], "in file order: {v}");
        for st in v["streams"].as_array().unwrap() {
            assert!(st["capability"].as_u64().is_some(), "{st}");
            assert!(st["version"].as_str().is_some(), "{st}");
            assert!(st["bytes"].as_u64().is_some_and(|b| b > 0), "{st}");
        }
        assert_eq!(v["skipped"], serde_json::json!([]));
        assert_eq!(v["layers"], serde_json::json!(["walls", "doors"]));
        assert_eq!(v["text_records"], 2);
        assert_eq!(v["rasters"], 0);
        assert_eq!(v["scale"]["source"], "manifest");
        assert_eq!(v["scale"]["metres_per_unit"], 0.001);
        assert_eq!(v["georeference"], Value::Null);
    }

    #[test]
    fn info_text_says_the_same_in_lines() {
        let dir = scratch("info-text");
        let f = drawing(&dir, "plan.pvec", true, None);
        let text = stdout(&ok(&["pvec", "info", s(&f)]));
        for line in [
            "Producer: viprs-cli-147 1",
            "Entities: 2",
            "Layers: walls, doors",
            "Text records: 2",
            "Rasters: 0",
            "Scale: 0.001 m per unit (manifest)",
        ] {
            assert!(
                text.lines().any(|l| l.trim() == line),
                "{line:?} not in:\n{text}"
            );
        }
        for stream in ["layer (14) v", "text (16) v"] {
            assert!(
                text.lines().any(|l| l.trim_start().starts_with(stream)),
                "the stream line {stream:?} is not in:\n{text}"
            );
        }
    }

    #[test]
    fn info_reads_the_calibration_over_the_manifest_unit() {
        let dir = scratch("info-cal");
        let f = drawing(
            &dir,
            "plan.pvec",
            false,
            Some(&manual(0.002, "site survey")),
        );
        let v = json(&ok(&["pvec", "info", s(&f), "--json"]));
        assert_eq!(v["scale"]["source"], "calibration");
        assert_eq!(v["scale"]["metres_per_unit"], 0.002);
        assert_eq!(v["scale"]["provenance"], "manual");
        assert_eq!(v["scale"]["note"], "site survey");
        assert_eq!(v["scale"]["uncertainty"], Value::Null);
        assert_eq!(v["layers"], serde_json::json!([]));
        assert_eq!(v["text_records"], 0);
        let text = stdout(&ok(&["pvec", "info", s(&f)]));
        assert!(
            text.lines()
                .any(|l| l.trim() == "Scale: 0.002 m per unit (calibration, manual)"),
            "{text}"
        );
    }

    #[test]
    fn info_on_a_file_that_is_not_a_pvec_exits_1_and_says_so() {
        let dir = scratch("info-bad");
        let png = dir.join("plan.pvec");
        ok(&["test-image", s(&png), "--width", "8", "--height", "8"]);
        let out = run(&["pvec", "info", s(&png)]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(stderr(&out).contains("is not a .pvec"), "{}", stderr(&out));
        assert!(out.stdout.is_empty(), "nothing on stdout for a refusal");

        let missing = dir.join("missing.pvec");
        let out = run(&["pvec", "info", s(&missing)]);
        assert_eq!(code(&out), 1);
        assert!(stderr(&out).contains("missing.pvec"), "{}", stderr(&out));
    }

    // CV1.2: pvec validate

    #[test]
    fn validate_accepts_a_good_file_and_counts_what_pvec_counts() {
        let dir = scratch("validate-ok");
        let f = drawing(&dir, "plan.pvec", true, Some(&manual(0.001, "")));
        let (conforms, report) = pvec_says(&std::fs::read(&f).unwrap());
        assert!(conforms, "{report}");
        let text = stdout(&ok(&["pvec", "validate", s(&f)]));
        assert!(text.contains("valid"), "{text}");
        assert!(text.contains("2 entities"), "{text}");
        let v = json(&ok(&["pvec", "validate", s(&f), "--json"]));
        assert_eq!(v["v"], 1);
        assert_eq!(v["kind"], "pvec");
        assert_eq!(v["valid"], true);
        assert_eq!(v["entities"], 2);
        assert_eq!(
            v["blocks"], report["result"]["blocks"],
            "PVec's block count"
        );
        assert_eq!(
            v["entities"], report["result"]["entities"],
            "PVec's entity count"
        );
        assert_eq!(
            v["artifact_digest"], report["result"]["checks"]["artifact_digest"],
            "PVec's digest status"
        );
        assert_eq!(v["streams"], 2);
        assert_eq!(v["skipped_streams"], 0);
        assert_eq!(v["undecoded_streams"], serde_json::json!([]));
        assert_eq!(v["rasters"], serde_json::json!([]));
        assert_eq!(v["calibration"]["status"], "ok");
    }

    #[test]
    fn validate_refuses_a_truncated_file() {
        let dir = scratch("validate-cut");
        let f = drawing(&dir, "plan.pvec", true, None);
        let mut bytes = std::fs::read(&f).unwrap();
        bytes.truncate(bytes.len() - 16);
        std::fs::write(&f, &bytes).unwrap();
        assert!(!pvec_says(&bytes).0, "PVec refuses it too");
        let out = run(&["pvec", "validate", s(&f)]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(
            stderr(&out).contains("is not a valid .pvec"),
            "{}",
            stderr(&out)
        );
        let out = run(&["pvec", "validate", s(&f), "--json"]);
        assert_eq!(code(&out), 1);
        let v = json(&out);
        assert_eq!(v["kind"], "pvec");
        assert_eq!(v["valid"], false);
        assert!(v["rule"].as_str().is_some_and(|r| !r.is_empty()), "{v}");
        assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{v}");
    }

    #[test]
    fn validate_reports_a_cal11_residual_mismatch_and_the_file_still_conforms_as_pvec_says() {
        // The stored residual is 5 where the points give 0: CAL-11, which only a validator
        // recomputes. PVec reports the calibration refused and leaves the file conforming.
        let dir = scratch("validate-cal11");
        let f = drawing(
            &dir,
            "plan.pvec",
            false,
            Some(&georeferenced(None, fitted(5.0))),
        );
        let (conforms, report) = pvec_says(&std::fs::read(&f).unwrap());
        assert!(conforms, "{report}");
        assert_eq!(
            report["result"]["calibration"]["status"], "refused",
            "{report}"
        );
        assert_eq!(
            report["result"]["calibration"]["rule"], "CAL-11",
            "{report}"
        );

        let v = json(&ok(&["pvec", "validate", s(&f), "--json"]));
        assert_eq!(v["valid"], true);
        assert_eq!(v["calibration"]["status"], "refused");
        assert_eq!(v["calibration"]["rule"], "CAL-11");
        let text = stdout(&ok(&["pvec", "validate", s(&f)]));
        assert!(text.contains("calibration refused (CAL-11)"), "{text}");

        // The same georeference with the residual it really has passes the check.
        let good = drawing(
            &dir,
            "good.pvec",
            false,
            Some(&georeferenced(None, fitted(0.0))),
        );
        let v = json(&ok(&["pvec", "validate", s(&good), "--json"]));
        assert_eq!(v["calibration"]["status"], "ok");
        assert_eq!(v["calibration"]["residual"], 0.0);
    }

    #[test]
    fn validate_reports_a_refused_raster_with_its_rule_and_the_file_still_conforms_as_pvec_says() {
        // PVec's own hostile tile: a PNG that is also a ZIP, refused by RASTER-03 on read. The
        // geometry is sound, so PVec reports the raster skipped and the file conforms.
        use pvec_testkit::raster_hostile::{file as raster_file, hostile};
        let dir = scratch("validate-raster");
        let tile = hostile()
            .into_iter()
            .find(|t| t.name == "polyglot-png-and-zip")
            .expect("PVec's hostile corpus has the polyglot tile");
        let bytes = raster_file(&tile);
        let (conforms, report) = pvec_says(&bytes);
        assert!(conforms, "{report}");
        assert_eq!(report["result"]["rasters"][0]["status"], "skipped");
        assert_eq!(report["result"]["rasters"][0]["rule"], "RASTER-03");
        let f = dir.join("hostile.pvec");
        std::fs::write(&f, &bytes).unwrap();

        let v = json(&ok(&["pvec", "validate", s(&f), "--json"]));
        assert_eq!(v["valid"], true);
        let rasters = v["rasters"].as_array().expect("rasters is an array");
        assert_eq!(rasters.len(), 1, "{v}");
        assert_eq!(rasters[0]["raster"], 0);
        assert_eq!(rasters[0]["status"], "skipped");
        assert_eq!(rasters[0]["rule"], "RASTER-03");
        assert!(
            rasters[0]["message"]
                .as_str()
                .is_some_and(|m| m.contains("RASTER-03")),
            "{v}"
        );
        let text = stdout(&ok(&["pvec", "validate", s(&f)]));
        assert!(text.contains("raster 0 skipped (RASTER-03)"), "{text}");
    }

    #[test]
    fn validate_gives_pvecs_verdict_on_a_checksum_valid_file_whose_text_stream_does_not_decode() {
        // The writer stores a component payload as it is, unparsed, and seals the file, so the
        // block checksums and the digest hold and only a decode of the payload can find this.
        let dir = scratch("validate-undecodable");
        let mut w = PvecWriter::new(Manifest {
            metres_per_unit: 1.0,
            bounds: Bounds2 {
                min: [0.0, 0.0],
                max: [10.0, 10.0],
            },
            capabilities: Vec::new(),
            producer_id: "viprs-cli-148".into(),
            producer_version: "1".into(),
        })
        .unwrap();
        let line =
            Polyline::new(vec![Point2 { x: 0.0, y: 0.0 }, Point2 { x: 4.0, y: 3.0 }]).unwrap();
        w.push(id(0), &Curve::Polyline(line)).unwrap();
        let payload = vec![0xff, 0xff, 0xff];
        assert!(
            TextStream::decode(&payload).is_err(),
            "the payload must not decode"
        );
        let version = Registry::v1()
            .get(TextStream::CAPABILITY)
            .expect("the registry knows text")
            .version;
        w.components(Stream {
            capability: TextStream::CAPABILITY,
            version,
            payload,
        })
        .expect("the writer stores a component payload unparsed");
        let bytes = w.finish().expect("the writer seals it");
        let f = dir.join("undecodable.pvec");
        std::fs::write(&f, &bytes).unwrap();

        let (conforms, report) = pvec_says(&bytes);
        let out = run(&["pvec", "validate", s(&f), "--json"]);
        assert_eq!(
            code(&out) == 0,
            conforms,
            "PVec says conforming={conforms} ({report}), viprs exited {}:\n{}",
            code(&out),
            stderr(&out)
        );
        let v = json(&out);
        if conforms {
            // The verdict is PVec's; the stream PVec does not read is still reported.
            let undecoded = v["undecoded_streams"]
                .as_array()
                .expect("undecoded_streams is an array");
            assert_eq!(undecoded.len(), 1, "{v}");
            assert_eq!(undecoded[0]["capability"], 16);
            assert_eq!(undecoded[0]["name"], "text");
            let text = stdout(&ok(&["pvec", "validate", s(&f)]));
            assert!(text.contains("stream 16 (text) does not decode"), "{text}");
        } else {
            assert_eq!(v["valid"], false);
        }
        // Reading the text out of it is a different question, and that cannot be answered.
        let out = run(&["pvec", "text", s(&f)]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(stderr(&out).contains("text stream"), "{}", stderr(&out));
    }

    #[test]
    fn validate_agrees_with_the_library_on_every_flipped_byte() {
        // A byte flipped anywhere: validate has to refuse exactly the files PVec's own validator
        // refuses, so it neither misses a corruption PVec catches nor invents one.
        let dir = scratch("validate-flip");
        let f = drawing(&dir, "plan.pvec", true, Some(&manual(0.001, "")));
        let good = std::fs::read(&f).unwrap();
        let mut refused = 0;
        let step = (good.len() / 40).max(1);
        for at in (0..good.len()).step_by(step) {
            let mut bad = good.clone();
            bad[at] ^= 0x5a;
            let target = dir.join(format!("flip-{at}.pvec"));
            std::fs::write(&target, &bad).unwrap();
            let (expected, _) = pvec_says(&bad);
            let out = run(&["pvec", "validate", s(&target)]);
            assert_eq!(
                code(&out) == 0,
                expected,
                "byte {at}: PVec says conforming={expected}, viprs exited {}:\n{}",
                code(&out),
                stderr(&out)
            );
            refused += usize::from(!expected);
        }
        assert!(refused > 0, "no flip was refused; the probe proves nothing");
    }

    // CV1.3: pvec text

    #[test]
    fn text_lists_every_record_as_tab_separated_lines() {
        let dir = scratch("text-tsv");
        let f = drawing(&dir, "plan.pvec", true, None);
        let text = stdout(&ok(&["pvec", "text", s(&f)]));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "0\t100\t250\t2.5\t0\tROOM 101",
                "1\t40\t60\t1.75\t90\tStair B"
            ],
            "entity, x, y, height, rotation, text"
        );
    }

    #[test]
    fn text_json_round_trips_the_records() {
        let dir = scratch("text-json");
        let f = drawing(&dir, "plan.pvec", true, None);
        let v = json(&ok(&["pvec", "text", s(&f), "--json"]));
        assert_eq!(v["v"], 1);
        assert_eq!(
            v["text"],
            serde_json::json!([
                {"entity": 0, "x": 100.0, "y": 250.0, "height": 2.5, "rotation": 0.0, "text": "ROOM 101"},
                {"entity": 1, "x": 40.0, "y": 60.0, "height": 1.75, "rotation": 90.0, "text": "Stair B"},
            ])
        );
    }

    #[test]
    fn text_on_a_drawing_without_a_text_stream_is_empty_not_an_error() {
        let dir = scratch("text-none");
        let f = drawing(&dir, "plan.pvec", false, None);
        assert!(ok(&["pvec", "text", s(&f)]).stdout.is_empty());
        let v = json(&ok(&["pvec", "text", s(&f), "--json"]));
        assert_eq!(v["text"], serde_json::json!([]));
    }

    // CV1.4: pvec calibrate

    fn calibration_of(path: &Path) -> Option<Calibration> {
        open(path)
            .calibration()
            .expect("the calibration reads back")
    }

    #[test]
    fn calibrate_scale_writes_a_manual_claim_and_keeps_everything_else() {
        let dir = scratch("cal-scale");
        let f = drawing(&dir, "plan.pvec", true, None);
        let before = open(&f).streams().unwrap();
        ok(&[
            "pvec",
            "calibrate",
            s(&f),
            "--scale",
            "0.002",
            "--note",
            "from the title block",
        ]);
        let reader = open(&f);
        assert_eq!(reader.entity_count().unwrap(), 2);
        assert_eq!(
            reader.streams().unwrap(),
            before,
            "the streams are copied untouched"
        );
        let claim = calibration_of(&f)
            .and_then(|c| c.scale)
            .expect("a scale record");
        assert_eq!(claim.metres_per_source_unit, 0.002);
        assert_eq!(claim.provenance, Provenance::Manual);
        assert_eq!(claim.uncertainty, None);
        assert_eq!(claim.note, "from the title block");
        assert!(matches!(
            reader.effective_scale().unwrap(),
            EffectiveScale::Calibration { metres_per_source_unit, .. } if metres_per_source_unit == 0.002
        ));
    }

    #[test]
    fn calibrate_plan_scale_derives_the_unit_from_the_ratio() {
        let dir = scratch("cal-plan");
        let f = drawing(&dir, "plan.pvec", false, None);
        ok(&[
            "pvec",
            "calibrate",
            s(&f),
            "--plan-scale",
            "1:100",
            "--paper-unit",
            "0.001",
        ]);
        let claim = calibration_of(&f)
            .and_then(|c| c.scale)
            .expect("a scale record");
        assert_eq!(
            claim.provenance,
            Provenance::PlanScale {
                numerator: 1,
                denominator: 100,
                paper_metres_per_unit: 0.001,
            }
        );
        assert!(
            (claim.metres_per_source_unit - 0.1).abs() < 1e-12,
            "{claim:?}"
        );
    }

    #[test]
    fn calibrate_measured_divides_the_distance_by_the_length() {
        let dir = scratch("cal-measured");
        let f = drawing(&dir, "plan.pvec", false, None);
        ok(&[
            "pvec",
            "calibrate",
            s(&f),
            "--measured",
            "0,0,3,4",
            "--distance",
            "10",
            "--uncertainty",
            "0.01",
        ]);
        let claim = calibration_of(&f)
            .and_then(|c| c.scale)
            .expect("a scale record");
        assert_eq!(
            claim.provenance,
            Provenance::Measured {
                p0: [0.0, 0.0],
                p1: [3.0, 4.0],
                distance_metres: 10.0,
            }
        );
        assert_eq!(claim.metres_per_source_unit, 2.0);
        assert_eq!(claim.uncertainty, Some(0.01));
    }

    #[test]
    fn calibrate_replaces_one_calibration_with_another_and_remove_drops_it() {
        let dir = scratch("cal-remove");
        let f = drawing(&dir, "plan.pvec", true, Some(&manual(0.5, "old")));
        ok(&["pvec", "calibrate", s(&f), "--scale", "0.25"]);
        let claim = calibration_of(&f)
            .and_then(|c| c.scale)
            .expect("a scale record");
        assert_eq!(claim.metres_per_source_unit, 0.25);
        assert_eq!(claim.note, "");
        ok(&["pvec", "calibrate", s(&f), "--remove"]);
        assert_eq!(calibration_of(&f), None);
        assert!(matches!(
            open(&f).effective_scale().unwrap(),
            EffectiveScale::Manifest { metres_per_unit } if metres_per_unit == 0.001
        ));
        let v = json(&ok(&["pvec", "info", s(&f), "--json"]));
        assert_eq!(v["scale"]["source"], "manifest");
    }

    #[test]
    fn calibrate_on_a_file_that_is_not_a_pvec_exits_1_and_writes_nothing() {
        let dir = scratch("cal-bad");
        let f = dir.join("plan.pvec");
        std::fs::write(&f, b"not a pvec at all").unwrap();
        let out = run(&["pvec", "calibrate", s(&f), "--scale", "0.001"]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(stderr(&out).contains("plan.pvec"), "{}", stderr(&out));
        assert_eq!(std::fs::read(&f).unwrap(), b"not a pvec at all");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file is left beside it");
    }
    // Pins the PR promised without a test behind them (review of #149, item 5).

    #[test]
    fn info_json_reports_the_georeference_object() {
        let dir = scratch("info-georef");
        let f = drawing(
            &dir,
            "plan.pvec",
            false,
            Some(&georeferenced(Some(manual_claim(0.5)), declared())),
        );
        let v = json(&ok(&["pvec", "info", s(&f), "--json"]));
        assert_eq!(
            v["georeference"],
            serde_json::json!({
                "crs_id": "EPSG:2056",
                "crs_kind": "projected",
                "method": "declared",
                "source_to_crs": [0.5, 0.0, 0.0, 0.5, 2_600_000.0, 1_200_000.0],
                "control_points": 0,
                "residual_max": 0.0,
            })
        );
        let text = stdout(&ok(&["pvec", "info", s(&f)]));
        assert!(
            text.lines()
                .any(|l| l.trim() == "Georeference: EPSG:2056 (projected, 0 control points)"),
            "{text}"
        );
    }

    #[test]
    fn text_escapes_tab_newline_return_and_backslash_in_tsv_and_keeps_them_in_json() {
        let dir = scratch("text-escape");
        let raw = "a\tb\nc\rd\\e";
        let f = drawing_with_text(&dir, raw);
        let text = stdout(&ok(&["pvec", "text", s(&f)]));
        assert_eq!(text, "0\t1\t2\t0.5\t0\ta\\tb\\nc\\rd\\\\e\n");
        let v = json(&ok(&["pvec", "text", s(&f), "--json"]));
        assert_eq!(v["text"][0]["text"], raw, "JSON carries the text as it is");
    }

    #[test]
    fn calibrate_keeps_the_georeference_and_external_id_through_scale_and_remove() {
        let dir = scratch("cal-georef");
        let georeference = declared();
        let f = drawing(
            &dir,
            "plan.pvec",
            false,
            Some(&georeferenced(
                Some(manual_claim(0.5)),
                georeference.clone(),
            )),
        );
        ok(&["pvec", "calibrate", s(&f), "--scale", "0.25"]);
        let c = calibration_of(&f).expect("a calibration");
        assert_eq!(c.scale.map(|s| s.metres_per_source_unit), Some(0.25));
        assert_eq!(c.georeference.as_ref(), Some(&georeference));
        assert_eq!(c.external_id, "survey-17");

        ok(&["pvec", "calibrate", s(&f), "--remove"]);
        let c = calibration_of(&f).expect("the georeference keeps a calibration");
        assert_eq!(c.scale, None);
        assert_eq!(c.georeference.as_ref(), Some(&georeference));
        assert_eq!(c.external_id, "survey-17");
    }
}
