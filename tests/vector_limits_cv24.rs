//! The vector tiler's limits and the DWG readers' limits as flags on `viprs vector` (CV2.4,
//! libviprs-cli#153).
//!
//! Before this, `build` and `report` reached two `TilerConfig` fields (`--max-zoom`,
//! `--timeout`) and the readers only through `--hardened`. Each cell here runs the real binary over
//! the committed `sample_AC1018.dwg` with the default pure Rust reader and checks what the flag did
//! to the run: the counters `report --json` prints, the tiler metadata a built archive carries,
//! or the exit code and message of a refusal. None of them is satisfied by the flag merely parsing.
//!
//! The ACadSharp half (`--max-input-bytes` and friends reaching `ProviderLimits`) needs the native
//! archive, so it lives with the other native cells in `tests/vector_cli.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(feature = "dwg-acadrust")]
use serde_json::Value;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("the viprs binary must be spawnable")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("a normal exit")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vector/sample_AC1018.dwg")
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[cfg(feature = "dwg-acadrust")]
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-cv24-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

#[cfg(feature = "dwg-acadrust")]
fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// `viprs vector report --json` over the sample at a shallow zoom, plus `extra`.
fn report(extra: &[&str]) -> Output {
    let sample = sample();
    let mut args = vec!["vector", "report", s(&sample), "--json", "--max-zoom", "3"];
    args.extend_from_slice(extra);
    run(&args)
}

/// The JSON of a report that has to succeed.
#[cfg(feature = "dwg-acadrust")]
fn report_ok(extra: &[&str]) -> Value {
    let out = report(extra);
    assert_eq!(code(&out), 0, "{extra:?}: stderr:\n{}", stderr(&out));
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{extra:?}: stdout must be JSON ({e}): {}", stdout(&out)))
}

#[cfg(feature = "dwg-acadrust")]
fn num(j: &Value, a: &str, b: &str) -> f64 {
    j[a][b]
        .as_f64()
        .unwrap_or_else(|| panic!("{a}.{b} must be a number in {j}"))
}

/// A refusal: the exit code it has to have, and stderr.
fn refused(extra: &[&str], want: i32) -> String {
    let out = report(extra);
    assert_eq!(
        code(&out),
        want,
        "{extra:?}: stdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stdout(&out).is_empty(),
        "{extra:?} printed on stdout: {}",
        stdout(&out)
    );
    stderr(&out)
}

// ---------------------------------------------------------------------------
// The help names every flag, on both commands that tile
// ---------------------------------------------------------------------------

const TILER_FLAGS: [&str; 15] = [
    "--extent",
    "--buffer",
    "--margin",
    "--deviation",
    "--simplify",
    "--cull",
    "--min-text",
    "--max-points-per-tile",
    "--max-text-per-tile",
    "--max-level-points",
    "--max-level-bytes",
    "--max-tile-bytes",
    "--oversize",
    "--max-primitives",
    "--max-text-bytes",
];

const READER_FLAGS: [&str; 12] = [
    "--max-block-depth",
    "--max-entities",
    "--max-polyline-points",
    "--max-spline-control-points",
    "--max-spline-knots",
    "--max-insert-grid",
    "--max-expanded-instances",
    "--max-allocation-bytes",
    "--max-name-bytes",
    "--max-input-bytes",
    "--max-string-bytes",
    "--max-output-bytes",
];

#[test]
fn build_and_report_document_every_tiler_and_reader_flag() {
    for command in ["build", "report"] {
        let out = run(&["vector", command, "--help"]);
        assert_eq!(code(&out), 0, "{command}: {}", stderr(&out));
        let help = stdout(&out);
        for flag in TILER_FLAGS.iter().chain(READER_FLAGS.iter()) {
            assert!(
                help.contains(flag),
                "{command} --help lacks {flag}:\n{help}"
            );
        }
    }
    // `views` opens the drawing too, so the reader limits apply there; the tiler's do not.
    let out = run(&["vector", "views", "--help"]);
    let help = stdout(&out);
    for flag in READER_FLAGS {
        assert!(help.contains(flag), "views --help lacks {flag}:\n{help}");
    }
    assert!(!help.contains("--max-tile-bytes"), "{help}");
}

// ---------------------------------------------------------------------------
// Tiler: what each knob does to a run
// ---------------------------------------------------------------------------

