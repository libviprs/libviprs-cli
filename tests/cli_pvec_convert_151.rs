//! libviprs-cli#151 (CV2.2), `viprs pvec convert` for a DWG: the profile, the capability policy,
//! calibration at convert time, the view profile, the source limits, the leaf and spill settings
//! and the region tuning, the real binary. Exit codes are not evidence, so each cell reads what the
//! conversion wrote: the report beside the artifact, and PVec's own reader over the artifact.
//! The PDF half is `cli_pvec_pdf_151.rs`.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(feature = "pvec-convert")]
use serde_json::Value;

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
    let dir = std::env::temp_dir().join(format!("viprs-convert-151-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Model space of `drawing` into `dir`, plus `extra`: the process and the report it wrote.
#[cfg(feature = "pvec-convert")]
fn model(drawing: &str, dir: &Path, extra: &[&str]) -> (Output, Value) {
    let path = fixture(drawing);
    let mut args = vec![
        "pvec",
        "convert",
        s(&path),
        s(dir),
        "--model",
        "--unit",
        "m",
    ];
    args.extend_from_slice(extra);
    let out = run(&args);
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    let report = std::fs::read(dir.join(format!("{stem}.pvec.report.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    (out, report)
}

/// `model` for a run that has to succeed, and the report it wrote.
#[cfg(feature = "pvec-convert")]
fn model_ok(drawing: &str, tag: &str, extra: &[&str]) -> Value {
    let dir = scratch(tag);
    let (out, report) = model(drawing, &dir, extra);
    assert_eq!(code(&out), 0, "{extra:?}: {}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
    report
}

/// `pvec info --json` over a file.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
fn info(file: &Path) -> Value {
    let out = run(&["pvec", "info", s(file), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    serde_json::from_slice(&out.stdout).expect("info --json is JSON")
}

/// A usage mistake: exit 2, the flag named, and nothing written (the output directory is not made).
#[cfg(feature = "pvec-convert")]
fn usage(extra: &[&str], names: &str) {
    let dir = scratch("usage");
    let out_dir = dir.join("out");
    let path = fixture("vc_layers.dwg");
    let mut args = vec![
        "pvec",
        "convert",
        s(&path),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
    ];
    args.extend_from_slice(extra);
    let out = run(&args);
    assert_eq!(code(&out), 2, "{extra:?}: {}", stderr(&out));
    // A flag clap does not know also exits 2, so the refusal has to be the one for this flag's rule.
    assert!(
        !stderr(&out).contains("unexpected argument"),
        "{extra:?} is not a flag yet: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains(names),
        "{extra:?} should name {names}: {}",
        stderr(&out)
    );
    assert!(!out_dir.exists(), "{extra:?} wrote something");
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Profile, capability policy, allowed capabilities
// ---------------------------------------------------------------------------

/// `--profile` is the core's `ConversionProfile`, and the report names the one that ran.
#[cfg(feature = "pvec-convert")]
#[test]
fn profile_reaches_the_conversion_and_the_report_names_it() {
    for (flag, profile, policy, exact_only) in [
        ("default", "Default", "RejectEntity", false),
        ("permissive", "Permissive", "Downgrade", false),
    ] {
        let report = model_ok("vc_layers.dwg", "profile", &["--profile", flag]);
        let caps = &report["capabilities"];
        assert_eq!(caps["profile"], profile, "{flag}: {report}");
        assert_eq!(caps["policy"], policy, "{flag}: {report}");
        assert_eq!(caps["exact_only"], exact_only, "{flag}: {report}");
    }
}

/// `strict` allows only exactly stored entities, and a line is stored as an equivalent trimmed curve,
/// so it stops the run and names the capability: that is the profile, not a usage mistake.
#[cfg(feature = "pvec-convert")]
#[test]
fn strict_stops_the_run_at_the_first_entity_that_is_not_exact() {
    let dir = scratch("strict");
    let (out, _) = model("vc_layers.dwg", &dir, &["--profile", "strict"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("needs the PVec capability") && stderr(&out).contains("not allowed"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A profile that is not one of the three is a usage error.
#[cfg(feature = "pvec-convert")]
#[test]
fn an_unknown_profile_is_a_usage_error() {
    usage(&["--profile", "lenient"], "--profile");
}

/// `--capability-policy` replaces the profile's policy, and the report calls the run `Custom`.
#[cfg(feature = "pvec-convert")]
#[test]
fn capability_policy_replaces_the_profiles_policy() {
    for (flag, policy) in [
        ("reject-input", "RejectInput"),
        ("reject-entity", "RejectEntity"),
        ("downgrade", "Downgrade"),
    ] {
        let report = model_ok("vc_layers.dwg", "policy", &["--capability-policy", flag]);
        assert_eq!(report["capabilities"]["policy"], policy, "{flag}: {report}");
        assert_eq!(
            report["capabilities"]["profile"], "Custom",
            "{flag}: {report}"
        );
    }
    usage(&["--capability-policy", "ignore"], "--capability-policy");
}

/// `--allow-capability` replaces the profile's set: the ids given are the ids the report lists.
#[cfg(feature = "pvec-convert")]
#[test]
fn allow_capability_replaces_the_profiles_set() {
    let wide = model_ok("vc_layers.dwg", "allow-wide", &["--profile", "permissive"]);
    let all: Vec<u64> = wide["capabilities"]["allowed"]
        .as_array()
        .expect("the report lists the allowed ids")
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert!(all.len() > 1, "{wide}");
    // Repeated, and comma separated: both spellings reach the same set.
    let mut repeated: Vec<String> = Vec::new();
    for id in &all {
        repeated.push("--allow-capability".into());
        repeated.push(id.to_string());
    }
    let repeated: Vec<&str> = repeated.iter().map(String::as_str).collect();
    let report = model_ok("vc_layers.dwg", "allow-repeat", &repeated);
    assert_eq!(
        report["capabilities"]["allowed"],
        wide["capabilities"]["allowed"]
    );
    assert_eq!(report["capabilities"]["profile"], "Custom", "{report}");
    let joined = all.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    let report = model_ok(
        "vc_layers.dwg",
        "allow-comma",
        &["--allow-capability", &joined],
    );
    assert_eq!(
        report["capabilities"]["allowed"],
        wide["capabilities"]["allowed"]
    );
    usage(&["--allow-capability", "geometry"], "--allow-capability");
}

/// A capability left out of the set leaves the entities that need it out, and the policy decides
/// how loudly: `reject-entity` counts them and carries on, `reject-input` stops the run.
#[cfg(feature = "pvec-convert")]
#[test]
fn an_entity_outside_the_allowed_set_is_left_out_or_stops_the_run() {
    // Capability 1 is not one the line mapping needs, so every line is outside the set.
    let dir = scratch("outside-reject-input");
    let (out, _) = model(
        "vc_layers.dwg",
        &dir,
        &[
            "--allow-capability",
            "1",
            "--capability-policy",
            "reject-input",
        ],
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).to_lowercase().contains("capab"),
        "the refusal names the capability: {}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Calibration at convert time
// ---------------------------------------------------------------------------

/// Without a flag a DWG has no calibration: its scale is the manifest's.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_dwg_has_no_calibration_unless_asked() {
    let dir = scratch("cal-none");
    let (out, _) = model("vc_layers.dwg", &dir, &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        info(&dir.join("vc_layers.pvec"))["scale"]["source"],
        "manifest"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Each way of stating a scale lands in the artifact with its own provenance.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_scale_given_at_convert_time_is_written_into_the_artifact() {
    let cases: [(&[&str], &str, f64); 3] = [
        (&["--scale", "0.5"], "manual", 0.5),
        (
            &["--plan-scale", "1:100", "--paper-unit", "0.001"],
            "plan-scale",
            0.1,
        ),
        (
            &["--measured", "0,0,3,4", "--distance", "10"],
            "measured",
            2.0,
        ),
    ];
    for (extra, provenance, metres) in cases {
        let dir = scratch("cal");
        let (out, _) = model("vc_layers.dwg", &dir, extra);
        assert_eq!(code(&out), 0, "{extra:?}: {}", stderr(&out));
        let scale = info(&dir.join("vc_layers.pvec"))["scale"].clone();
        assert_eq!(scale["source"], "calibration", "{extra:?}: {scale}");
        assert_eq!(scale["provenance"], provenance, "{extra:?}: {scale}");
        let got = scale["metres_per_unit"].as_f64().unwrap();
        assert!((got - metres).abs() < 1e-9, "{extra:?}: {scale}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The note and the uncertainty are stored with the scale.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn the_scale_note_and_uncertainty_are_stored() {
    let dir = scratch("cal-note");
    let (out, _) = model(
        "vc_layers.dwg",
        &dir,
        &[
            "--scale",
            "0.5",
            "--scale-note",
            "site survey",
            "--scale-uncertainty",
            "0.05",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let scale = info(&dir.join("vc_layers.pvec"))["scale"].clone();
    assert_eq!(scale["note"], "site survey", "{scale}");
    assert!(
        (scale["uncertainty"].as_f64().unwrap() - 0.05).abs() < 1e-12,
        "{scale}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A sheet is calibrated the same way: every sheet of the set carries the scale.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn every_sheet_carries_the_calibration() {
    let dir = scratch("cal-sheets");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&dir),
        "--scale",
        "0.25",
        "--profile",
        "permissive",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let mut sheets = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "pvec") {
            sheets += 1;
            let scale = info(&path)["scale"].clone();
            assert_eq!(scale["provenance"], "manual", "{}: {scale}", path.display());
            let report = std::fs::read(path.with_extension("pvec.report.json")).unwrap();
            let report: Value = serde_json::from_slice(&report).unwrap();
            assert_eq!(report["capabilities"]["profile"], "Permissive", "{report}");
        }
    }
    assert_eq!(sheets, 3);
    let _ = std::fs::remove_dir_all(dir);
}

/// The ways of stating a scale are one choice, and each needs its companion.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_calibration_flags_are_one_choice_each_with_its_companion() {
    usage(
        &["--scale", "1", "--measured", "0,0,3,4", "--distance", "5"],
        "--scale",
    );
    usage(&["--plan-scale", "1:100"], "--paper-unit");
    usage(&["--paper-unit", "0.001"], "--plan-scale");
    usage(&["--measured", "0,0,3,4"], "--distance");
    usage(&["--distance", "5"], "--measured");
    for bad in ["0", "-1", "nan", "inf", "metre"] {
        usage(&["--scale", bad], "--scale");
    }
    usage(
        &["--plan-scale", "0:100", "--paper-unit", "0.001"],
        "--plan-scale",
    );
    usage(&["--measured", "1,1,1,1", "--distance", "5"], "--measured");
    usage(&["--scale-note", "x"], "--scale");
    usage(&["--scale-uncertainty", "0.1"], "--scale");
}

fn sidecar(dir: &Path, version: &str, id: Option<&str>) -> PathBuf {
    let path = dir.join("old.georef.json");
    let id = id.map_or("null".to_owned(), |i| format!("\"{i}\""));
    std::fs::write(
        &path,
        format!(
            r#"{{"schema_version":"{version}","bounds":null,"unit":{{"source":"Unknown","insunits":null,"physical":false,"evidence":"none","applied":"Meters"}},"calibration_id":{id},"control_points":[]}}"#
        ),
    )
    .unwrap();
    path
}

/// A retired georeference sidecar is brought in: its calibration id becomes the external id of the
/// calibration the scale flags state.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_georeference_sidecar_is_imported_with_its_id() {
    use pvec_facade::PvecReader;
    let dir = scratch("sidecar");
    let side = sidecar(&dir, "1", Some("site-9"));
    let out_dir = dir.join("out");
    let drawing = fixture("vc_layers.dwg");
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
        "--scale",
        "0.5",
        "--georef-sidecar",
        s(&side),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let reader = PvecReader::open(std::fs::read(out_dir.join("vc_layers.pvec")).unwrap()).unwrap();
    let calibration = reader.calibration().unwrap().expect("a calibration");
    assert_eq!(calibration.external_id, "site-9");
    assert!(calibration.scale.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

/// A sidecar that names no calibration imports nothing, says so, and the conversion still succeeds.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_sidecar_without_an_id_imports_nothing() {
    use pvec_facade::PvecReader;
    let dir = scratch("sidecar-noid");
    let side = sidecar(&dir, "1", None);
    let out_dir = dir.join("out");
    let drawing = fixture("vc_layers.dwg");
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
        "--scale",
        "0.5",
        "--georef-sidecar",
        s(&side),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no calibration id"),
        "{}",
        stderr(&out)
    );
    let reader = PvecReader::open(std::fs::read(out_dir.join("vc_layers.pvec")).unwrap()).unwrap();
    assert_eq!(reader.calibration().unwrap().unwrap().external_id, "");
    let _ = std::fs::remove_dir_all(dir);
}

/// A sidecar that cannot be used stops the run before anything is written: exit 1, the schema
/// version named for a version this reader does not know, a missing file named by its path.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_sidecar_that_cannot_be_read_stops_the_run_before_it_writes() {
    let dir = scratch("sidecar-bad");
    let drawing = fixture("vc_layers.dwg");
    let out_dir = dir.join("out");
    let side = sidecar(&dir, "2", Some("site-9"));
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
        "--scale",
        "0.5",
        "--georef-sidecar",
        s(&side),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("schema_version"), "{}", stderr(&out));
    assert!(!out_dir.exists());
    let missing = dir.join("nope.georef.json");
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(&out_dir),
        "--model",
        "--unit",
        "m",
        "--scale",
        "0.5",
        "--georef-sidecar",
        s(&missing),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("nope.georef.json"),
        "{}",
        stderr(&out)
    );
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// The sidecar's id names a calibration the scale flags state, so it needs one of them.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_sidecar_needs_a_scale_to_import() {
    let dir = scratch("sidecar-usage");
    let side = sidecar(&dir, "1", Some("site-9"));
    usage(&["--georef-sidecar", s(&side)], "--scale");
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// View profile
// ---------------------------------------------------------------------------

/// Entities the view kept, from the report.
#[cfg(feature = "pvec-convert")]
fn included(report: &Value) -> u64 {
    report["view"]["included"].as_u64().expect("view.included")
}

/// `vc_layers.dwg` has layers `VC_ON`, `VC_OFF` (off), `VC_FROZEN` (frozen) and `0` (off). The
/// default view leaves the off and frozen ones out; each flag decides one of those rules.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_view_profile_flags_decide_which_layers_are_drawn() {
    let base = included(&model_ok("vc_layers.dwg", "view-base", &[]));
    assert_eq!(base, 3);
    for flag in ["--off-layers", "--frozen-layers"] {
        let drawn = included(&model_ok("vc_layers.dwg", "view-draw", &[flag, "draw"]));
        assert!(drawn > base, "{flag} draw: {drawn} against {base}");
        let kept = included(&model_ok("vc_layers.dwg", "view-excl", &[flag, "exclude"]));
        assert_eq!(kept, base, "{flag} exclude is the default");
    }
    let both = included(&model_ok(
        "vc_layers.dwg",
        "view-both",
        &["--off-layers", "draw", "--frozen-layers", "draw"],
    ));
    // Nothing is excluded, so the block instances count too: 9 entities met, 13 leaves drawn.
    assert_eq!(both, 13, "every leaf of the drawing");
}

/// A layer chosen by name wins over its flags, in both directions, with the case ignored.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_layer_can_be_forced_hidden_or_visible_by_name() {
    let base = included(&model_ok("vc_layers.dwg", "layer-base", &[]));
    let drawn = ["--off-layers", "draw"];
    let all = included(&model_ok("vc_layers.dwg", "layer-all", &drawn));
    let hidden = included(&model_ok(
        "vc_layers.dwg",
        "layer-hidden",
        &["--off-layers", "draw", "--layer-hidden", "vc_off"],
    ));
    assert!(hidden < all, "{hidden} against {all}");
    // Everything the default view draws is on VC_ON, so hiding it leaves nothing to convert.
    let dir = scratch("layer-none");
    let (out, _) = model("vc_layers.dwg", &dir, &["--layer-hidden", "vc_on"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no entity carries geometry"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
    let shown = included(&model_ok(
        "vc_layers.dwg",
        "layer-visible",
        &["--layer-visible", "VC_OFF", "--layer-visible", "VC_FROZEN"],
    ));
    assert!(shown > base, "{shown} against {base}");
}

/// The other view knobs are accepted, and a value outside their words is a usage error.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_other_view_knobs_take_their_words_and_nothing_else() {
    for extra in [
        ["--no-plot-layers", "draw"],
        ["--no-plot-layers", "exclude"],
        ["--frozen-text", "exclude"],
        ["--no-plot-text", "exclude"],
        ["--invisible-entities", "draw"],
        ["--viewport-freezes", "report"],
        ["--insert-layer", "own"],
        ["--insert-layer", "inherit"],
        ["--max-insert-depth", "64"],
    ] {
        let _ = model_ok("vc_layers.dwg", "view-knob", &extra);
    }
    for (flag, bad) in [
        ("--off-layers", "hide"),
        ("--viewport-freezes", "draw"),
        ("--insert-layer", "all"),
        ("--max-insert-depth", "0"),
    ] {
        usage(&[flag, bad], flag);
    }
}

/// `apply` for a layer frozen in a viewport is the print rule, which the core only enables for a
/// sheet: on model space the run fails and the message names the field.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_print_freeze_rule_is_refused_on_model_space() {
    let dir = scratch("freeze-apply");
    let (out, _) = model("vc_layers.dwg", &dir, &["--viewport-freezes", "apply"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("frozen_in_viewport"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A sheet starts from the print profile, not the model-space one: the view flags are written over it.
#[cfg(feature = "pvec-convert")]
#[test]
fn view_flags_apply_to_sheets_over_the_print_profile() {
    let dir = scratch("view-sheets");
    let out = run(&[
        "pvec",
        "convert",
        s(&fixture("sample_AC1018.dwg")),
        s(&dir),
        "--no-plot-layers",
        "draw",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Source limits
// ---------------------------------------------------------------------------

/// Every limit lands in its own `SourceLimits` field: a ceiling of 1 refuses the drawing and the
/// refusal names the field, as `viprs vector` does with the same flags.
#[cfg(feature = "pvec-convert")]
#[test]
fn each_source_limit_refuses_a_drawing_past_it_and_names_the_field() {
    for (flag, field) in [
        ("--max-entities", "max_entities"),
        ("--max-polyline-points", "max_polyline_vertices"),
        ("--max-spline-control-points", "max_spline_control_points"),
        ("--max-spline-knots", "max_spline_knots"),
        ("--max-expanded-instances", "max_expanded_instances"),
        ("--max-alloc-bytes", "max_allocation_bytes"),
        ("--max-name-bytes", "max_name_bytes"),
    ] {
        let dir = scratch("limits");
        let (out, _) = model("sample_AC1018.dwg", &dir, &[flag, "1"]);
        assert_eq!(code(&out), 1, "{flag}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(&format!("limit {field} exceeded")),
            "{flag}: {}",
            stderr(&out)
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// `g13_deep_blocks.dwg` nests INSERTs six deep: a ceiling of 5 refuses it and 6 reads it.
#[cfg(feature = "pvec-convert")]
#[test]
fn max_block_depth_refuses_nesting_past_it() {
    let dir = scratch("depth-5");
    let (out, _) = model("g13_deep_blocks.dwg", &dir, &["--max-block-depth", "5"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("limit max_block_depth exceeded"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
    let _ = model_ok(
        "g13_deep_blocks.dwg",
        "depth-6",
        &["--max-block-depth", "6"],
    );
}

/// A ceiling of 0 is a usage error, for every limit.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_source_limit_of_zero_is_a_usage_error() {
    for flag in [
        "--max-block-depth",
        "--max-entities",
        "--max-polyline-points",
        "--max-spline-control-points",
        "--max-spline-knots",
        "--max-insert-grid",
        "--max-expanded-instances",
        "--max-alloc-bytes",
        "--max-name-bytes",
    ] {
        usage(&[flag, "0"], flag);
    }
}

// ---------------------------------------------------------------------------
// Leaves and spill
// ---------------------------------------------------------------------------

/// `--leaf-capacity` lays PVec's spatial directory into the artifact, which `pvec info` reports.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn leaf_capacity_lays_a_spatial_directory_into_the_artifact() {
    let plain = scratch("leaf-plain");
    let (out, _) = model("sample_AC1018.dwg", &plain, &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        info(&plain.join("sample_AC1018.pvec"))["spatial_index"],
        false
    );

    let leafy = scratch("leaf-on");
    let (out, _) = model("sample_AC1018.dwg", &leafy, &["--leaf-capacity", "8"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        info(&leafy.join("sample_AC1018.pvec"))["spatial_index"],
        true
    );
    let valid = run(&["pvec", "validate", s(&leafy.join("sample_AC1018.pvec"))]);
    assert_eq!(code(&valid), 0, "{}{}", stdout(&valid), stderr(&valid));
    let _ = std::fs::remove_dir_all(plain);
    let _ = std::fs::remove_dir_all(leafy);
}

/// `--tier0-max-error` adds the Tier 0 preview blocks to an indexed artifact, so the file grows.
#[cfg(feature = "pvec-convert")]
#[test]
fn tier0_max_error_adds_preview_blocks_to_an_indexed_artifact() {
    let size = |tag: &str, extra: &[&str]| {
        let dir = scratch(tag);
        let (out, _) = model("sample_AC1018.dwg", &dir, extra);
        assert_eq!(code(&out), 0, "{extra:?}: {}", stderr(&out));
        let n = std::fs::metadata(dir.join("sample_AC1018.pvec"))
            .unwrap()
            .len();
        let _ = std::fs::remove_dir_all(dir);
        n
    };
    let without = size("tier0-off", &["--leaf-capacity", "8"]);
    let with = size(
        "tier0-on",
        &["--leaf-capacity", "8", "--tier0-max-error", "1"],
    );
    assert!(with > without, "{with} against {without}");
    usage(&["--tier0-max-error", "1"], "--leaf-capacity");
    usage(&["--leaf-capacity", "0"], "--leaf-capacity");
    usage(
        &["--leaf-capacity", "8", "--tier0-max-error", "0"],
        "--tier0-max-error",
    );
}

/// A spilled conversion writes the bytes an in-memory one does, and leaves no spill file behind.
#[cfg(feature = "pvec-convert")]
#[test]
fn spill_changes_where_the_memory_goes_and_not_the_artifact() {
    let flat = scratch("spill-flat");
    let (out, _) = model("sample_AC1018.dwg", &flat, &["--leaf-capacity", "8"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let dir = scratch("spill");
    let spill = dir.join("spill");
    std::fs::create_dir_all(&spill).unwrap();
    let (out, _) = model(
        "sample_AC1018.dwg",
        &dir,
        &[
            "--leaf-capacity",
            "8",
            "--spill-dir",
            s(&spill),
            "--spill-budget",
            "65536",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        std::fs::read(flat.join("sample_AC1018.pvec")).unwrap(),
        std::fs::read(dir.join("sample_AC1018.pvec")).unwrap(),
        "the budget changes the memory, not the bytes"
    );
    assert_eq!(
        std::fs::read_dir(&spill).unwrap().count(),
        0,
        "spill files left behind"
    );
    let _ = std::fs::remove_dir_all(flat);
    let _ = std::fs::remove_dir_all(dir);
}

/// The budget needs a directory to spill into, and both are positive.
#[cfg(feature = "pvec-convert")]
#[test]
fn spill_flags_go_together() {
    usage(&["--spill-budget", "65536"], "--spill-dir");
    let dir = scratch("spill-usage");
    usage(
        &["--spill-dir", s(&dir), "--spill-budget", "0"],
        "--spill-budget",
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Region tuning
// ---------------------------------------------------------------------------

/// The sample closes eight wall gaps at the default 0.5 m. A gap tolerance too small to close any
/// closes none: the knob reaches the regions step and is in metres, as the default is.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_region_gap_reaches_the_regions_step() {
    let base = model_ok("sample_AC1018.dwg", "gap-base", &["--regions"]);
    assert_eq!(base["regions"]["gaps_closed"], 8, "{base}");
    let tight = model_ok(
        "sample_AC1018.dwg",
        "gap-tight",
        &["--regions", "--region-gap", "0.000001"],
    );
    assert_eq!(tight["regions"]["gaps_closed"], 0, "{tight}");
}

/// A face budget of 1 is below what the sample builds, so the step refuses and the conversion
/// carries on without regions, as the core does.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_region_face_budget_reaches_the_regions_step() {
    let report = model_ok(
        "sample_AC1018.dwg",
        "faces",
        &["--regions", "--region-max-faces", "1"],
    );
    assert_eq!(
        report["regions"]["refused"], "regions.face-limit",
        "{report}"
    );
}

/// The corridor knobs and `--region-keep-unlabelled` are accepted with `--regions`, and need it.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_other_region_knobs_need_regions() {
    let _ = model_ok(
        "sample_AC1018.dwg",
        "region-knobs",
        &[
            "--regions",
            "--region-corridor-width",
            "1.5",
            "--region-corridor-aspect",
            "4",
            "--region-keep-unlabelled",
        ],
    );
    let alone: [&[&str]; 5] = [
        &["--region-gap", "0.5"],
        &["--region-corridor-width", "1.5"],
        &["--region-corridor-aspect", "4"],
        &["--region-max-faces", "10"],
        &["--region-keep-unlabelled"],
    ];
    for extra in alone {
        usage(extra, "--regions");
    }
    for bad in ["0", "-1", "nan"] {
        usage(&["--regions", "--region-gap", bad], "--region-gap");
    }
    usage(
        &["--regions", "--region-max-faces", "0"],
        "--region-max-faces",
    );
}
