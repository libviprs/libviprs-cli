//! End-to-end cells for `viprs vector` (libviprs#1035).
//!
//! Three kinds of cell, because three things differ between builds:
//!
//! * the ones that read a PMTiles archive (`info`, `validate`) and the usage
//!   refusals, which run in every build off the committed `ladder.pmtiles`;
//! * the ones for a build **without** `dwg-acadsharp`, which prove the commands
//!   that need a DWG reader say so and write nothing;
//! * the ones for a build **with** it, which need the native archive
//!   `acadsharp-rs` links. They skip, saying so, when the reader is not linked,
//!   unless `LIBVIPRS_REQUIRE_ACADSHARP_NATIVE=1` turns the skip into a failure.
//!
//! The cancel and timeout behaviour that does not need a DWG is pinned in the
//! unit tests in `src/vector.rs`, against a drawing built from a loop.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn viprs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
}

fn run(args: &[&str]) -> Output {
    viprs()
        .args(args)
        .output()
        .expect("the viprs binary must be spawnable")
}

fn code(out: &Output) -> i32 {
    out.status
        .code()
        .expect("the process must exit normally rather than via a signal")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vector")
        .join(name)
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// A scratch directory of this cell's own.
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-vector-cli-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON ({e}): {}", stdout(out)))
}

// ---------------------------------------------------------------------------
// info and validate: every build
// ---------------------------------------------------------------------------

