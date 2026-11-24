//! libviprs-cli#150 (CV2.1), `viprs pvec layouts DRAWING`: the paper layouts of a DWG over the
//! core's `paper_sheets`, listed without converting anything. Each cell looks at what the command
//! printed and at what it left on disk, and the listing is held to what `pvec convert` then writes
//! for the same drawing, because a listing that disagrees with the set is the bug this API exists to
//! prevent.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vector")
        .join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("viprs must spawn")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the process must exit normally")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-layouts-150-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(feature = "pvec-convert")]
#[test]
fn every_paper_layout_is_listed_in_tab_order_and_nothing_is_written() {
    let dir = scratch("order");
    let out = Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(["pvec", "layouts", s(&fixture("sample_AC1018.dwg"))])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let rows: Vec<Vec<String>> = stdout(&out)
        .lines()
        .map(|l| l.split_whitespace().map(str::to_owned).collect())
        .collect();
    assert_eq!(
        rows,
        [
            ["1", "Layout1", "sheet"],
            ["2", "Layout2", "sheet"],
            ["3", "MyLayout", "sheet"]
        ],
        "{}",
        stdout(&out)
    );
    assert!(listing(&dir).is_empty(), "{:?}", listing(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn json_has_one_object_per_layout() {
    let out = run(&[
        "pvec",
        "layouts",
        s(&fixture("sample_AC1018.dwg")),
        "--json",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "v": 1,
            "layouts": [
                {"tab_order": 1, "layout": "Layout1", "sheet": true, "skip": null},
                {"tab_order": 2, "layout": "Layout2", "sheet": true, "skip": null},
                {"tab_order": 3, "layout": "MyLayout", "sheet": true, "skip": null},
            ]
        })
    );
}

/// A layout with nothing to print is listed and says why, and the command still exits 0: it is a
/// listing, not a conversion.
#[cfg(feature = "pvec-convert")]
#[test]
fn an_empty_layout_is_listed_with_its_reason() {
    let drawing = fixture("vc_layers.dwg");
    let out = run(&["pvec", "layouts", s(&drawing)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let rows: Vec<Vec<String>> = stdout(&out)
        .lines()
        .map(|l| l.split_whitespace().map(str::to_owned).collect())
        .collect();
    assert_eq!(rows, [["1", "Layout1", "empty"]], "{}", stdout(&out));
    let out = run(&["pvec", "layouts", s(&drawing), "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["layouts"][0]["sheet"], false, "{json}");
    assert_eq!(json["layouts"][0]["skip"], "empty", "{json}");
}

/// The listing and the set agree: the same layouts, the same numbers, and a layout listed as
/// not a sheet is the one `convert` reports as empty.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_listing_agrees_with_what_convert_writes() {
    for name in ["sample_AC1018.dwg", "vc_layers.dwg"] {
        let drawing = fixture(name);
        let listed = run(&["pvec", "layouts", s(&drawing), "--json"]);
        let listed: serde_json::Value = serde_json::from_str(&stdout(&listed)).unwrap();
        let dir = scratch("agree");
        let stem = name.trim_end_matches(".dwg");
        run(&["pvec", "convert", s(&drawing), s(&dir)]);
        let index: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join(format!("{stem}.sheets.json"))).unwrap(),
        )
        .unwrap();
        let (listed, written) = (
            listed["layouts"].as_array().unwrap(),
            index["sheets"].as_array().unwrap(),
        );
        assert!(!listed.is_empty(), "{name}: nothing listed");
        assert_eq!(listed.len(), written.len(), "{name}");
        for (l, w) in listed.iter().zip(written) {
            assert_eq!(l["layout"], w["layout"], "{name}");
            assert_eq!(l["tab_order"], w["tab_order"], "{name}");
            if l["sheet"] == false {
                assert_eq!(w["status"], "empty", "{name}: {l} against {w}");
            } else {
                assert_eq!(w["status"], "written", "{name}: {l} against {w}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(feature = "pvec-convert")]
#[test]
fn a_drawing_that_is_not_there_or_not_a_dwg_exits_1_and_says_which() {
    let dir = scratch("missing");
    let out = run(&["pvec", "layouts", s(&dir.join("nope.dwg"))]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("nope.dwg"), "{}", stderr(&out));
    let text = dir.join("plan.dwg");
    std::fs::write(&text, b"this is not a drawing").unwrap();
    let out = run(&["pvec", "layouts", s(&text)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("unrecognized subcommand"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_drawing_argument_is_a_usage_error() {
    let out = run(&["pvec", "layouts"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("unrecognized subcommand"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn the_help_says_what_the_listing_is() {
    let out = run(&["pvec", "layouts", "--help"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let help = stdout(&out);
    assert!(help.contains("--json"), "{help}");
    assert!(help.contains("tab_order"), "{help}");
}

/// A build without `pvec-convert` exits 1 and names the feature.
#[cfg(not(feature = "pvec-convert"))]
#[test]
fn a_build_without_the_feature_exits_1_and_names_it() {
    let out = run(&["pvec", "layouts", s(&fixture("sample_AC1018.dwg"))]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-convert"), "{}", stderr(&out));
}
