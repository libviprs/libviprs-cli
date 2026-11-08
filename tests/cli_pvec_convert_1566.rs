//! libviprs#1566 (slice C of libviprs#1557), the real binary: `viprs pvec convert <drawing> <out-dir>`
//! writes one `.pvec` per paper layout, one layout with `--layout`, or model space with `--model`.
//! Exit codes are not evidence on their own, so every cell also looks at what is on disk: the
//! sheet files, their reports and the `<stem>.sheets.json` index the library writes last.

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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "viprs-pvec-convert-1566-{}-{tag}",
        std::process::id()
    ));
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
fn index(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[cfg(feature = "pvec-convert")]
#[test]
fn every_paper_layout_is_listed_in_tab_order_and_the_exit_code_follows_the_index() {
    let dir = scratch("all");
    let out_dir = dir.join("sheets");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&out_dir),
    ]);
    let err = stderr(&out);

    // The directory is created, and the index is the record of what happened to each layout.
    let json = index(&out_dir.join("sample_AC1018.sheets.json"));
    let rows = json["sheets"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["layout"].as_str().unwrap()).collect();
    assert_eq!(names, ["Layout1", "Layout2", "MyLayout"], "{err}");
    let files: Vec<&str> = rows.iter().map(|r| r["file"].as_str().unwrap()).collect();
    assert_eq!(
        files,
        [
            "sample_AC1018.01-Layout1.pvec",
            "sample_AC1018.02-Layout2.pvec",
            "sample_AC1018.03-MyLayout.pvec"
        ]
    );
    // A written sheet is on disk with its report; a skipped or failed one is not.
    for r in rows {
        let file = out_dir.join(r["file"].as_str().unwrap());
        let written = r["status"] == "written";
        assert_eq!(file.is_file(), written, "{r}");
        assert_eq!(
            out_dir
                .join(format!("{}.report.json", r["file"].as_str().unwrap()))
                .is_file(),
            written,
            "{r}"
        );
        // Each sheet has a line on stderr naming it.
        assert!(err.contains(r["layout"].as_str().unwrap()), "{err}");
    }
    // 0 only when at least one sheet was written and none failed.
    let any_written = rows.iter().any(|r| r["status"] == "written");
    let any_failed = rows.iter().any(|r| r["status"] == "failed");
    // Nothing in this drawing is a reason to fail a sheet.
    assert!(!any_failed, "{json}\n{err}");
    assert_eq!(
        code(&out) == 0,
        any_written && !any_failed,
        "exit {}:\n{err}",
        code(&out)
    );
    assert_ne!(code(&out), 2, "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn a_drawing_with_no_sheet_to_print_exits_1_and_writes_only_the_index() {
    let dir = scratch("empty");
    let out = run(&["pvec", "convert", s(&fixture("vc_layers.dwg")), s(&dir)]);
    let err = stderr(&out);
    assert_eq!(code(&out), 1, "{err}");
    assert!(err.contains("no sheet was written"), "{err}");
    assert_eq!(listing(&dir), ["vc_layers.sheets.json"], "{err}");
    let json = index(&dir.join("vc_layers.sheets.json"));
    assert_eq!(json["sheets"][0]["layout"], "Layout1");
    assert_eq!(json["sheets"][0]["status"], "empty");
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn one_layout_keeps_its_number_in_the_set() {
    let dir = scratch("one");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&dir),
        "--layout",
        "layout2",
        "--stem",
        "plan",
    ]);
    let err = stderr(&out);
    let json = index(&dir.join("plan.sheets.json"));
    let rows = json["sheets"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{json}");
    assert_eq!(rows[0]["layout"], "Layout2");
    assert_eq!(rows[0]["nn"], "02");
    assert_eq!(rows[0]["file"], "plan.02-Layout2.pvec");
    assert_eq!(
        code(&out) == 0,
        rows[0]["status"] == "written",
        "exit {}:\n{err}",
        code(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn an_unknown_layout_exits_1_names_the_ones_there_are_and_writes_nothing() {
    let dir = scratch("unknown");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&dir),
        "--layout",
        "Nope",
    ]);
    let err = stderr(&out);
    assert_eq!(code(&out), 1, "{err}");
    for needle in ["Nope", "Layout1", "MyLayout"] {
        assert!(err.contains(needle), "{needle:?} not in:\n{err}");
    }
    assert!(listing(&dir).is_empty(), "{:?}", listing(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn a_drawing_that_is_not_there_exits_1_and_writes_nothing() {
    let dir = scratch("missing");
    let out_dir = dir.join("out");
    let out = run(&["pvec", "convert", s(&dir.join("nope.dwg")), s(&out_dir)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("nope.dwg"), "{}", stderr(&out));
    assert!(!out_dir.exists());
    // A layout the drawing does not have leaves no directory behind either.
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&out_dir),
        "--layout",
        "Nope",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn model_space_is_one_pvec_named_after_the_stem() {
    let dir = scratch("model");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("vc_layers.dwg")),
        s(&dir),
        "--model",
        "--unit",
        "m",
    ]);
    let err = stderr(&out);
    assert_eq!(code(&out), 0, "{err}");
    assert_eq!(
        listing(&dir),
        ["vc_layers.pvec", "vc_layers.pvec.report.json"],
        "{err}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn usage_mistakes_exit_2_and_write_nothing() {
    let dir = scratch("usage");
    let file = dir.join("taken");
    std::fs::write(&file, b"x").unwrap();
    let drawing = fixture("sample_AC1018.dwg");
    let cases: [&[&str]; 4] = [
        // The output is a file, not a directory.
        &["pvec", "convert", s(&drawing), s(&file)],
        // Model space and a layout at once.
        &[
            "pvec",
            "convert",
            s(&drawing),
            s(&dir),
            "--model",
            "--layout",
            "Layout1",
        ],
        // A stem that is a path.
        &["pvec", "convert", s(&drawing), s(&dir), "--stem", "a/b"],
        // A unit the command does not take.
        &["pvec", "convert", s(&drawing), s(&dir), "--unit", "parsec"],
    ];
    for args in cases {
        let out = run(args);
        let err = stderr(&out);
        assert_eq!(code(&out), 2, "{args:?}:\n{err}");
        assert!(!err.contains("unrecognized subcommand"), "{args:?}:\n{err}");
    }
    assert_eq!(listing(&dir), ["taken"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A build without `pvec-convert` exits 1 and names the feature, and still reports a usage mistake.
#[cfg(not(feature = "pvec-convert"))]
#[test]
fn a_build_without_the_feature_exits_1_and_names_it() {
    let dir = scratch("nofeature");
    let out = run(&["pvec", "convert", s(&fixture("sample_AC1018.dwg")), s(&dir)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-convert"), "{}", stderr(&out));
    assert!(listing(&dir).is_empty());
    let file = dir.join("taken");
    std::fs::write(&file, b"x").unwrap();
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&file),
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}
