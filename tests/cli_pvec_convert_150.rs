//! libviprs-cli#150 (CV2.1), `viprs pvec convert`: `--annotations`, `--element-rule` and
//! `--thumbnail`, the real binary. Exit codes are not evidence, so every cell reads what the
//! conversion wrote: the report beside the artifact, the streams PVec's reader finds in it, the PNG.
//! They assert what the pinned core does today, not what it ought to.

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
    let dir = std::env::temp_dir().join(format!("viprs-convert-150-{}-{tag}", std::process::id()));
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

/// The report a model-space conversion wrote beside `<stem>.pvec`.
#[cfg(feature = "pvec-convert")]
fn model(drawing: &str, dir: &Path, extra: &[&str]) -> (Output, serde_json::Value) {
    let drawing = fixture(drawing);
    let mut args = vec![
        "pvec",
        "convert",
        s(&drawing),
        s(dir),
        "--model",
        "--unit",
        "m",
    ];
    args.extend_from_slice(extra);
    let out = run(&args);
    let stem = drawing.file_stem().unwrap().to_string_lossy().into_owned();
    let report = std::fs::read(dir.join(format!("{stem}.pvec.report.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null);
    (out, report)
}

/// Without `--annotations` a drawing that is only text has nothing to write, which is the core's
/// answer today. With it, the text stream carries both records.
#[cfg(feature = "pvec-convert")]
#[test]
fn annotations_carry_text_into_the_text_stream() {
    let dir = scratch("text-off");
    let (out, _) = model("g13_text.dwg", &dir, &[]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no entity carries geometry"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);

    let dir = scratch("text-on");
    let (out, report) = model("g13_text.dwg", &dir, &["--annotations"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["components"]["texts"], true, "{report}");
    assert_eq!(report["components"]["fills"], true, "{report}");
    assert!(
        report["components"]["annotations"]
            .as_object()
            .is_some_and(|m| !m.is_empty()),
        "{report}"
    );
    #[cfg(feature = "pvec-raster")]
    {
        let text = run(&["pvec", "text", s(&dir.join("g13_text.pvec"))]);
        assert_eq!(code(&text), 0, "{}", stderr(&text));
        for needle in ["VIPRS-G13-TEXT", "VIPRS-G13-MTEXT"] {
            assert!(
                stdout(&text).contains(needle),
                "{needle} in:\n{}",
                stdout(&text)
            );
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// HATCH and SOLID go into the hatch stream, which the artifact then declares.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn annotations_carry_hatches_into_the_hatch_stream() {
    let dir = scratch("hatch-on");
    let (out, report) = model("g13_hatch.dwg", &dir, &["--annotations"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["components"]["fills"], true, "{report}");
    let info = run(&["pvec", "info", s(&dir.join("g13_hatch.pvec"))]);
    assert!(stdout(&info).contains("hatch (17)"), "{}", stdout(&info));
    let _ = std::fs::remove_dir_all(dir);
}

/// Annotations need the line style stream, so `--geometry-only` and them are a usage error,
/// reported before anything is written.
#[cfg(feature = "pvec-convert")]
#[test]
fn annotations_and_geometry_only_conflict() {
    let dir = scratch("conflict");
    let out_dir = dir.join("out");
    let drawing = fixture("sample_AC1018.dwg");
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&out_dir),
        "--annotations",
        "--geometry-only",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--geometry-only"), "{}", stderr(&out));
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// `wall_source` in the report says whether the element rules named the walls (`layers`) or no
/// rule matched and every straight line stood in (`geometry`).
#[cfg(feature = "pvec-convert")]
#[test]
fn element_rules_decide_where_the_walls_come_from() {
    let wall_source = |rule: &[&str]| {
        let dir = scratch("rules");
        let mut extra = vec!["--regions"];
        extra.extend_from_slice(rule);
        let (out, report) = model("vc_layers.dwg", &dir, &extra);
        assert_eq!(code(&out), 0, "{rule:?}: {}", stderr(&out));
        let _ = std::fs::remove_dir_all(dir);
        report["regions"]["wall_source"].clone()
    };
    // The default profile names A-WALL* and S-WALL*, which this drawing has not got.
    assert_eq!(wall_source(&[]), "geometry");
    // A layer rule, matched with the case ignored.
    assert_eq!(
        wall_source(&["--element-rule", "layer:VC_ON=wall"]),
        "layers"
    );
    assert_eq!(
        wall_source(&["--element-rule", "layer:vc_*=wall"]),
        "layers"
    );
    // A rule that matches nothing leaves the lines to stand in.
    assert_eq!(
        wall_source(&["--element-rule", "layer:NOPE*=wall"]),
        "geometry"
    );
    // The rules are a list: a later one still counts, and every kind is accepted.
    assert_eq!(
        wall_source(&[
            "--element-rule",
            "layer:NOPE*=column",
            "--element-rule",
            "block:NOPE*=column",
            "--element-rule",
            "colour:7=wall",
            "--element-rule",
            "lineweight:25=wall",
            "--element-rule",
            "layer:VC_ON=wall",
        ]),
        "layers"
    );
}

#[cfg(feature = "pvec-convert")]
#[test]
fn a_malformed_element_rule_is_a_usage_error_that_writes_nothing() {
    let dir = scratch("rule-usage");
    let out_dir = dir.join("out");
    let drawing = fixture("vc_layers.dwg");
    let bad = [
        "wall",
        "layer:A-WALL*",
        "layer:=wall",
        "layer:A-WALL*=roof",
        "floor:A-WALL*=wall",
        "colour:0=wall",
        "colour:300=wall",
        "colour:red=wall",
        "lineweight:thick=wall",
    ];
    for rule in bad {
        let out = run(&[
            "pvec",
            "convert",
            s(&drawing),
            s(&out_dir),
            "--model",
            "--unit",
            "m",
            "--regions",
            "--element-rule",
            rule,
        ]);
        assert_eq!(code(&out), 2, "{rule}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("--element-rule"),
            "{rule}: {}",
            stderr(&out)
        );
        assert!(!out_dir.exists(), "{rule}");
    }
    // The rules only mean something to the regions step.
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
        "--element-rule",
        "layer:VC_ON=wall",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--regions"), "{}", stderr(&out));
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// A width of zero or past the cap is a usage error in every build, before the drawing is opened.
#[test]
fn a_thumbnail_width_that_cannot_be_drawn_is_a_usage_error() {
    let dir = scratch("thumb-usage");
    let out_dir = dir.join("out");
    for width in ["0", "4097", "wide"] {
        let out = run(&[
            "pvec",
            "convert",
            s(&dir.join("not-there.dwg")),
            s(&out_dir),
            "--thumbnail",
            width,
        ]);
        assert_eq!(code(&out), 2, "{width}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("invalid value"),
            "{width}: {}",
            stderr(&out)
        );
        assert!(!out_dir.exists());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_help_says_a_thumbnail_is_strokes_only() {
    let out = run(&["pvec", "convert", "--help"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let help = stdout(&out);
    for needle in [
        "--thumbnail",
        "strokes only",
        "no text, no fills",
        "--annotations",
        "--element-rule",
    ] {
        assert!(help.contains(needle), "{needle} not in:\n{help}");
    }
}

/// A build without `pvec-raster-thumbnail` exits 1 and names it, before writing anything.
#[cfg(all(feature = "pvec-convert", not(feature = "pvec-raster-thumbnail")))]
#[test]
fn a_build_without_the_thumbnail_feature_exits_1_and_names_it() {
    let dir = scratch("thumb-nofeature");
    let out_dir = dir.join("out");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&out_dir),
        "--thumbnail",
        "200",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("pvec-raster-thumbnail"),
        "{}",
        stderr(&out)
    );
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// The PNG beside a sheet is as wide as asked, as tall as the sheet's paper says, and has strokes.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster-thumbnail"))]
#[test]
fn every_sheet_gets_a_png_the_width_asked_and_the_height_of_its_paper() {
    let dir = scratch("thumb-sheets");
    let drawing = fixture("sample_AC1018.dwg");
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&dir),
        "--thumbnail",
        "400",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("sample_AC1018.sheets.json")).unwrap())
            .unwrap();
    for row in index["sheets"].as_array().unwrap() {
        let file = row["file"].as_str().unwrap();
        let png = dir.join(file.replace(".pvec", ".png"));
        let raster =
            libviprs::decode_file(&png).unwrap_or_else(|e| panic!("{}: {e}", png.display()));
        assert_eq!(raster.width(), 400, "{file}");
        let b: Vec<f64> = row["sheet"]["box"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse().unwrap())
            .collect();
        let want = (400.0 * (b[3] - b[1]) / (b[2] - b[0])).round() as u32;
        assert_eq!(raster.height(), want, "{file}");
        assert!(
            raster.data().iter().any(|&v| v != 255),
            "{file} is all white"
        );
    }
    // Nothing but the sheets, their reports, their pictures and the index.
    assert_eq!(listing(&dir).len(), 3 * 3 + 1, "{:?}", listing(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

/// No flag, no picture.
#[cfg(feature = "pvec-convert")]
#[test]
fn without_the_flag_no_png_is_written() {
    let dir = scratch("thumb-none");
    let (out, _) = model("vc_layers.dwg", &dir, &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        listing(&dir).iter().all(|n| !n.ends_with(".png")),
        "{:?}",
        listing(&dir)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// `--thumbnail` with `--model` is a usage error in every build: one stray far entity gives model
/// space an extreme aspect, and the core draws the picture before anything can cap its height.
/// The message points at `viprs pvec thumbnail`, which caps both sides.
#[test]
fn a_model_space_thumbnail_is_a_usage_error_that_points_at_pvec_thumbnail() {
    let dir = scratch("thumb-model-usage");
    let out_dir = dir.join("out");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("vc_layers.dwg")),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
        "--thumbnail",
        "300",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("viprs pvec thumbnail FILE OUT.png"),
        "{}",
        stderr(&out)
    );
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}