#[cfg(feature = "dwg-acadrust")]
#[test]
fn extent_and_buffer_reach_the_archive_the_build_writes() {
    let dir = unique_dir("extent");
    let out_path = dir.join("a.pmtiles");
    let out = run(&[
        "vector",
        "build",
        s(&sample()),
        s(&out_path),
        "--max-zoom",
        "2",
        "--extent",
        "512",
        "--buffer",
        "8",
    ]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let info = run(&["vector", "info", s(&out_path), "--json"]);
    assert_eq!(code(&info), 0, "{}", stderr(&info));
    let j: Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(j["tiler"]["extent"], 512, "{j}");
    assert_eq!(j["tiler"]["buffer"], 8, "{j}");
    let ok = run(&["vector", "validate", s(&out_path)]);
    assert_eq!(code(&ok), 0, "{}", stderr(&ok));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn margin_widens_the_world_the_drawing_is_fitted_into() {
    let base = num(&report_ok(&[]), "tiles", "world_side");
    let wide = num(&report_ok(&["--margin", "0.5"]), "tiles", "world_side");
    // The default leaves 2% each side, so a 50% margin is close to twice the side.
    assert!(
        wide > base * 1.8,
        "world side {base} at the default, {wide} at --margin 0.5"
    );
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn simplify_and_deviation_trade_points_for_precision() {
    let base = num(&report_ok(&[]), "tiles", "points");
    let simplified = num(&report_ok(&["--simplify", "64"]), "tiles", "points");
    assert!(
        simplified < base,
        "{base} points by default, {simplified} at --simplify 64"
    );
    let coarse = num(&report_ok(&["--deviation", "64"]), "tiles", "points");
    assert!(
        coarse < base,
        "{base} points by default, {coarse} at --deviation 64"
    );
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn cull_and_min_text_drop_what_is_too_small_to_see() {
    let base = report_ok(&[]);
    let culled = report_ok(&["--cull", "256"]);
    assert!(
        num(&culled, "fidelity", "culled_subpixel") > num(&base, "fidelity", "culled_subpixel"),
        "--cull 256 culled no more than the default: {} vs {}",
        culled["fidelity"],
        base["fidelity"]
    );
    let text = report_ok(&["--min-text", "100000"]);
    assert!(
        num(&text, "fidelity", "culled_text") > num(&base, "fidelity", "culled_text"),
        "--min-text 100000 culled no more text: {} vs {}",
        text["fidelity"],
        base["fidelity"]
    );
    assert!(num(&text, "tiles", "text") < num(&base, "tiles", "text"));
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn the_per_tile_budgets_drop_runs_and_text_and_say_so() {
    let base = report_ok(&[]);
    assert_eq!(
        num(&base, "fidelity", "dropped_for_point_budget"),
        0.0,
        "{}",
        base["fidelity"]
    );
    assert_eq!(
        num(&base, "fidelity", "dropped_for_text_budget"),
        0.0,
        "{}",
        base["fidelity"]
    );
    let points = report_ok(&["--max-points-per-tile", "2"]);
    assert!(
        num(&points, "fidelity", "dropped_for_point_budget") > 0.0,
        "{}",
        points["fidelity"]
    );
    let text = report_ok(&["--max-text-per-tile", "1"]);
    assert!(
        num(&text, "fidelity", "dropped_for_text_budget") > 0.0,
        "{}",
        text["fidelity"]
    );
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn max_text_bytes_cuts_labels_and_counts_them() {
    let base = report_ok(&[]);
    assert_eq!(
        num(&base, "fidelity", "text_truncated"),
        0.0,
        "{}",
        base["fidelity"]
    );
    let cut = report_ok(&["--max-text-bytes", "1"]);
    assert!(
        num(&cut, "fidelity", "text_truncated") > 0.0,
        "{}",
        cut["fidelity"]
    );
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn an_oversized_tile_is_reported_by_default_and_listed_in_the_text_output() {
    let j = report_ok(&["--max-tile-bytes", "16"]);
    assert!(num(&j, "tiles", "oversized") > 0.0, "{}", j["tiles"]);
    let sample = sample();
    let out = run(&[
        "vector",
        "report",
        s(&sample),
        "--max-zoom",
        "3",
        "--max-tile-bytes",
        "16",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("tiles past the byte budget"),
        "{}",
        stdout(&out)
    );
    // Spelled out, the default is the same.
    let same = report_ok(&["--max-tile-bytes", "16", "--oversize", "report"]);
    assert_eq!(same["tiles"]["oversized"], j["tiles"]["oversized"]);
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn oversize_error_stops_the_build_and_leaves_nothing_behind() {
    let dir = unique_dir("oversize");
    let out_path = dir.join("never.pmtiles");
    let out = run(&[
        "vector",
        "build",
        s(&sample()),
        s(&out_path),
        "--max-zoom",
        "3",
        "--max-tile-bytes",
        "16",
        "--oversize",
        "error",
    ]);
    assert_eq!(
        code(&out),
        1,
        "stdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stderr(&out).contains("byte budget"), "{}", stderr(&out));
    assert!(entries(&dir).is_empty(), "left behind: {:?}", entries(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn the_memory_budgets_refuse_a_run_that_would_pass_them() {
    let err = refused(&["--max-primitives", "1"], 1);
    assert!(err.contains("more than 1 primitives"), "{err}");
    let err = refused(&["--max-level-points", "1"], 1);
    assert!(err.contains("points held at once"), "{err}");
    let err = refused(&["--max-level-bytes", "1"], 1);
    assert!(err.contains("bytes held at once"), "{err}");
}

#[test]
fn a_tiler_value_out_of_range_is_a_usage_error() {
    // Refused while the command line is parsed, before the drawing is opened, so these hold
    // in every build.
    for (extra, names) in [
        (&["--extent", "8"][..], "--extent"),
        (&["--margin=-1"][..], "--margin"),
        (&["--simplify", "NaN"][..], "--simplify"),
        (&["--cull", "inf"][..], "--cull"),
        (&["--max-points-per-tile", "0"][..], "--max-points-per-tile"),
        (&["--max-tile-bytes", "lots"][..], "--max-tile-bytes"),
        (&["--oversize", "maybe"][..], "--oversize"),
    ] {
        let err = refused(extra, 2);
        assert!(err.contains(names), "{extra:?}: {err}");
    }
}

/// Two values that are each fine and wrong together, which only the tiler's own check sees. It
/// runs once the drawing is read, so this needs a reader; it is still the command line's
/// mistake, so still a 2.
#[cfg(feature = "dwg-acadrust")]
#[test]
fn a_tiler_combination_the_library_refuses_is_a_usage_error() {
    for (extra, names) in [
        (&["--extent", "512", "--buffer", "512"][..], "buffer"),
        (&["--deviation", "0"][..], "deviation"),
    ] {
        let err = refused(extra, 2);
        assert!(err.contains(names), "{extra:?}: {err}");
    }
}

// ---------------------------------------------------------------------------
// Readers: the limits reach the decoder
// ---------------------------------------------------------------------------

#[cfg(feature = "dwg-acadrust")]
#[test]
fn an_acadrust_limit_refuses_a_drawing_past_it_and_names_the_limit() {
    let err = refused(&["--max-entities", "1"], 1);
    assert!(err.contains("max_entities"), "{err}");
    // The same ceiling under --hardened: the flag tightens the hardened profile, it does not
    // replace it with the defaults.
    let err = refused(&["--hardened", "--max-entities", "1"], 1);
    assert!(err.contains("max_entities"), "{err}");
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn every_acadrust_limit_at_its_default_changes_nothing() {
    let base = report_ok(&[]);
    let same = report_ok(&[
        "--max-block-depth",
        "64",
        "--max-entities",
        "20000000",
        "--max-polyline-points",
        "1048576",
        "--max-spline-control-points",
        "1048576",
        "--max-spline-knots",
        "1048640",
        "--max-insert-grid",
        "1000000",
        "--max-expanded-instances",
        "10000000",
        "--max-allocation-bytes",
        "2147483648",
        "--max-name-bytes",
        "1020",
    ]);
    assert_eq!(same["tiles"], base["tiles"]);
    assert_eq!(same["fidelity"], base["fidelity"]);
}

#[cfg(feature = "dwg-acadrust")]
#[test]
fn the_acadrust_allocation_and_name_limits_reach_the_walk() {
    let err = refused(&["--max-allocation-bytes", "1"], 1);
    assert!(err.contains("max_allocation_bytes"), "{err}");
    let err = refused(&["--max-name-bytes", "0"], 1);
    assert!(err.contains("max_name_bytes"), "{err}");
}

#[test]
fn a_limit_the_chosen_reader_does_not_have_is_a_usage_error() {
    // acadrust has no input, string or output cap of its own (its one input cap is --hardened),
    // and ACadSharp has no INSERT-walk ceilings. Saying so beats silently ignoring the flag.
    // Checked before the reader is built, so this holds in every build.
    for flag in [
        "--max-input-bytes",
        "--max-string-bytes",
        "--max-output-bytes",
    ] {
        let err = refused(&[flag, "1000", "--provider", "acadrust"], 2);
        assert!(
            err.contains(flag) && err.contains("acadsharp"),
            "{flag}: {err}"
        );
    }
    for flag in [
        "--max-spline-control-points",
        "--max-spline-knots",
        "--max-insert-grid",
        "--max-expanded-instances",
        "--max-allocation-bytes",
        "--max-name-bytes",
    ] {
        let err = refused(&[flag, "1000", "--provider", "acadsharp"], 2);
        assert!(
            err.contains(flag) && err.contains("acadrust"),
            "{flag}: {err}"
        );
    }
}
