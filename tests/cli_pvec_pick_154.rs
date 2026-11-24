//! libviprs-cli#154 (CV2.5), `viprs pvec pick FILE`: point, rectangle and nearest queries over
//! the entities of a `.pvec`, through the facade's `PickIndex`. The cells compare what the command
//! prints with what `PickIndex` answers for the same file and the same query, and with the numbers
//! worked out by hand for the shared drawing, never an exit code on its own.
//!
//! The refusals the command line decides by itself run in every build; what needs PVec's facade
//! is behind `pvec-raster`.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

#[path = "pvec_fixture_154/mod.rs"]
mod fx;

use fx::*;

#[test]
fn pick_needs_exactly_one_query() {
    let dir = scratch("one-query");
    let f = placeholder(&dir);
    usage(&["pvec", "pick", s(&f)], &["--point"]);
    usage(
        &[
            "pvec",
            "pick",
            s(&f),
            "--point",
            "1,1",
            "--tolerance",
            "1",
            "--nearest",
            "1,1",
        ],
        &["--nearest"],
    );
    usage(
        &[
            "pvec",
            "pick",
            s(&f),
            "--rect",
            "0,0,1,1",
            "--nearest",
            "1,1",
        ],
        &["--rect"],
    );
}

#[test]
fn pick_point_needs_a_tolerance_and_a_rectangle_does_not_take_one() {
    let dir = scratch("tolerance");
    let f = placeholder(&dir);
    usage(&["pvec", "pick", s(&f), "--point", "1,1"], &["--tolerance"]);
    usage(
        &[
            "pvec",
            "pick",
            s(&f),
            "--rect",
            "0,0,1,1",
            "--tolerance",
            "1",
        ],
        &["--tolerance"],
    );
    for bad in ["-1", "nan", "inf", "wide", ""] {
        usage(
            &["pvec", "pick", s(&f), "--point", "1,1", "--tolerance", bad],
            &["--tolerance"],
        );
    }
}

#[test]
fn pick_refuses_a_point_or_rectangle_of_the_wrong_shape() {
    let dir = scratch("shape");
    let f = placeholder(&dir);
    for bad in ["1", "1,2,3", "x,2", "nan,2", "1,inf", ""] {
        usage(&["pvec", "pick", s(&f), "--nearest", bad], &["--nearest"]);
    }
    for bad in ["1,2,3", "0,0,1,x", "5,5,1,1", "0,5,1,1", "nan,0,1,1"] {
        usage(&["pvec", "pick", s(&f), "--rect", bad], &["--rect"]);
    }
}

#[test]
fn pick_mode_belongs_to_a_rectangle_and_has_two_values() {
    let dir = scratch("mode");
    let f = placeholder(&dir);
    usage(
        &[
            "pvec",
            "pick",
            s(&f),
            "--nearest",
            "1,1",
            "--mode",
            "contains",
        ],
        &["--mode"],
    );
    usage(
        &[
            "pvec",
            "pick",
            s(&f),
            "--rect",
            "0,0,1,1",
            "--mode",
            "overlaps",
        ],
        &["overlaps"],
    );
}

#[cfg(feature = "pvec-raster")]
mod with_facade {
    use super::*;

    use pvec_facade::{Bounds2, PickIndex, PickTolerance, RectMode, Transform2};

    fn index(path: &std::path::Path) -> PickIndex {
        let reader = open(path);
        let mut index = PickIndex::new(&reader).unwrap();
        for (entity, curve) in reader.entities().unwrap() {
            // A bare line has no bounds and the index refuses it; the command says so too.
            let _ = index.insert_exact(entity, curve);
        }
        index
    }

    fn hit(out: &std::process::Output) -> serde_json::Value {
        json(out)["hit"].clone()
    }

