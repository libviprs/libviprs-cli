//! libviprs-cli#154 (CV2.5), `viprs pvec export FILE OUT --format gml|gpkg|mvt|pmtiles|step`, the
//! real binary, through `pvec-interchange` behind the `pvec-export` feature. An exit code is not
//! evidence of an export, so each format is checked by something that is not the writer: the GML by
//! an XML parser, the GeoPackage opened by a real SQLite, the vector tile decoded byte by byte, the
//! archive by its header, the STEP file by its Part 21 shape, and the fidelity report counted
//! against the number of entities the drawing has.
//!
//! The usage refusals run in every build, and a build without the feature has to say which one.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

#[path = "pvec_fixture_154/mod.rs"]
mod fx;

use fx::*;

const FORMATS: [&str; 5] = ["gml", "gpkg", "mvt", "pmtiles", "step"];

fn out_of(dir: &std::path::Path, format: &str) -> std::path::PathBuf {
    dir.join(format!("out.{format}"))
}

#[test]
fn export_needs_a_known_format() {
    let dir = scratch("format");
    let f = placeholder(&dir);
    let out = dir.join("out.bin");
    usage(&["pvec", "export", s(&f), s(&out)], &["--format"]);
    usage(
        &["pvec", "export", s(&f), s(&out), "--format", "shapefile"],
        &["shapefile"],
    );
    assert!(!out.exists(), "a usage error writes nothing");
}

#[test]
fn export_gml_and_gpkg_need_a_tolerance_and_the_others_do_not_take_one() {
    let dir = scratch("tolerance");
    let f = placeholder(&dir);
    for format in ["gml", "gpkg"] {
        let out = out_of(&dir, format);
        usage(
            &["pvec", "export", s(&f), s(&out), "--format", format],
            &["--tolerance"],
        );
        for bad in ["0", "-1", "nan", "inf", "fine", ""] {
            usage(
                &[
                    "pvec",
                    "export",
                    s(&f),
                    s(&out),
                    "--format",
                    format,
                    "--tolerance",
                    bad,
                ],
                &["--tolerance"],
            );
        }
    }
    for format in ["mvt", "pmtiles", "step"] {
        let out = out_of(&dir, format);
        usage(
            &[
                "pvec",
                "export",
                s(&f),
                s(&out),
                "--format",
                format,
                "--tolerance",
                "1",
            ],
            &["--tolerance"],
        );
    }
}

#[test]
fn export_flags_belong_to_the_formats_that_read_them() {
    let dir = scratch("belongs");
    let f = placeholder(&dir);
    let case = |format: &str, extra: &[&str], needle: &str| {
        let out = out_of(&dir, format);
        let mut args = vec!["pvec", "export", s(&f), s(&out), "--format", format];
        if format == "gml" || format == "gpkg" {
            args.extend(["--tolerance", "1"]);
        }
        args.extend(extra);
        usage(&args, &[needle]);
    };
    case("gml", &["--tile", "0/0/0"], "--tile");
    case("step", &["--zooms", "0-1"], "--zooms");
    case("gpkg", &["--srs", "urn:x"], "--srs");
    case("mvt", &["--zooms", "0-1"], "--zooms");
    case("pmtiles", &["--tile", "0/0/0"], "--tile");
    case("gpkg", &["--error-px", "0.5"], "--error-px");
    case("gml", &["--error-px", "0.5"], "--error-px");
    case("mvt", &["--affine", "1,0,0,1,0,0"], "--affine");
    case("pmtiles", &["--srs", "urn:x"], "--srs");
}