#[test]
fn info_describes_an_archive() {
    let out = run(&["vector", "info", s(&fixture("ladder.pmtiles"))]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Name:        Ladder"), "{text}");
    assert!(text.contains("Zoom:        0 to 3"), "{text}");
    assert!(text.contains("Layer:       lines"), "{text}");
    assert!(text.contains("Layer:       text"), "{text}");
}

#[test]
fn info_json_is_versioned_and_complete() {
    let out = run(&["vector", "info", s(&fixture("ladder.pmtiles")), "--json"]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let j = json(&out);
    assert_eq!(j["v"], 1);
    assert_eq!(j["name"], "Ladder");
    assert_eq!(j["tile_type"], "mvt");
    assert_eq!(j["max_zoom"], 3);
    assert!(
        j["layers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["id"] == "lines")
    );
    assert_eq!(j["tiler"]["max_zoom"], 3);
}

#[test]
fn validate_accepts_a_sound_archive() {
    let out = run(&["vector", "validate", s(&fixture("ladder.pmtiles"))]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(stdout(&out).starts_with("OK:"), "{}", stdout(&out));
    let out = run(&[
        "vector",
        "validate",
        s(&fixture("ladder.pmtiles")),
        "--json",
    ]);
    assert_eq!(code(&out), 0);
    let j = json(&out);
    assert_eq!(j["v"], 1);
    assert_eq!(j["valid"], true);
    assert_eq!(j["findings"].as_array().unwrap().len(), 0);
}

#[test]
fn validate_names_what_is_wrong_with_a_damaged_archive_and_exits_1() {
    let dir = unique_dir("damaged");
    let bytes = std::fs::read(fixture("ladder.pmtiles")).unwrap();
    let cut = dir.join("cut.pmtiles");
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let out = run(&["vector", "validate", s(&cut)]);
    assert_eq!(code(&out), 1, "stdout:\n{}", stdout(&out));
    assert!(stderr(&out).contains("Error:"), "{}", stderr(&out));
    assert!(!stdout(&out).starts_with("OK:"));

    let out = run(&["vector", "validate", s(&cut), "--json"]);
    assert_eq!(code(&out), 1);
    let j = json(&out);
    assert_eq!(j["valid"], false);
    assert!(!j["findings"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_that_is_not_an_archive_is_an_operational_failure_for_both() {
    let dir = unique_dir("junk");
    let junk = dir.join("junk.pmtiles");
    std::fs::write(&junk, vec![9u8; 2048]).unwrap();
    assert_eq!(code(&run(&["vector", "validate", s(&junk)])), 1);
    let out = run(&["vector", "info", s(&junk)]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("Error:"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_archive_is_an_operational_failure() {
    for cmd in ["info", "validate"] {
        let out = run(&["vector", cmd, "/nonexistent/viprs/none.pmtiles"]);
        assert_eq!(code(&out), 1, "{cmd}: {}", stderr(&out));
        assert!(stderr(&out).contains("Error:"));
    }
}

// ---------------------------------------------------------------------------
// Usage mistakes: nothing is written, and the exit is 2
// ---------------------------------------------------------------------------

#[test]
fn a_command_line_mistake_is_a_usage_error() {
    let dir = unique_dir("usage");
    let input = dir.join("in.dwg");
    std::fs::write(&input, b"AC1018 not read").unwrap();
    let out_path = dir.join("out.pmtiles");
    let cases: Vec<Vec<&str>> = vec![
        vec!["vector"],
        vec!["vector", "nonsense"],
        vec!["vector", "build"],
        vec!["vector", "info"],
        vec!["vector", "build", s(&input), s(&out_path), "--timeout", "0"],
        vec![
            "vector",
            "build",
            s(&input),
            s(&out_path),
            "--timeout",
            "soon",
        ],
        vec![
            "vector",
            "build",
            s(&input),
            s(&out_path),
            "--timeout",
            "5x",
        ],
        vec![
            "vector",
            "build",
            s(&input),
            s(&out_path),
            "--max-zoom",
            "25",
        ],
        vec!["vector", "report", s(&input), "--max-zoom", "-1"],
        vec!["vector", "build", s(&input), s(&input)],
    ];
    for args in cases {
        let out = run(&args);
        assert_eq!(code(&out), 2, "{args:?}\nstderr:\n{}", stderr(&out));
    }
    assert!(!out_path.exists(), "a usage error writes nothing");
    assert_eq!(entries(&dir), vec!["in.dwg"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_drawing_that_does_not_exist_is_an_operational_failure() {
    for cmd in ["views", "build", "report"] {
        let out = run(&["vector", cmd, "/nonexistent/viprs/none.dwg"]);
        assert_eq!(code(&out), 1, "{cmd}: {}", stderr(&out));
    }
}

#[test]
fn the_help_lists_the_five_commands_and_the_convert_alias_parses() {
    let out = run(&["vector", "--help"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for cmd in ["views", "build", "report", "info", "validate"] {
        assert!(text.contains(cmd), "{cmd} missing from:\n{text}");
    }
    let out = run(&["vector", "convert", "--help"]);
    assert_eq!(code(&out), 0, "`convert` is the issue's name for `build`");
    assert!(stdout(&out).contains("--timeout"));
    assert!(stdout(&out).contains("--max-zoom"));
    assert!(stdout(&out).contains("--hardened"));
}

// ---------------------------------------------------------------------------
// A build with no DWG reader
// ---------------------------------------------------------------------------

#[cfg(not(feature = "dwg-acadsharp"))]
#[test]
fn without_the_feature_the_drawing_commands_say_how_to_rebuild_and_write_nothing() {
    let dir = unique_dir("nofeature");
    let input = dir.join("in.dwg");
    std::fs::write(&input, b"AC1018 not read").unwrap();
    let output = dir.join("out.pmtiles");
    let cases: Vec<Vec<&str>> = vec![
        vec!["vector", "views", s(&input)],
        vec!["vector", "build", s(&input), s(&output)],
        vec!["vector", "report", s(&input)],
    ];
    for args in cases {
        let out = run(&args);
        assert_eq!(code(&out), 1, "{args:?}\nstderr:\n{}", stderr(&out));
        assert!(
            stderr(&out).contains("--features dwg-acadsharp"),
            "{args:?}\n{}",
            stderr(&out)
        );
    }
    assert_eq!(entries(&dir), vec!["in.dwg"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(not(feature = "dwg-acadsharp"))]
#[test]
fn features_does_not_list_a_reader_this_build_left_out() {
    let out = run(&["features"]);
    assert_eq!(code(&out), 0);
    assert!(!stdout(&out).lines().any(|l| l == "dwg-acadsharp"));
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn features_lists_the_reader() {
    let out = run(&["features"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).lines().any(|l| l == "dwg-acadsharp"));
}

// ---------------------------------------------------------------------------
// A build with the reader, and a native library to drive it
// ---------------------------------------------------------------------------

/// Whether the native reader is linked into this binary. Skips (or fails, when
/// the lane is required) when it is not.
#[cfg(feature = "dwg-acadsharp")]
fn native_lane() -> bool {
    let out = run(&["vector", "views", s(&fixture("sample_AC1018.dwg"))]);
    if code(&out) == 0 {
        return true;
    }
    assert!(
        std::env::var("LIBVIPRS_REQUIRE_ACADSHARP_NATIVE").as_deref() != Ok("1"),
        "native lane required, but the reader is not linked:\n{}",
        stderr(&out)
    );
    eprintln!("skipped: the native reader is not linked: {}", stderr(&out));
    false
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn views_lists_model_space() {
    if !native_lane() {
        return;
    }
    let sample = fixture("sample_AC1018.dwg");
    let out = run(&["vector", "views", s(&sample)]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(stdout(&out).contains("model"), "{}", stdout(&out));
    let out = run(&["vector", "views", s(&sample), "--json"]);
    let j = json(&out);
    assert_eq!(j["v"], 1);
    assert_eq!(j["views"][0]["kind"], "model");
    assert_eq!(j["views"][0]["enabled"], true);
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn build_writes_one_valid_archive_and_nothing_else() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("build");
    let out_path = dir.join("sample.pmtiles");
    let out = run(&[
        "vector",
        "build",
        s(&fixture("sample_AC1018.dwg")),
        s(&out_path),
        "--max-zoom",
        "6",
        "--name",
        "Sample",
        "--hardened",
    ]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(stdout(&out).starts_with("Wrote "), "{}", stdout(&out));
    assert_eq!(
        entries(&dir),
        vec!["sample.pmtiles"],
        "no staging files left"
    );
    assert_eq!(code(&run(&["vector", "validate", s(&out_path)])), 0);
    let info = run(&["vector", "info", s(&out_path), "--json"]);
    let j = json(&info);
    assert_eq!(j["name"], "Sample");
    assert_eq!(j["max_zoom"], 6);

    // And as JSON, over the same input, to a name derived from the input.
    let copy = dir.join("again.dwg");
    std::fs::copy(fixture("sample_AC1018.dwg"), &copy).unwrap();
    let out = run(&["vector", "build", s(&copy), "--max-zoom", "4", "--json"]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let j = json(&out);
    assert_eq!(j["v"], 1);
    assert_eq!(j["decode"]["complete"], true);
    assert!(j["tiles"]["written"].as_u64().unwrap() > 0);
    assert!(
        dir.join("again.pmtiles").is_file(),
        "the output name follows the input's"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn report_writes_nothing_and_agrees_with_a_build() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("report");
    let sample = fixture("sample_AC1018.dwg");
    let rep = run(&["vector", "report", s(&sample), "--max-zoom", "5", "--json"]);
    assert_eq!(code(&rep), 0, "stderr:\n{}", stderr(&rep));
    assert!(entries(&dir).is_empty());
    let built = dir.join("b.pmtiles");
    let b = run(&[
        "vector",
        "build",
        s(&sample),
        s(&built),
        "--max-zoom",
        "5",
        "--json",
    ]);
    assert_eq!(code(&b), 0);
    let (rj, bj) = (json(&rep), json(&b));
    assert_eq!(
        rj["tiles"], bj["tiles"],
        "a report is a build that writes nothing"
    );
    assert_eq!(rj["decode"], bj["decode"]);
    assert!(rj["destination"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn a_timeout_exits_1_and_leaves_nothing_behind() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("timeout");
    let out_path = dir.join("never.pmtiles");
    let out = run(&[
        "vector",
        "build",
        s(&fixture("sample_AC1018.dwg")),
        s(&out_path),
        "--max-zoom",
        "10",
        "--timeout",
        "1ms",
    ]);
    assert_eq!(code(&out), 1, "stderr:\n{}", stderr(&out));
    assert!(stderr(&out).contains("--timeout"), "{}", stderr(&out));
    assert!(entries(&dir).is_empty(), "left: {:?}", entries(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn a_layout_or_an_unknown_view_is_a_usage_error() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("view");
    let out_path = dir.join("never.pmtiles");
    let out = run(&[
        "vector",
        "build",
        s(&fixture("sample_AC1018.dwg")),
        s(&out_path),
        "--view",
        "No such view",
    ]);
    assert_eq!(code(&out), 2, "stderr:\n{}", stderr(&out));
    assert!(
        stderr(&out).contains("no view is named"),
        "{}",
        stderr(&out)
    );
    assert!(entries(&dir).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(all(feature = "dwg-acadsharp", unix))]
#[test]
fn ctrl_c_mid_build_exits_130_and_leaves_nothing_behind() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("sigint");
    let out_path = dir.join("never.pmtiles");
    // A deep zoom over a real drawing runs for long enough that the staging
    // files exist (the writer opens them first) well before it finishes.
    let mut child = viprs()
        .args([
            "vector",
            "build",
            s(&fixture("sample_AC1018.dwg")),
            s(&out_path),
            "--max-zoom",
            "24",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let started = std::time::Instant::now();
    let staged = loop {
        if !entries(&dir).is_empty() {
            break true;
        }
        if child.try_wait().unwrap().is_some() || started.elapsed().as_secs() > 60 {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    if !staged {
        let out = child.wait_with_output().unwrap();
        panic!(
            "the build ended (exit {:?}) before it staged anything; the cell needs a slower input\n{}",
            out.status.code(),
            stderr(&out)
        );
    }
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill must be runnable");
    assert!(killed.success());
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 130, "stderr:\n{}", stderr(&out));
    assert!(entries(&dir).is_empty(), "left behind: {:?}", entries(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// --hardened (libviprs-cli#122)
// ---------------------------------------------------------------------------

/// A DWG version stamp followed by zeros, one byte past the hardened input cap
/// (256 MiB). A sparse file, so it costs no disk.
#[cfg(feature = "dwg-acadsharp")]
fn oversized_drawing(dir: &Path) -> PathBuf {
    use std::io::Write;
    let path = dir.join("huge.dwg");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"AC1032").unwrap();
    f.set_len((256 << 20) + 1).unwrap();
    path
}

/// The flag reaches `default_decoder`. The reviewer's mutation, passing `false`
/// there, left every other cell green because the sample drawing is inside every
/// hardened cap. A file past the input cap is refused by the cap only when the
/// flag is on, and the provider's own limit error is what says so.
#[cfg(feature = "dwg-acadsharp")]
#[test]
fn hardened_reaches_the_decoder_and_refuses_a_drawing_past_the_input_cap() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("hardened");
    let huge = oversized_drawing(&dir);
    for command in ["views", "report"] {
        let out = run(&["vector", command, s(&huge), "--hardened"]);
        assert_eq!(code(&out), 1, "{command}: stderr:\n{}", stderr(&out));
        assert!(
            stderr(&out).contains("limit"),
            "{command} --hardened should be refused by the input cap: {}",
            stderr(&out)
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dwg-acadsharp")]
#[test]
fn without_hardened_the_input_cap_is_not_applied() {
    if !native_lane() {
        return;
    }
    let dir = unique_dir("unhardened");
    let huge = oversized_drawing(&dir);
    for command in ["views", "report"] {
        let out = run(&["vector", command, s(&huge)]);
        // The zeros are not a drawing, so this is refused too, by the parser.
        assert_eq!(code(&out), 1, "{command}: stderr:\n{}", stderr(&out));
        assert!(
            !stderr(&out).contains("limit"),
            "{command} applied a limit it was not given: {}",
            stderr(&out)
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
