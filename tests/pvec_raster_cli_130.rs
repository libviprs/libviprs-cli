//! Issue #130 (RL1.4): every refusal of `viprs pyramid --storage pvec`, `viprs pmtiles to-pvec`
//! and `viprs pvec rasters` that the command line alone decides is a usage error: exit 2, and a
//! hint that names the way out. None of these needs a file, so they run in every build, and a
//! build without the `pvec-raster` feature is held to exiting 1 and naming it.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("the viprs binary must be spawnable")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the process must exit normally")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A command line that is fine as far as the command line goes. The input and the drawing do not
/// exist, so a run that gets past the usage checks exits 1 and not 2.
fn valid() -> Vec<&'static str> {
    vec![
        "pyramid",
        "in.png",
        "out.pvec",
        "--storage",
        "pvec",
        "--pvec-into",
        "drawing.pvec",
        "--geo-origin",
        "0,0",
        "--geo-scale",
        "1,-1",
        "--source-to-geo",
        "1,0,0,0,1,0",
    ]
}

fn without(drop: &[&str]) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut skip = 0;
    for a in valid() {
        if skip > 0 {
            skip -= 1;
        } else if drop.contains(&a) {
            skip = 1;
        } else {
            out.push(a);
        }
    }
    out
}

/// `valid()` plus `extra`.
fn with(extra: &[&'static str]) -> Vec<&'static str> {
    let mut v = valid();
    v.extend_from_slice(extra);
    v
}

/// Exit 2 with every `needle` somewhere on stderr.
#[track_caller]
fn usage(args: &[&str], needles: &[&str]) {
    let out = run(args);
    let err = stderr(&out);
    assert_eq!(code(&out), 2, "{args:?} must be a usage error, got:\n{err}");
    for needle in needles {
        assert!(
            err.contains(needle),
            "{args:?}: stderr must contain {needle:?}, got:\n{err}"
        );
    }
}

#[test]
fn a_tile_size_pvec_does_not_take_is_a_usage_error() {
    for size in ["128", "300", "2048"] {
        usage(
            &with(&["--tile-size", size]),
            &["256, 512 or 1024", "Hint:"],
        );
    }
}

#[test]
fn the_tile_sizes_pvec_takes_get_past_the_usage_checks() {
    for size in ["256", "512", "1024"] {
        let out = run(&with(&["--tile-size", size]));
        assert_eq!(code(&out), 1, "{size}: {}", stderr(&out));
    }
}

#[test]
fn centring_and_overlap_are_refused_with_their_reason() {
    usage(
        &with(&["--centre"]),
        &["--centre", "never centred", "Hint:"],
    );
    usage(
        &with(&["--overlap", "2"]),
        &["--overlap", "no overlap", "Hint:"],
    );
}

#[test]
fn the_layout_is_fixed_to_google() {
    usage(&with(&["--layout", "xyz"]), &["--layout", "google"]);
    usage(&with(&["--layout", "deep-zoom"]), &["--layout", "google"]);
    let out = run(&with(&["--layout", "google"]));
    assert_eq!(code(&out), 1, "{}", stderr(&out));
}

#[test]
fn raw_tiles_and_placeholder_blanks_are_refused() {
    usage(
        &with(&["--format", "raw"]),
        &["--format raw", "--format png"],
    );
    usage(&with(&["--skip-blank"]), &["placeholder", "--drop-blanks"]);
    usage(
        &with(&["--blank-tolerance", "3"]),
        &["placeholder", "--drop-blanks"],
    );
}

#[test]
fn dropping_blanks_is_how_a_blank_tile_becomes_a_not_stored_tile() {
    let out = run(&with(&["--drop-blanks"]));
    assert_eq!(code(&out), 1, "{}", stderr(&out));
}

#[test]
fn flags_for_a_tree_or_an_archive_do_not_apply() {
    for flag in [
        "--resume",
        "--verify",
        "--dedupe-blanks",
        "--dedupe-all",
        "--ordered-emission",
    ] {
        usage(&with(&[flag]), &[flag, "--storage pvec"]);
    }
}

#[test]
fn the_drawing_is_required_because_an_artifact_needs_an_entity() {
    usage(
        &without(&["--pvec-into"]),
        &["--pvec-into", "WRT-01", "Hint:"],
    );
}

#[test]
fn the_output_is_required() {
    let mut args = without(&[]);
    args.retain(|a| *a != "out.pvec");
    usage(&args, &["no output", "Hint:"]);
}

#[test]
fn a_placement_is_required_and_comes_from_one_place() {
    // Nothing at all.
    usage(
        &without(&["--geo-origin", "--geo-scale", "--source-to-geo"]),
        &["no placement", "--pixel-to-source"],
    );
    // A transform of the raster's own with no calibration to land it in.
    usage(&without(&["--source-to-geo"]), &["--source-to-geo"]);
    // A calibration with no transform of the raster's own.
    usage(
        &without(&["--geo-origin", "--geo-scale"]),
        &["--source-to-geo", "--geo-origin"],
    );
}

#[test]
fn the_placement_flags_exclude_each_other_in_the_documented_way() {
    let direct = "1,0,0,-1,0,100";
    // --pixel-to-source is the placement outright, so it takes neither a calibration nor a transform.
    usage(
        &with(&["--pixel-to-source", direct]),
        &["--pixel-to-source"],
    );
    usage(
        &without(&["--source-to-geo"])
            .into_iter()
            .chain(["--affine", "1,0,0,0,-1,0", "--pixel-to-source", direct])
            .collect::<Vec<_>>(),
        &["--affine"],
    );
    // --affine is the general form of --geo-origin and --geo-scale.
    usage(&with(&["--affine", "1,0,0,0,-1,0"]), &["--affine"]);
}

#[test]
fn pixel_to_source_alone_is_a_complete_placement() {
    let mut args = without(&["--geo-origin", "--geo-scale", "--source-to-geo"]);
    args.extend(["--pixel-to-source", "1,0,0,-1,0,100"]);
    let out = run(&args);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
}

#[test]
fn a_bad_number_list_is_a_usage_error_like_viprs_geos() {
    for bad in [
        "1,0,0,nan,0,0",
        "1,0,0,inf,0,0",
        "1,0,0,1e400,0,0",
        "1,0,0,-1,0",
        "1,0,0,-1,0,0,0",
        "a,b,c,d,e,f",
    ] {
        let mut args = without(&["--geo-origin", "--geo-scale", "--source-to-geo"]);
        args.extend(["--pixel-to-source", bad]);
        usage(&args, &["--pixel-to-source"]);
    }
    usage(
        &without(&["--source-to-geo"])
            .into_iter()
            .chain(["--source-to-geo", "1,0,0,0,1,inf"])
            .collect::<Vec<_>>(),
        &["--source-to-geo"],
    );
}

#[test]
fn band_opacity_and_attribution_are_checked_on_the_command_line() {
    usage(&with(&["--band", "2"]), &["--band"]);
    usage(&with(&["--opacity", "256"]), &["--opacity"]);
    usage(&with(&["--opacity", "-1"]), &[]);
    usage(
        &with(&["--attribution", "line one\nline two"]),
        &["--attribution", "control character"],
    );
    usage(&with(&["--attribution", "tab\there"]), &["--attribution"]);
    let long = "x".repeat(256);
    let out = run(&{
        let mut v = valid();
        v.push("--attribution");
        v.push(&long);
        v
    });
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("255"), "{}", stderr(&out));
    for ok in [("--band", "1"), ("--opacity", "0"), ("--layer", "Scan")] {
        let out = run(&with(&[ok.0, ok.1]));
        assert_eq!(code(&out), 1, "{ok:?}: {}", stderr(&out));
    }
}

#[test]
fn the_pvec_flags_are_refused_on_a_run_that_is_not_going_to_a_pvec() {
    for (flag, value) in [
        ("--pvec-into", "d.pvec"),
        ("--source-to-geo", "1,0,0,0,1,0"),
        ("--pixel-to-source", "1,0,0,-1,0,0"),
        ("--band", "1"),
        ("--opacity", "3"),
        ("--attribution", "credit"),
        ("--layer", "Scan"),
    ] {
        usage(
            &["pyramid", "in.png", "out.pmtiles", flag, value],
            &[flag, "--storage pvec"],
        );
    }
}

#[test]
fn to_pvec_needs_the_drawing_and_a_placement() {
    usage(
        &[
            "pmtiles",
            "to-pvec",
            "a.pmtiles",
            "out.pvec",
            "--source-to-geo",
            "1,0,0,0,1,0",
        ],
        &["--pvec-into", "WRT-01"],
    );
    usage(
        &[
            "pmtiles",
            "to-pvec",
            "a.pmtiles",
            "out.pvec",
            "--pvec-into",
            "d.pvec",
        ],
        &["no way to place", "--source-to-geo"],
    );
    usage(
        &[
            "pmtiles",
            "to-pvec",
            "a.pmtiles",
            "out.pvec",
            "--pvec-into",
            "d.pvec",
            "--pixel-to-source",
            "1,0,0,nan,0,0",
        ],
        &["--pixel-to-source"],
    );
    usage(
        &[
            "pmtiles",
            "to-pvec",
            "a.pmtiles",
            "out.pvec",
            "--pvec-into",
            "d.pvec",
            "--source-to-geo",
            "1,0,0,0,1,0",
            "--pixel-to-source",
            "1,0,0,-1,0,0",
        ],
        &["--pixel-to-source"],
    );
    usage(
        &[
            "pmtiles",
            "to-pvec",
            "a.pmtiles",
            "out.pvec",
            "--pvec-into",
            "d.pvec",
            "--source-to-geo",
            "1,0,0,0,1,0",
            "--band",
            "3",
        ],
        &["--band"],
    );
}

#[test]
fn pvec_rasters_takes_a_tile_as_five_values_and_numbers_for_four_of_them() {
    usage(
        &[
            "pvec", "rasters", "f.pvec", "--tile", "a", "0", "0", "0", "-",
        ],
        &["--tile", "whole number"],
    );
    usage(&["pvec", "rasters", "f.pvec", "--tile", "0", "0", "0"], &[]);
    usage(
        &[
            "pvec", "rasters", "f.pvec", "--json", "--tile", "0", "0", "0", "0", "-",
        ],
        &[],
    );
}

#[cfg(not(feature = "pvec-raster"))]
#[test]
fn a_build_without_the_feature_exits_1_and_names_it() {
    for args in [
        valid(),
        vec!["pvec", "rasters", "f.pvec"],
        vec![
            "pmtiles",
            "to-pvec",
            "a.pmtiles",
            "out.pvec",
            "--pvec-into",
            "d.pvec",
            "--source-to-geo",
            "1,0,0,0,1,0",
        ],
    ] {
        let out = run(&args);
        assert_eq!(code(&out), 1, "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("pvec-raster"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

#[cfg(not(feature = "pvec-raster"))]
#[test]
fn features_does_not_list_it_without_the_feature() {
    let out = run(&["features"]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("pvec-raster"));
}

#[cfg(feature = "pvec-raster")]
#[test]
fn features_lists_it_with_the_feature() {
    let out = run(&["features"]);
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l == "pvec-raster")
    );
}