#[test]
fn export_refuses_a_value_of_the_wrong_shape() {
    let dir = scratch("shape");
    let f = placeholder(&dir);
    let at = |format: &str| out_of(&dir, format);
    for bad in [
        "1/2", "a/b/c", "0/1/0", "0/0/1", "2/4/0", "40/0/0", "-1/0/0", "",
    ] {
        usage(
            &[
                "pvec",
                "export",
                s(&f),
                s(&at("mvt")),
                "--format",
                "mvt",
                "--tile",
                bad,
            ],
            &["--tile"],
        );
    }
    for bad in ["3-1", "0-9", "x-2", "1", "0..2", ""] {
        usage(
            &[
                "pvec",
                "export",
                s(&f),
                s(&at("pmtiles")),
                "--format",
                "pmtiles",
                "--zooms",
                bad,
            ],
            &["--zooms"],
        );
    }
    for bad in [
        "1,0,0,1,0",
        "1,0,0,1,0,x",
        "0,0,0,0,0,0",
        "nan,0,0,1,0,0",
        "",
    ] {
        usage(
            &[
                "pvec",
                "export",
                s(&f),
                s(&at("gml")),
                "--format",
                "gml",
                "--tolerance",
                "1",
                "--affine",
                bad,
            ],
            &["--affine"],
        );
    }
    for bad in ["0", "-1", "nan", "inf", ""] {
        usage(
            &[
                "pvec",
                "export",
                s(&f),
                s(&at("mvt")),
                "--format",
                "mvt",
                "--error-px",
                bad,
            ],
            &["--error-px"],
        );
    }
}

#[test]
fn export_will_not_write_over_its_own_input() {
    let dir = scratch("same");
    let f = placeholder(&dir);
    let before = std::fs::read(&f).unwrap();
    usage(
        &["pvec", "export", s(&f), s(&f), "--format", "step"],
        &["input"],
    );
    assert_eq!(std::fs::read(&f).unwrap(), before);
}

/// Exit 2 naming --report, and every file exactly as it was (the report is never written over
/// the drawing, or over the export it belongs to).
#[track_caller]
fn report_refused(f: &std::path::Path, out: &std::path::Path, report: &std::path::Path) {
    let before = std::fs::read(f).unwrap();
    let out_before = std::fs::read(out).ok();
    usage(
        &[
            "pvec",
            "export",
            s(f),
            s(out),
            "--format",
            "gml",
            "--tolerance",
            "1",
            "--report",
            s(report),
        ],
        &["--report"],
    );
    assert_eq!(std::fs::read(f).unwrap(), before, "the input is untouched");
    assert_eq!(std::fs::read(out).ok(), out_before, "OUT is untouched");
}

#[test]
fn export_report_may_not_be_the_input() {
    let dir = scratch("report-input");
    let f = placeholder(&dir);
    report_refused(&f, &dir.join("o.gml"), &f);
}

#[test]
fn export_report_may_not_be_out() {
    let dir = scratch("report-out");
    let f = placeholder(&dir);
    let out = dir.join("o.gml");
    report_refused(&f, &out, &out);
}

#[test]
fn export_report_may_not_reach_the_input_or_out_by_another_name() {
    let dir = scratch("report-alias");
    let f = placeholder(&dir);
    let out = dir.join("o.gml");
    std::fs::write(&out, b"an earlier export").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // a path with a detour in it
    report_refused(
        &f,
        &out,
        &dir.join("sub").join("..").join("placeholder.pvec"),
    );
    report_refused(&f, &out, &dir.join("sub").join("..").join("o.gml"));
    // a relative name for a file that does not exist yet is the same file as OUT
    let fresh = dir.join("fresh.gml");
    report_refused(&f, &fresh, &dir.join("sub").join("..").join("fresh.gml"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&f, dir.join("link-to-input")).unwrap();
        report_refused(&f, &out, &dir.join("link-to-input"));
        std::os::unix::fs::symlink(&out, dir.join("link-to-out")).unwrap();
        report_refused(&f, &out, &dir.join("link-to-out"));
        // a hard link is the same file too
        std::fs::hard_link(&f, dir.join("hard-to-input")).unwrap();
        report_refused(&f, &out, &dir.join("hard-to-input"));
    }
}

#[test]
fn export_out_may_not_reach_the_input_through_a_link() {
    let dir = scratch("out-alias");
    let f = placeholder(&dir);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&f, dir.join("link.gml")).unwrap();
        let before = std::fs::read(&f).unwrap();
        usage(
            &[
                "pvec",
                "export",
                s(&f),
                s(&dir.join("link.gml")),
                "--format",
                "step",
            ],
            &["input"],
        );
        assert_eq!(std::fs::read(&f).unwrap(), before);
    }
}