    #[test]
    fn point_hits_the_entity_under_it_and_reports_what_the_index_reports() {
        let dir = scratch("point");
        let f = drawing(&dir);
        let out = ok(&[
            "pvec",
            "pick",
            s(&f),
            "--point",
            "200,2",
            "--tolerance",
            "3",
            "--json",
        ]);
        let h = hit(&out);
        assert_eq!(h["entity"], 0);
        let oracle = index(&f)
            .query_point(
                pvec_facade::Point2 { x: 200.0, y: 2.0 },
                PickTolerance::new(3.0, &Transform2::IDENTITY).unwrap(),
            )
            .expect("the index hits too");
        assert_eq!(h["entity"], oracle.entity.index());
        assert!((h["distance"].as_f64().unwrap() - oracle.distance).abs() < 1e-9);
        assert!((h["distance"].as_f64().unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(h["capability"], "polyline");
        assert_eq!(h["bounds"], serde_json::json!([0.0, 0.0, 400.0, 300.0]));
        assert!(
            h["param"].is_number(),
            "an exact curve has a parameter: {h}"
        );
    }

    #[test]
    fn point_outside_its_tolerance_hits_nothing_and_still_exits_0() {
        let dir = scratch("miss");
        let f = drawing(&dir);
        let out = ok(&[
            "pvec",
            "pick",
            s(&f),
            "--point",
            "200,2",
            "--tolerance",
            "1",
            "--json",
        ]);
        assert!(hit(&out).is_null(), "{}", stdout(&out));
        let text = ok(&[
            "pvec",
            "pick",
            s(&f),
            "--point",
            "200,2",
            "--tolerance",
            "1",
        ]);
        assert!(stdout(&text).trim().is_empty(), "{}", stdout(&text));
        assert!(stderr(&text).contains("no hit"), "{}", stderr(&text));
    }

    #[test]
    fn nearest_finds_the_circle_and_its_distance_wherever_it_is() {
        let dir = scratch("nearest");
        let f = drawing(&dir);
        let out = ok(&["pvec", "pick", s(&f), "--nearest", "1000,1300", "--json"]);
        let h = hit(&out);
        assert_eq!(h["entity"], 1);
        assert!(
            (h["distance"].as_f64().unwrap() - 100.0).abs() < 1e-6,
            "{h}"
        );
        assert_eq!(h["capability"], "circle");
        let oracle = index(&f)
            .query_nearest(
                pvec_facade::Point2 {
                    x: 1000.0,
                    y: 1300.0,
                },
                f64::INFINITY,
            )
            .unwrap();
        assert_eq!(h["entity"], oracle.entity.index());
        assert_eq!(h["param"].as_f64(), oracle.param);
    }

    #[test]
    fn nearest_with_a_tolerance_is_a_largest_distance() {
        let dir = scratch("nearest-max");
        let f = drawing(&dir);
        let near = ok(&[
            "pvec",
            "pick",
            s(&f),
            "--nearest",
            "1000,1300",
            "--tolerance",
            "150",
            "--json",
        ]);
        assert_eq!(hit(&near)["entity"], 1);
        let far = ok(&[
            "pvec",
            "pick",
            s(&f),
            "--nearest",
            "1000,1300",
            "--tolerance",
            "50",
            "--json",
        ]);
        assert!(hit(&far).is_null());
    }

    #[test]
    fn rect_selects_by_intersection_or_containment_as_the_index_does() {
        let dir = scratch("rect");
        let f = drawing(&dir);
        let idx = index(&f);
        for (rect, mode, want) in [
            ("0,0,500,500", "intersects", vec![0u32]),
            ("0,0,1000,1000", "intersects", vec![0, 1]),
            ("0,0,1000,1000", "contains", vec![0]),
            ("0,0,3200,3200", "contains", vec![0, 1, 3]),
            ("0,1900,1600,2700", "contains", vec![3]),
            ("0,0,4000,4000", "contains", vec![0, 1, 2, 3]),
            ("3600,0,4000,100", "intersects", vec![]),
        ] {
            let out = ok(&[
                "pvec",
                "pick",
                s(&f),
                "--rect",
                rect,
                "--mode",
                mode,
                "--json",
            ]);
            let got: Vec<u32> = json(&out)["entities"]
                .as_array()
                .unwrap_or_else(|| panic!("no entity list: {}", stdout(&out)))
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect();
            assert_eq!(got, want, "--rect {rect} --mode {mode}");
            let n: Vec<f64> = rect.split(',').map(|v| v.parse().unwrap()).collect();
            let oracle = idx.query_rect(
                Bounds2 {
                    min: [n[0], n[1]],
                    max: [n[2], n[3]],
                },
                if mode == "contains" {
                    RectMode::Contains
                } else {
                    RectMode::Intersects
                },
            );
            assert_eq!(
                got,
                oracle.as_slice(),
                "the index disagrees for {rect} {mode}"
            );
        }
    }

    #[test]
    fn rect_defaults_to_intersects_and_prints_one_id_per_line() {
        let dir = scratch("rect-text");
        let f = drawing(&dir);
        let out = ok(&["pvec", "pick", s(&f), "--rect", "0,0,1000,1000"]);
        assert_eq!(stdout(&out), "0\n1\n");
    }

    #[test]
    fn point_text_is_one_tab_separated_line() {
        let dir = scratch("text");
        let f = drawing(&dir);
        let out = ok(&[
            "pvec",
            "pick",
            s(&f),
            "--point",
            "200,0",
            "--tolerance",
            "1",
        ]);
        let text = stdout(&out);
        let fields: Vec<&str> = text.trim_end().split('\t').collect();
        assert_eq!(fields.len(), 5, "{text:?}");
        assert_eq!(fields[0], "0");
        assert_eq!(fields[1].parse::<f64>().unwrap(), 0.0);
        assert_eq!(fields[3], "0,0,400,300");
        assert_eq!(fields[4], "polyline");
    }

    #[test]
    fn a_file_that_is_not_a_pvec_exits_1() {
        let dir = scratch("not-pvec");
        let f = not_a_pvec(&dir);
        let out = run(&["pvec", "pick", s(&f), "--nearest", "0,0"]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(stderr(&out).contains("not a .pvec"), "{}", stderr(&out));
    }

    #[test]
    fn a_file_that_is_missing_exits_1() {
        let dir = scratch("missing");
        let gone = dir.join("gone.pvec");
        let out = run(&["pvec", "pick", s(&gone), "--nearest", "0,0"]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
    }
}

#[cfg(not(feature = "pvec-raster"))]
#[test]
fn a_build_without_pvec_raster_names_the_feature() {
    let dir = scratch("no-feature");
    let f = placeholder(&dir);
    let out = run(&["pvec", "pick", s(&f), "--nearest", "0,0"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-raster"), "{}", stderr(&out));
}
