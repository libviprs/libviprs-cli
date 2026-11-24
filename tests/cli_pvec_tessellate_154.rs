//! libviprs-cli#154 (CV2.5), `viprs pvec tessellate FILE`: every curve of a `.pvec` as a polyline
//! inside an error budget, through the facade's `Tessellate`. The cells hold the command to the
//! facade's own answer for the same curve and the same budget, and to the budget itself: a polyline
//! is only a tessellation if it stays inside what it was asked for.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

#[path = "pvec_fixture_154/mod.rs"]
mod fx;

use fx::*;

#[test]
fn tessellate_needs_one_budget_in_one_unit() {
    let dir = scratch("budget");
    let f = placeholder(&dir);
    usage(&["pvec", "tessellate", s(&f)], &["--error"]);
    usage(
        &[
            "pvec",
            "tessellate",
            s(&f),
            "--error",
            "1",
            "--error-px",
            "1",
            "--units-per-pixel",
            "2",
        ],
        &["--error-px"],
    );
}

#[test]
fn tessellate_error_px_needs_its_scale_and_the_scale_needs_it() {
    let dir = scratch("scale");
    let f = placeholder(&dir);
    usage(
        &["pvec", "tessellate", s(&f), "--error-px", "0.5"],
        &["--units-per-pixel"],
    );
    usage(
        &[
            "pvec",
            "tessellate",
            s(&f),
            "--error",
            "1",
            "--units-per-pixel",
            "2",
        ],
        &["--units-per-pixel"],
    );
}

#[test]
fn tessellate_refuses_a_budget_that_is_not_a_positive_number() {
    let dir = scratch("positive");
    let f = placeholder(&dir);
    for bad in ["0", "-1", "nan", "inf", "tight", ""] {
        usage(&["pvec", "tessellate", s(&f), "--error", bad], &["--error"]);
        usage(
            &[
                "pvec",
                "tessellate",
                s(&f),
                "--error-px",
                bad,
                "--units-per-pixel",
                "1",
            ],
            &["--error-px"],
        );
        usage(
            &[
                "pvec",
                "tessellate",
                s(&f),
                "--error-px",
                "1",
                "--units-per-pixel",
                bad,
            ],
            &["--units-per-pixel"],
        );
    }
    for bad in ["0", "-3", "many", ""] {
        usage(
            &[
                "pvec",
                "tessellate",
                s(&f),
                "--error",
                "1",
                "--max-segments",
                bad,
            ],
            &["--max-segments"],
        );
    }
}

#[cfg(feature = "pvec-raster")]
mod with_facade {
    use super::*;

    use pvec_facade::{Tessellate, TessellationContext, Transform2};

    fn context(error_px: f64, units_per_pixel: f64, max_segments: usize) -> TessellationContext {
        TessellationContext {
            source_to_map: Transform2::IDENTITY,
            map_to_screen: Transform2::scale(1.0 / units_per_pixel, 1.0 / units_per_pixel).unwrap(),
            pixel_ratio: 1.0,
            max_screen_error_px: error_px,
            max_world_error: f64::INFINITY,
            max_segments,
        }
    }

    fn entity(doc: &serde_json::Value, n: u64) -> &serde_json::Value {
        doc["entities"]
            .as_array()
            .unwrap_or_else(|| panic!("no entity list: {doc}"))
            .iter()
            .find(|e| e["entity"] == n)
            .unwrap_or_else(|| panic!("entity {n} is not in {doc}"))
    }

    #[test]
    fn error_in_source_units_matches_the_facade_for_every_curve() {
        let dir = scratch("source-units");
        let f = drawing(&dir);
        let doc = json(&ok(&[
            "pvec",
            "tessellate",
            s(&f),
            "--error",
            "0.5",
            "--json",
        ]));
        assert_eq!(doc["v"], 1);
        for (id, curve) in curves() {
            let want = curve.tessellate(&context(0.5, 1.0, 65_536)).unwrap();
            let got = entity(&doc, u64::from(id.index()));
            assert_eq!(
                got["vertices"].as_u64().unwrap() as usize,
                want.vertices.len(),
                "entity {}",
                id.index()
            );
            assert!(
                (got["achieved_world_error"].as_f64().unwrap() - want.achieved_world_error).abs()
                    < 1e-9
            );
            assert!(
                got["achieved_screen_error_px"].as_f64().unwrap() <= 0.5,
                "entity {} is over its budget: {got}",
                id.index()
            );
        }
    }