#[cfg(not(feature = "pvec-export"))]
#[test]
fn a_build_without_pvec_export_names_the_feature() {
    let dir = scratch("no-feature");
    let f = placeholder(&dir);
    for format in FORMATS {
        let out = out_of(&dir, format);
        let mut args = vec!["pvec", "export", s(&f), s(&out), "--format", format];
        if format == "gml" || format == "gpkg" {
            args.extend(["--tolerance", "1"]);
        }
        let result = run(&args);
        assert_eq!(code(&result), 1, "{format}: {}", stderr(&result));
        assert!(
            stderr(&result).contains("pvec-export"),
            "{format}: {}",
            stderr(&result)
        );
        assert!(!out.exists());
    }
}

#[cfg(feature = "pvec-export")]
mod with_export {
    use super::*;

    use serde_json::Value;

    fn export(
        dir: &std::path::Path,
        format: &str,
        extra: &[&str],
    ) -> (std::path::PathBuf, std::process::Output) {
        let f = drawing(dir);
        let out = out_of(dir, format);
        let mut args = vec!["pvec", "export", s(&f), s(&out), "--format", format];
        args.extend(extra);
        let result = ok(&args);
        (out, result)
    }

    fn report_of(
        dir: &std::path::Path,
        format: &str,
        extra: &[&str],
    ) -> (std::path::PathBuf, Value) {
        let report = dir.join(format!("report-{format}.json"));
        let mut args = vec!["--report", s(&report)];
        args.extend(extra);
        let (out, _) = export(dir, format, &args);
        let text = std::fs::read_to_string(&report).expect("--report writes the file");
        (
            out,
            serde_json::from_str(&text).expect("the report is JSON"),
        )
    }

    /// How many entities the drawing has, by PVec's own reader.
    fn entity_count(dir: &std::path::Path) -> u64 {
        open(&dir.join("drawing.pvec")).entity_count().unwrap()
    }

    fn by_fidelity(report: &Value, kind: &str) -> u64 {
        report["entities"]["by_fidelity"][kind]
            .as_u64()
            .unwrap_or_else(|| panic!("no {kind}: {report}"))
    }

    /// The report is counted against the artifact: one row per entity of the drawing, and the
    /// five fidelity values add up to all of them.
    #[track_caller]
    fn counted(report: &Value, entities: u64) {
        assert_eq!(report["entities"]["total"], entities, "{report}");
        let rows = report["entities"]["rows"].as_array().unwrap();
        assert_eq!(rows.len() as u64, entities);
        let mut seen: Vec<u64> = rows.iter().map(|r| r["entity"].as_u64().unwrap()).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..entities).collect::<Vec<_>>());
        let sum: u64 = [
            "Exact",
            "ExactReparameterized",
            "EquivalentCompound",
            "BoundedApproximation",
            "Unsupported",
        ]
        .iter()
        .map(|k| by_fidelity(report, k))
        .sum();
        assert_eq!(sum, entities, "{report}");
        let digest = report["source_digest"].as_str().unwrap();
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        assert_eq!(hex.len(), 64, "{digest}");
    }

    // ---- GML ------------------------------------------------------------------------------

    #[test]
    fn gml_parses_names_every_entity_and_its_report_counts_them() {
        let dir = scratch("gml");
        let (out, report) = report_of(
            &dir,
            "gml",
            &["--tolerance", "0.5", "--srs", "urn:ogc:def:crs:EPSG::3857"],
        );
        let xml = std::fs::read_to_string(&out).unwrap();
        let doc = roxmltree::Document::parse(&xml).expect("the GML is well-formed XML");
        let root = doc.root_element();
        assert_eq!(root.tag_name().name(), "MultiCurve");
        assert_eq!(
            root.tag_name().namespace(),
            Some("http://www.opengis.net/gml/3.2")
        );
        assert_eq!(
            root.attribute("srsName"),
            Some("urn:ogc:def:crs:EPSG::3857")
        );
        let mut ids: Vec<u32> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "identifier")
            .filter(|n| n.attribute("codeSpace") == Some("urn:pvec:entity"))
            .map(|n| n.text().unwrap().trim().parse().unwrap())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, [0, 1, 2, 3], "every entity is a member");
        counted(&report, entity_count(&dir));
        assert_eq!(report["target"]["name"], "GML");
    }

    #[test]
    fn gml_affine_is_applied_to_the_coordinates() {
        let dir = scratch("gml-affine");
        let (out, _) = export(
            &dir,
            "gml",
            &["--tolerance", "0.001", "--affine", "0.001,0,0,0.001,0,0"],
        );
        let xml = std::fs::read_to_string(&out).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let numbers: Vec<f64> = doc
            .descendants()
            .filter(|n| matches!(n.tag_name().name(), "posList" | "pos"))
            .flat_map(|n| {
                n.text()
                    .unwrap()
                    .split_whitespace()
                    .map(|v| v.parse::<f64>().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect();
        let largest = numbers.iter().cloned().fold(f64::MIN, f64::max);
        assert!(
            (3.4..3.6).contains(&largest),
            "3500 source units are 3.5 after the map: {largest}"
        );
    }

    // ---- GeoPackage -----------------------------------------------------------------------

    #[test]
    fn gpkg_opens_in_sqlite_holds_every_entity_and_its_report_counts_them() {
        let dir = scratch("gpkg");
        let (out, report) = report_of(&dir, "gpkg", &["--tolerance", "0.5"]);
        let db =
            rusqlite::Connection::open_with_flags(&out, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("a real SQLite opens the file");
        let check: String = db
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        let app: i64 = db
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap();
        assert_eq!(app, 0x4750_4B47, "the GeoPackage application id");
        let kind: String = db
            .query_row(
                "SELECT data_type FROM gpkg_contents WHERE table_name = 'pvec_features'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "features");
        let mut stmt = db
            .prepare("SELECT entity_id, fidelity, error_bound, substr(geom, 1, 2) FROM pvec_features ORDER BY entity_id")
            .unwrap();
        let rows: Vec<(i64, String, Option<f64>, Vec<u8>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [0, 1, 2, 3]);
        for (entity, _, _, magic) in &rows {
            assert_eq!(
                magic, b"GP",
                "entity {entity} has a GeoPackage geometry blob"
            );
        }
        for (entity, fidelity, bound, _) in &rows {
            if fidelity == "BoundedApproximation" {
                assert!(
                    bound.unwrap() <= 0.5,
                    "entity {entity} claims {bound:?} against a budget of 0.5"
                );
            }
        }
        counted(&report, entity_count(&dir));
        // The database and the report say the same about each entity.
        for row in report["entities"]["rows"].as_array().unwrap() {
            let entity = row["entity"].as_i64().unwrap();
            let stored = &rows.iter().find(|r| r.0 == entity).unwrap().1;
            assert_eq!(row["fidelity"].as_str().unwrap(), stored, "entity {entity}");
        }
    }

    // ---- vector tiles ---------------------------------------------------------------------

    fn varint(buf: &[u8], at: &mut usize) -> u64 {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let b = buf[*at];
            *at += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    }

    /// The fields of one protobuf message: (field number, wire type, varint value or bytes).
    fn fields(buf: &[u8]) -> Vec<(u64, u64, u64, Vec<u8>)> {
        let (mut at, mut out) = (0, Vec::new());
        while at < buf.len() {
            let key = varint(buf, &mut at);
            let (field, wire) = (key >> 3, key & 7);
            match wire {
                0 => out.push((field, wire, varint(buf, &mut at), Vec::new())),
                1 => {
                    out.push((field, wire, 0, buf[at..at + 8].to_vec()));
                    at += 8;
                }
                2 => {
                    let n = varint(buf, &mut at) as usize;
                    out.push((field, wire, 0, buf[at..at + n].to_vec()));
                    at += n;
                }
                5 => {
                    out.push((field, wire, 0, buf[at..at + 4].to_vec()));
                    at += 4;
                }
                other => panic!("wire type {other} is not protobuf"),
            }
        }
        assert_eq!(at, buf.len(), "the message ends where it says");
        out
    }

    /// A feature: its id, its geometry type and its geometry commands.
    type Feature = (u64, u64, Vec<u8>);

    /// A layer: its name, its extent and its features.
    type Layer = (String, u64, Vec<Feature>);

    /// Every layer of a tile.
    fn decode_tile(bytes: &[u8]) -> Vec<Layer> {
        fields(bytes)
            .into_iter()
            .map(|(field, wire, _, layer)| {
                assert_eq!(
                    (field, wire),
                    (3, 2),
                    "a tile holds layers and nothing else"
                );
                let mut name = String::new();
                let (mut extent, mut version) = (0, 0);
                let mut features = Vec::new();
                for (f, _, v, payload) in fields(&layer) {
                    match f {
                        1 => name = String::from_utf8(payload).unwrap(),
                        2 => {
                            let (mut id, mut kind, mut geometry) = (u64::MAX, 0, Vec::new());
                            for (ff, _, fv, fp) in fields(&payload) {
                                match ff {
                                    1 => id = fv,
                                    3 => kind = fv,
                                    4 => geometry = fp,
                                    _ => {}
                                }
                            }
                            features.push((id, kind, geometry));
                        }
                        5 => extent = v,
                        15 => version = v,
                        _ => {}
                    }
                }
                assert_eq!(version, 2, "layer {name}");
                (name, extent, features)
            })
            .collect()
    }

    #[test]
    fn mvt_whole_drawing_tile_decodes_and_holds_every_entity_as_a_linestring() {
        let dir = scratch("mvt");
        let (out, result) = export(&dir, "mvt", &[]);
        let layers = decode_tile(&std::fs::read(&out).unwrap());
        assert!(!layers.is_empty());
        let mut ids: Vec<u64> = layers
            .iter()
            .flat_map(|l| l.2.iter().map(|f| f.0))
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, [0, 1, 2, 3]);
        for (name, extent, features) in &layers {
            assert_eq!(*extent, 4096, "layer {name}");
            for (id, kind, geometry) in features {
                assert_eq!(*kind, 2, "entity {id} is a LINESTRING");
                assert!(geometry.len() >= 4, "entity {id} has a MoveTo and a LineTo");
                let mut at = 0;
                assert_eq!(
                    varint(geometry, &mut at) & 7,
                    1,
                    "entity {id} starts with MoveTo"
                );
            }
        }
        assert!(
            stderr(&result).contains("lossy"),
            "a display-only export says so: {}",
            stderr(&result)
        );
    }

    #[test]
    fn mvt_a_deeper_tile_holds_only_what_reaches_it() {
        let dir = scratch("mvt-tile");
        let (out, _) = export(&dir, "mvt", &["--tile", "3/6/1"]);
        let layers = decode_tile(&std::fs::read(&out).unwrap());
        let ids: Vec<u64> = layers
            .iter()
            .flat_map(|l| l.2.iter().map(|f| f.0))
            .collect();
        assert_eq!(ids, [2], "only the far polyline reaches 3/6/1");
        let (empty, _) = export(&dir, "mvt", &["--tile", "3/7/0"]);
        assert!(
            decode_tile(&std::fs::read(&empty).unwrap()).is_empty(),
            "nothing reaches 3/7/0"
        );
    }

    #[test]
    fn mvt_report_counts_the_entities_that_reach_the_tile_and_calls_them_display_only() {
        let dir = scratch("mvt-report");
        let (_, report) = report_of(&dir, "mvt", &[]);
        counted(&report, 4);
        assert_eq!(by_fidelity(&report, "BoundedApproximation"), 4, "{report}");
        assert_eq!(by_fidelity(&report, "Exact"), 0);
        assert_eq!(report["conformance"]["display"]["claimed"], true);
        assert_eq!(report["conformance"]["geometric"]["claimed"], false);
    }

    // ---- PMTiles --------------------------------------------------------------------------

    #[test]
    fn pmtiles_archive_has_a_v3_header_over_vector_tiles_and_the_zoom_range_asked() {
        let dir = scratch("pmtiles");
        let (out, _) = export(&dir, "pmtiles", &["--zooms", "0-2"]);
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.len() > 127);
        assert_eq!(&bytes[..7], b"PMTiles");
        assert_eq!(bytes[7], 3, "version 3");
        let addressed = u64::from_le_bytes(bytes[72..80].try_into().unwrap());
        assert!(
            addressed >= 3,
            "at least one tile in each of three zooms: {addressed}"
        );
        assert_eq!(bytes[99], 1, "tile type 1 is MVT");
        assert_eq!((bytes[100], bytes[101]), (0, 2), "min and max zoom");
    }

    #[test]
    fn pmtiles_report_counts_every_entity_once_across_tiles() {
        let dir = scratch("pmtiles-report");
        let (_, report) = report_of(&dir, "pmtiles", &["--zooms", "0-2"]);
        counted(&report, 4);
        assert_eq!(report["target"]["name"], "PMTiles");
    }

    // ---- STEP -----------------------------------------------------------------------------

    #[test]
    fn step_is_a_part_21_file_with_the_curves_in_it_and_its_report_counts_them() {
        let dir = scratch("step");
        let (out, report) = report_of(&dir, "step", &[]);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(
            text.starts_with("ISO-10303-21;"),
            "{}",
            &text[..40.min(text.len())]
        );
        assert!(text.trim_end().ends_with("END-ISO-10303-21;"));
        assert!(text.contains("AP242"), "the schema is named");
        assert!(
            text.contains("CIRCLE("),
            "the circle is a circle, not a polyline"
        );
        assert!(text.contains("POLYLINE("), "the polylines are polylines");
        counted(&report, entity_count(&dir));
        assert_eq!(report["target"]["name"], "STEP AP242");
    }

    // ---- the report, the note on stderr, determinism, failures ------------------------------

    #[test]
    fn a_lossy_export_says_so_on_stderr_exactly_when_its_report_says_it_was() {
        for format in ["gml", "gpkg", "step"] {
            let dir = scratch(&format!("lossy-{format}"));
            let extra: &[&str] = if format == "step" {
                &[]
            } else {
                &["--tolerance", "0.5"]
            };
            let report = dir.join("r.json");
            let mut args = vec!["--report", s(&report)];
            args.extend(extra);
            let (_, result) = export(&dir, format, &args);
            let report: Value =
                serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
            let lossy =
                by_fidelity(&report, "BoundedApproximation") + by_fidelity(&report, "Unsupported");
            assert_eq!(
                stderr(&result).contains("lossy"),
                lossy > 0,
                "{format}: {lossy} lossy entities, stderr:\n{}",
                stderr(&result)
            );
        }
    }

    #[test]
    fn a_report_path_ending_in_md_is_markdown() {
        let dir = scratch("md");
        let report = dir.join("r.md");
        export(&dir, "step", &["--report", s(&report)]);
        let text = std::fs::read_to_string(&report).unwrap();
        assert!(text.starts_with("# Fidelity report: STEP AP242"), "{text}");
    }

    #[test]
    fn every_export_is_byte_identical_when_run_twice() {
        for (format, extra) in [
            ("gml", &["--tolerance", "0.5"][..]),
            ("gpkg", &["--tolerance", "0.5"][..]),
            ("mvt", &[][..]),
            ("pmtiles", &["--zooms", "0-1"][..]),
            ("step", &[][..]),
        ] {
            let dir = scratch(&format!("twice-{format}"));
            let (first, _) = export(&dir, format, extra);
            let a = std::fs::read(&first).unwrap();
            let (second, _) = export(&dir, format, extra);
            assert_eq!(a, std::fs::read(&second).unwrap(), "{format}");
        }
    }

    #[test]
    fn a_file_that_is_not_a_pvec_exits_1_and_writes_nothing() {
        let dir = scratch("not-pvec");
        let f = not_a_pvec(&dir);
        for format in FORMATS {
            let out = out_of(&dir, format);
            let mut args = vec!["pvec", "export", s(&f), s(&out), "--format", format];
            if format == "gml" || format == "gpkg" {
                args.extend(["--tolerance", "1"]);
            }
            let result = run(&args);
            assert_eq!(code(&result), 1, "{format}: {}", stderr(&result));
            assert!(
                stderr(&result).contains("not a .pvec"),
                "{format}: {}",
                stderr(&result)
            );
            assert!(!out.exists(), "{format}: a failed export leaves no file");
        }
    }

    #[test]
    fn a_report_that_cannot_be_written_is_exit_1() {
        let dir = scratch("report-fail");
        let f = drawing(&dir);
        let out = out_of(&dir, "step");
        let report = dir.join("no-such-dir").join("r.json");
        let result = run(&[
            "pvec",
            "export",
            s(&f),
            s(&out),
            "--format",
            "step",
            "--report",
            s(&report),
        ]);
        assert_eq!(code(&result), 1, "{}", stderr(&result));
    }
}