    #[test]
    fn a_polyline_is_its_own_tessellation_and_a_circle_needs_many_vertices() {
        let dir = scratch("kinds");
        let f = drawing(&dir);
        let doc = json(&ok(&[
            "pvec",
            "tessellate",
            s(&f),
            "--error",
            "0.5",
            "--json",
        ]));
        assert_eq!(entity(&doc, 0)["vertices"], 3);
        assert_eq!(entity(&doc, 2)["vertices"], 2);
        assert!(entity(&doc, 1)["vertices"].as_u64().unwrap() > 16, "{doc}");
        assert!(entity(&doc, 3)["vertices"].as_u64().unwrap() > 3, "{doc}");
    }

    #[test]
    fn a_tighter_budget_never_gives_fewer_vertices() {
        let dir = scratch("tighter");
        let f = drawing(&dir);
        let count = |error: &str| {
            let doc = json(&ok(&[
                "pvec",
                "tessellate",
                s(&f),
                "--error",
                error,
                "--json",
            ]));
            [1u64, 3].map(|n| entity(&doc, n)["vertices"].as_u64().unwrap())
        };
        let (loose, tight) = (count("5"), count("0.05"));
        for (l, t) in loose.into_iter().zip(tight) {
            assert!(t > l, "{l} vertices at 5 units, {t} at 0.05");
        }
    }

    #[test]
    fn error_px_with_units_per_pixel_is_the_same_budget_in_screen_terms() {
        let dir = scratch("px");
        let f = drawing(&dir);
        let doc = json(&ok(&[
            "pvec",
            "tessellate",
            s(&f),
            "--error-px",
            "0.5",
            "--units-per-pixel",
            "2",
            "--json",
        ]));
        for (id, curve) in curves() {
            let want = curve.tessellate(&context(0.5, 2.0, 65_536)).unwrap();
            let got = entity(&doc, u64::from(id.index()));
            assert_eq!(
                got["vertices"].as_u64().unwrap() as usize,
                want.vertices.len()
            );
            assert!(
                got["achieved_screen_error_px"].as_f64().unwrap() <= 0.5,
                "{got}"
            );
        }
        // 0.5 px at 2 units per pixel is a world budget of 1 unit: the same polyline as --error 1.
        let same = json(&ok(&[
            "pvec",
            "tessellate",
            s(&f),
            "--error",
            "1",
            "--json",
        ]));
        for n in 0..4 {
            assert_eq!(
                entity(&doc, n)["vertices"],
                entity(&same, n)["vertices"],
                "entity {n}"
            );
        }
    }

    #[test]
    fn text_is_one_tab_separated_line_per_entity_and_a_summary_on_stderr() {
        let dir = scratch("text");
        let f = drawing(&dir);
        let out = ok(&["pvec", "tessellate", s(&f), "--error", "0.5"]);
        let text = stdout(&out);
        let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
        assert_eq!(rows.len(), 4, "{text}");
        for (n, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), 4, "{row:?}");
            assert_eq!(row[0], n.to_string());
            assert!(row[1].parse::<usize>().unwrap() >= 2);
            assert!(row[2].parse::<f64>().unwrap() <= 0.5);
            assert!(row[3].parse::<f64>().unwrap() <= 0.5 + 1e-9);
        }
    }

    #[test]
    fn a_budget_the_segment_cap_cannot_reach_is_exit_1_and_names_the_entity() {
        let dir = scratch("unmet");
        let f = drawing(&dir);
        let out = run(&[
            "pvec",
            "tessellate",
            s(&f),
            "--error",
            "0.0001",
            "--max-segments",
            "4",
            "--json",
        ]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains("entity 1") && err.contains("budget"), "{err}");
        // The entities that did meet it are still reported, and the failure is in the document.
        let doc = json(&out);
        assert_eq!(doc["ok"], false);
        assert_eq!(entity(&doc, 0)["vertices"], 3);
        assert_eq!(entity(&doc, 1)["status"], "budget-unmet");
    }

    #[test]
    fn a_file_that_is_not_a_pvec_exits_1() {
        let dir = scratch("not-pvec");
        let f = not_a_pvec(&dir);
        let out = run(&["pvec", "tessellate", s(&f), "--error", "1"]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(stderr(&out).contains("not a .pvec"), "{}", stderr(&out));
    }
}

#[cfg(not(feature = "pvec-raster"))]
#[test]
fn a_build_without_pvec_raster_names_the_feature() {
    let dir = scratch("no-feature");
    let f = placeholder(&dir);
    let out = run(&["pvec", "tessellate", s(&f), "--error", "1"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-raster"), "{}", stderr(&out));
}
