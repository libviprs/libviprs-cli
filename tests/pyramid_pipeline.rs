//! End-to-end cells for the `viprs pyramid` pipeline controls and
//! `viprs verify` (#66), driving the real binary.
//!
//! The wider suite lives in libviprs-tests (`tests/cli_pyramid_pipeline.rs`).
//! These are the cells for the refusals, renames and exit codes this crate
//! decides on its own, so a change to any of them goes red here, next to the
//! code, rather than in another repository after the pin moves.
//!
//! The inputs are hand-written PPMs, which the core decodes in every build,
//! so the cells need no pdfium and no image crate.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A temp directory of this cell's own (cargo runs cells as threads of one
/// process, so the pid alone would be shared).
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-pipe-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

/// Write a binary PPM whose pixel at `(x, y)` is `pixel(x, y)`.
fn write_ppm(path: &Path, width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) {
    let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
    bytes.reserve((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&pixel(x, y));
        }
    }
    std::fs::write(path, bytes).expect("the PPM must be writable");
}

/// 256x128: a gradient on the left half, flat black on the right, so at a
/// 64-pixel tile size the right half of the top level is blank tiles.
fn half_blank(dir: &Path) -> PathBuf {
    let path = dir.join("half.ppm");
    write_ppm(&path, 256, 128, |x, y| {
        if x < 128 {
            [x as u8, y as u8, (x ^ y) as u8]
        } else {
            [0, 0, 0]
        }
    });
    path
}

/// 200x100 of gradient, which does not fill a 64-pixel grid, so `--centre`
/// moves every pixel.
fn off_grid(dir: &Path) -> PathBuf {
    let path = dir.join("off-grid.ppm");
    write_ppm(&path, 200, 100, |x, y| [x as u8, y as u8, (x + y) as u8]);
    path
}

fn s(p: &Path) -> &str {
    p.to_str().expect("temp paths are UTF-8")
}

/// Every file under `root` whose name ends in `ext`.
fn files_with(root: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(ext) {
                out.push(path);
            }
        }
    }
    out
}

fn ok(out: &Output, what: &str) {
    assert_eq!(code(out), 0, "{what} failed:\n{}", stderr(out));
}

// ---------------------------------------------------------------------------
// --drop-blanks (was --skip-blanks)
// ---------------------------------------------------------------------------

#[test]
fn drop_blanks_leaves_blank_tiles_out_and_conflicts_with_skip_blank() {
    let dir = unique_dir("drop-blanks");
    let input = half_blank(&dir);
    let full = dir.join("full");
    let dropped = dir.join("dropped");
    let base = |out: &Path| {
        vec![
            "pyramid".to_string(),
            s(&input).to_string(),
            s(out).to_string(),
            "--storage".into(),
            "directory".into(),
            "--tile-size".into(),
            "64".into(),
        ]
    };

    let args = base(&full);
    ok(
        &run(&args.iter().map(String::as_str).collect::<Vec<_>>()),
        "the plain run",
    );
    let mut args = base(&dropped);
    args.push("--drop-blanks".into());
    ok(
        &run(&args.iter().map(String::as_str).collect::<Vec<_>>()),
        "the --drop-blanks run",
    );
    let (all, kept) = (
        files_with(&full, ".png").len(),
        files_with(&dropped, ".png").len(),
    );
    assert!(
        kept < all,
        "--drop-blanks kept {kept} of {all} tiles, so it dropped nothing"
    );

    // One letter apart and opposite in effect, so they cannot both be given.
    let mut args = base(&dir.join("both"));
    args.extend(["--drop-blanks".into(), "--skip-blank".into()]);
    let both = run(&args.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(code(&both), 2, "{}", stderr(&both));
    assert!(stderr(&both).contains("--skip-blank"), "{}", stderr(&both));

    // The old spelling is gone rather than kept as a near-twin.
    let mut args = base(&dir.join("old"));
    args.push("--skip-blanks".into());
    let old = run(&args.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(code(&old), 2, "{}", stderr(&old));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --retries / --skip-failed
// ---------------------------------------------------------------------------

/// A pyramid into the stub object store with its root pointed at a regular
/// file, so every tile write fails.
#[cfg(any(feature = "s3", feature = "object-store-sink"))]
fn failing_store_run(dir: &Path, extra: &[&str]) -> Output {
    let input = off_grid(dir);
    let root = dir.join("not-a-directory");
    std::fs::write(&root, b"a file where the store wants a directory").unwrap();
    let mut args = vec![
        "pyramid",
        s(&input),
        "--sink",
        "s3://bucket/run",
        "--object-store-root",
        s(&root),
        "--tile-size",
        "64",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn retries_alone_retries_then_fails_the_run() {
    let dir = unique_dir("retries-fail");
    let out = failing_store_run(&dir, &["--retries", "1", "--retry-backoff-ms", "1"]);
    assert_eq!(
        code(&out),
        1,
        "every tile write failed and --retries alone must not turn that into a \
         successful run with holes:\n{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn skip_failed_skips_the_tiles_and_still_exits_1() {
    let dir = unique_dir("skip-failed");
    let out = failing_store_run(
        &dir,
        &["--retries", "1", "--retry-backoff-ms", "1", "--skip-failed"],
    );
    let err = stderr(&out);
    assert_eq!(code(&out), 1, "{err}");
    assert!(
        err.contains("skipped"),
        "the run must say tiles were skipped rather than fail some other way:\n{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --events json and features --json carry a schema version
// ---------------------------------------------------------------------------

#[test]
fn events_json_lines_carry_a_schema_version_and_fixed_names() {
    let dir = unique_dir("events-v");
    let input = off_grid(&dir);
    let tree = dir.join("tree");
    let out = run(&[
        "pyramid",
        s(&input),
        s(&tree),
        "--storage",
        "directory",
        "--tile-size",
        "64",
        "--events",
        "json",
    ]);
    ok(&out, "the --events json run");
    let lines: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l:?}: {e}")))
        .collect();
    assert!(!lines.is_empty(), "no events at all");
    for line in &lines {
        assert_eq!(line["v"], 1, "every event line carries \"v\":1: {line}");
        assert!(line["event"].is_string(), "{line}");
    }
    let names: Vec<&str> = lines.iter().filter_map(|l| l["event"].as_str()).collect();
    for expected in ["level_started", "tile_completed", "level_completed"] {
        assert!(names.contains(&expected), "no {expected} in {names:?}");
    }
    assert_eq!(names.last(), Some(&"summary"), "{names:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn features_json_carries_a_schema_version() {
    let out = run(&["features", "--json"]);
    ok(&out, "features --json");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(json["v"], 1, "{json}");
    assert!(json["features"].is_array(), "{json}");
}

/// The subscriber writes to stderr, so `--events json` on stdout stays one
/// JSON object per line. The core emits spans and no events today, so this
/// cell guards the writer rather than catching anything printed now.
#[cfg(feature = "tracing")]
#[test]
fn trace_output_stays_off_the_events_stream() {
    let dir = unique_dir("trace-stderr");
    let input = off_grid(&dir);
    let tree = dir.join("tree");
    let out = run(&[
        "pyramid",
        s(&input),
        s(&tree),
        "--storage",
        "directory",
        "--events",
        "json",
        "--trace-level",
        "trace",
    ]);
    ok(&out, "the traced run");
    for line in stdout(&out).lines() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "stdout carries something that is not an event: {line:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// viprs verify: centre and skip_blanks from the pyramid, the flags as
// overrides for older ones, and the re-render it still cannot do (#85)
// ---------------------------------------------------------------------------

/// A centred raw tree, so a re-render compares bytes.
fn centred_tree(dir: &Path, input: &Path) -> PathBuf {
    let tree = dir.join("centred");
    ok(
        &run(&[
            "pyramid",
            s(input),
            s(&tree),
            "--storage",
            "directory",
            "--format",
            "raw",
            "--tile-size",
            "64",
            "--centre",
            "--checksum",
        ]),
        "the centred tree",
    );
    tree
}

/// A `--drop-blanks` tree with a manifest.
fn dropped_tree(dir: &Path, input: &Path) -> PathBuf {
    let tree = dir.join("tree");
    ok(
        &run(&[
            "pyramid",
            s(input),
            s(&tree),
            "--storage",
            "directory",
            "--tile-size",
            "64",
            "--drop-blanks",
            "--checksum",
        ]),
        "the --drop-blanks tree",
    );
    tree
}

/// Rewrite a tree's manifest (inside it, and the sibling copy if there is
/// one) the way a writer from before `centre` and `skip_blanks` existed left
/// it: neither key in the generation block.
fn forget_generation_flags(tree: &Path) {
    let inside = tree.join("manifest.json");
    let mut sibling = tree.to_path_buf().into_os_string();
    sibling.push(".manifest.json");
    for path in [inside, PathBuf::from(sibling)] {
        if !path.exists() {
            continue;
        }
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let generation = manifest["generation"]
            .as_object_mut()
            .expect("the manifest has a generation block");
        generation.remove("centre");
        generation.remove("skip_blanks");
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
}

#[test]
fn verify_reads_centre_from_a_centred_tree() {
    let dir = unique_dir("verify-centre");
    let input = off_grid(&dir);
    let centred = centred_tree(&dir, &input);

    // The manifest says the plan was centred, so verify needs no flag.
    let plain = run(&["verify", s(&centred)]);
    assert_eq!(code(&plain), 0, "{}", stderr(&plain));

    // The core's re-render lays a source out on a centred grid now
    // (libviprs#1163), so --source checks a centred tree byte for byte,
    // with or without the flag that repeats what the manifest says.
    let rerender = run(&["verify", s(&centred), "--source", s(&input)]);
    assert_eq!(code(&rerender), 0, "{}", stderr(&rerender));
    assert!(
        stdout(&rerender).contains("a re-render of the source"),
        "{}",
        stdout(&rerender)
    );
    let told = run(&["verify", s(&centred), "--centre", "--source", s(&input)]);
    assert_eq!(code(&told), 0, "{}", stderr(&told));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_takes_centre_for_a_tree_that_does_not_record_it() {
    let dir = unique_dir("verify-centre-old");
    let input = off_grid(&dir);
    let centred = centred_tree(&dir, &input);
    forget_generation_flags(&centred);

    // An older manifest reads as uncentred, so the flag is how to say it.
    let told = run(&["verify", s(&centred), "--centre", "--source", s(&input)]);
    assert_eq!(code(&told), 0, "{}", stderr(&told));

    // Without it the re-render is laid out on the uncentred grid and the
    // failure names the flag.
    let untold = run(&["verify", s(&centred), "--source", s(&input)]);
    assert_eq!(code(&untold), 1, "{}", stderr(&untold));
    assert!(stderr(&untold).contains("--centre"), "{}", stderr(&untold));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_reads_skip_blanks_from_a_tree() {
    let dir = unique_dir("verify-drop-tree");
    let input = half_blank(&dir);
    let tree = dropped_tree(&dir, &input);

    let plain = run(&["verify", s(&tree)]);
    assert_eq!(code(&plain), 0, "{}", stderr(&plain));
    assert!(
        stdout(&plain).contains("blank tiles dropped"),
        "{}",
        stdout(&plain)
    );
    let told = run(&["verify", s(&tree), "--drop-blanks"]);
    assert_eq!(code(&told), 0, "{}", stderr(&told));

    // The re-render accepts a dropped blank now (libviprs#1174), so --source
    // checks a --drop-blanks tree too, with or without the flag.
    for told in [&[][..], &["--drop-blanks"][..]] {
        let mut args = vec!["verify", s(&tree), "--source", s(&input)];
        args.extend_from_slice(told);
        let rerender = run(&args);
        assert_eq!(code(&rerender), 0, "{told:?}: {}", stderr(&rerender));
        assert!(
            stdout(&rerender).contains("a re-render of the source"),
            "{told:?}: {}",
            stdout(&rerender)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A raw `--drop-blanks` tree, so the re-render compares bytes, and a kept
/// tile that goes missing is still named with `--source`.
#[test]
fn verify_re_renders_a_raw_drop_blanks_tree_and_still_catches_a_lost_tile() {
    let dir = unique_dir("verify-drop-raw");
    let input = half_blank(&dir);
    let tree = dir.join("raw");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&tree),
            "--storage",
            "directory",
            "--format",
            "raw",
            "--tile-size",
            "64",
            "--drop-blanks",
            "--checksum",
        ]),
        "the raw --drop-blanks tree",
    );
    let rerender = run(&["verify", s(&tree), "--source", s(&input)]);
    assert_eq!(code(&rerender), 0, "{}", stderr(&rerender));

    // The top level's top-left tile is gradient, so the run kept it.
    let top = std::fs::read_dir(&tree)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .max()
        .expect("the tree has numbered level directories");
    let kept = tree.join(top.to_string()).join("0_0.raw");
    std::fs::remove_file(&kept).expect("the top-left tile was kept");
    let lost = run(&["verify", s(&tree), "--source", s(&input)]);
    assert_eq!(code(&lost), 1, "{}", stderr(&lost));
    assert!(
        stderr(&lost).contains(&format!("{top}/0_0.raw")),
        "{}",
        stderr(&lost)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_takes_drop_blanks_for_a_tree_that_does_not_record_it() {
    let dir = unique_dir("verify-drop-tree-old");
    let input = half_blank(&dir);
    let tree = dropped_tree(&dir, &input);
    forget_generation_flags(&tree);

    let told = run(&["verify", s(&tree), "--drop-blanks"]);
    assert_eq!(code(&told), 0, "{}", stderr(&told));
    let rerender = run(&["verify", s(&tree), "--drop-blanks", "--source", s(&input)]);
    assert_eq!(code(&rerender), 0, "{}", stderr(&rerender));

    let untold = run(&["verify", s(&tree)]);
    assert_eq!(code(&untold), 1, "{}", stderr(&untold));
    assert!(
        stderr(&untold).contains("--drop-blanks"),
        "a missing tile on a tree that may have dropped blanks must name the flag:\n{}",
        stderr(&untold)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_reads_skip_blanks_from_an_archive() {
    let dir = unique_dir("verify-drop-archive");
    let input = half_blank(&dir);
    let archive = dir.join("out.pmtiles");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&archive),
            "--tile-size",
            "64",
            "--drop-blanks",
        ]),
        "the --drop-blanks archive",
    );

    let plain = run(&["verify", s(&archive)]);
    assert_eq!(code(&plain), 0, "{}", stderr(&plain));
    assert!(
        stdout(&plain).contains("blank tiles dropped"),
        "{}",
        stdout(&plain)
    );
    let told = run(&["verify", s(&archive), "--drop-blanks"]);
    assert_eq!(code(&told), 0, "{}", stderr(&told));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_reads_centre_from_an_archive() {
    let dir = unique_dir("verify-centre-archive");
    let input = off_grid(&dir);
    let archive = dir.join("out.pmtiles");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&archive),
            "--tile-size",
            "64",
            "--centre",
        ]),
        "the centred archive",
    );

    let plain = run(&["verify", s(&archive)]);
    assert_eq!(code(&plain), 0, "{}", stderr(&plain));
    let told = run(&["verify", s(&archive), "--centre"]);
    assert_eq!(code(&told), 0, "{}", stderr(&told));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_refuses_the_re_renders_it_cannot_do_and_no_longer_drop_blanks() {
    let dir = unique_dir("verify-refuse");
    let input = off_grid(&dir);
    let tree = dir.join("tree");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&tree),
            "--storage",
            "directory",
            "--tile-size",
            "64",
            "--checksum",
        ]),
        "the tree",
    );

    // --drop-blanks with --source is no longer refused (libviprs#1174): on a
    // tree that dropped nothing it is the plain re-render.
    let dropped = run(&["verify", s(&tree), "--drop-blanks", "--source", s(&input)]);
    assert_eq!(code(&dropped), 0, "{}", stderr(&dropped));

    // A PDF needs the page, DPI and render mode the pyramid used, and verify
    // has none of them.
    let pdf = dir.join("in.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 not read").unwrap();
    let from_pdf = run(&["verify", s(&tree), "--source", s(&pdf)]);
    assert_eq!(code(&from_pdf), 2, "{}", stderr(&from_pdf));
    assert!(stderr(&from_pdf).contains("PDF"), "{}", stderr(&from_pdf));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plain tree written from a file, which is what #88 started handing the
/// core a source digest for. The digest is folded into the plan hash the
/// checkpoint records, so the re-render has to fold in the same one
/// (libviprs-cli#99).
fn plain_tree(dir: &Path, input: &Path) -> PathBuf {
    let tree = dir.join("plain");
    ok(
        &run(&[
            "pyramid",
            s(input),
            s(&tree),
            "--storage",
            "directory",
            "--format",
            "raw",
            "--tile-size",
            "64",
            "--checksum",
        ]),
        "the plain tree",
    );
    tree
}

#[test]
fn verify_source_passes_a_plain_tree_written_from_a_file_99() {
    let dir = unique_dir("verify-99-file");
    let input = off_grid(&dir);
    let tree = plain_tree(&dir, &input);
    let out = run(&["verify", s(&tree), "--source", s(&input)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("a re-render of the source"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A tree written from stdin carries no digest, so the re-render must not
/// insist on one either.
#[test]
fn verify_source_passes_a_tree_written_from_stdin_99() {
    let dir = unique_dir("verify-99-stdin");
    let input = off_grid(&dir);
    let tree = dir.join("from-stdin");
    let out = viprs()
        .args([
            "pyramid",
            "-",
            s(&tree),
            "--storage",
            "directory",
            "--format",
            "raw",
            "--tile-size",
            "64",
            "--checksum",
        ])
        .stdin(std::fs::File::open(&input).unwrap())
        .output()
        .unwrap();
    ok(&out, "the stdin tree");
    let out = run(&["verify", s(&tree), "--source", s(&input)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A same-sized but different image is still caught, and the message says
/// the source differs instead of only blaming --centre.
#[test]
fn verify_source_names_a_different_source_99() {
    let dir = unique_dir("verify-99-other");
    let input = off_grid(&dir);
    let tree = plain_tree(&dir, &input);
    let other = dir.join("other.ppm");
    write_ppm(&other, 200, 100, |x, y| [y as u8, x as u8, 7]);
    let out = run(&["verify", s(&tree), "--source", s(&other)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("made from a different"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--source` decodes through the same input path as every other command,
/// so a format this build left out is refused naming its feature.
#[cfg(not(feature = "svg"))]
#[test]
fn verify_source_goes_through_the_single_input_path() {
    let dir = unique_dir("verify-svg");
    let input = off_grid(&dir);
    let tree = dir.join("tree");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&tree),
            "--storage",
            "directory",
            "--checksum",
        ]),
        "the tree",
    );
    let svg = dir.join("in.svg");
    std::fs::write(
        &svg,
        "<svg xmlns='http://www.w3.org/2000/svg' width='200' height='100'/>",
    )
    .unwrap();
    let out = run(&["verify", s(&tree), "--source", s(&svg)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("--features svg"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A manifest that claims a huge source makes every planned tile "missing".
/// The report stops at a cap instead of printing (and holding) all of them.
#[test]
fn verify_caps_the_problems_it_lists() {
    let dir = unique_dir("verify-cap");
    let input = off_grid(&dir);
    let tree = dir.join("tree");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&tree),
            "--storage",
            "directory",
            "--checksum",
        ]),
        "the tree",
    );
    let manifest_path = tree.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["source"]["width"] = 100_000.into();
    manifest["source"]["height"] = 100_000.into();
    let text = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(&manifest_path, &text).unwrap();
    let mut sibling = tree.clone().into_os_string();
    sibling.push(".manifest.json");
    if Path::new(&sibling).exists() {
        std::fs::write(&sibling, &text).unwrap();
    }

    let out = run(&["verify", s(&tree)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let lines = stderr(&out).lines().count();
    assert!(
        lines <= 60,
        "verify printed {lines} lines for one bad manifest"
    );
    assert!(stderr(&out).contains("more"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The paths that used to stay on the old driver
// ---------------------------------------------------------------------------

#[test]
fn memory_budget_into_an_archive_says_it_does_not_apply() {
    let dir = unique_dir("budget-archive");
    let input = off_grid(&dir);
    let archive = dir.join("out.pmtiles");
    let out = run(&["pyramid", s(&input), s(&archive), "--memory-budget", "64"]);
    ok(&out, "--memory-budget into an archive");
    assert!(archive.is_file(), "no archive was written");
    assert!(
        stderr(&out).contains("--memory-budget"),
        "a budget the archive path ignores must be named, not dropped:\n{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn memory_budget_with_an_s3_sink_runs_through_the_pipeline() {
    let dir = unique_dir("budget-s3");
    let input = off_grid(&dir);
    let root = dir.join("store");
    let out = run(&[
        "pyramid",
        s(&input),
        "--sink",
        "s3://bucket/run",
        "--object-store-root",
        s(&root),
        "--memory-budget",
        "64",
    ]);
    let err = stderr(&out);
    assert!(!err.contains("not yet fully wired"), "{err}");
    assert_eq!(code(&out), 0, "{err}");
    assert!(
        !files_with(&root.join("bucket"), ".png").is_empty(),
        "nothing landed in the store"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_a_memory_budget_run_with_130() {
    let dir = unique_dir("budget-sigint");
    let input = dir.join("big.ppm");
    write_ppm(&input, 4096, 4096, |x, y| {
        [(x * 7) as u8, (y * 13) as u8, (x ^ y) as u8]
    });
    let tree = dir.join("tree");
    let mut child = viprs()
        .args([
            "pyramid",
            s(&input),
            s(&tree),
            "--storage",
            "directory",
            "--tile-size",
            "32",
            "--memory-budget",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    // Interrupt once tiles are landing, so the run is mid-flight.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "no tile appeared within 120 s");
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the run finished ({status}) before it could be interrupted");
        }
        if !files_with(&tree, ".png").is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let killed = Command::new("sh")
        .args(["-c", &format!("kill -INT {}", child.id())])
        .status()
        .unwrap();
    assert!(killed.success());

    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the run did not stop within 60 s of Ctrl-C");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        status.code(),
        Some(130),
        "a --memory-budget run must stop at a tile boundary and exit 130, got {status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The stub object store stays out of sight
// ---------------------------------------------------------------------------

#[test]
fn the_stub_store_flag_is_not_advertised() {
    let out = run(&["pyramid", "--help"]);
    ok(&out, "pyramid --help");
    assert!(
        !stdout(&out).contains("--object-store-root"),
        "the local stub store is a test seam, not user surface"
    );
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn an_s3_bucket_that_climbs_out_of_the_store_is_refused() {
    let dir = unique_dir("bucket-dotdot");
    let input = off_grid(&dir);
    let root = dir.join("store");
    std::fs::create_dir_all(&root).unwrap();
    let out = run(&[
        "pyramid",
        s(&input),
        "--sink",
        "s3://../escaped",
        "--object-store-root",
        s(&root),
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        files_with(&dir, ".png").is_empty(),
        "a refused run wrote tiles"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The s3:// sink writes through the core's DirectoryObjectStore (#87)
// ---------------------------------------------------------------------------

/// A bucket name the core's directory store refuses is still refused as a
/// usage mistake, before the input is read: a backslash is one plain name to
/// a Unix path parser, so only the store's own check catches it.
#[cfg(all(unix, any(feature = "s3", feature = "object-store-sink")))]
#[test]
fn an_s3_bucket_the_directory_store_refuses_is_a_usage_error() {
    let dir = unique_dir("bucket-backslash");
    let input = off_grid(&dir);
    let root = dir.join("store");
    let out = run(&[
        "pyramid",
        s(&input),
        "--sink",
        "s3://a\\b/run",
        "--object-store-root",
        s(&root),
        "--tile-size",
        "64",
    ]);
    let err = stderr(&out);
    assert_eq!(code(&out), 2, "{err}");
    assert!(err.contains("bucket"), "{err}");
    assert!(
        files_with(&dir, ".png").is_empty(),
        "a refused run wrote tiles"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A symlink planted under the store root is not a way out of it: the core's
/// store refuses every key that crosses one, so the run fails (exit 1) and
/// nothing lands where the link points.
#[cfg(all(unix, any(feature = "s3", feature = "object-store-sink")))]
#[test]
fn the_s3_sink_does_not_follow_a_symlink_under_the_store() {
    let dir = unique_dir("store-symlink");
    let input = off_grid(&dir);
    let root = dir.join("store");
    let outside = dir.join("outside");
    std::fs::create_dir_all(root.join("tiles")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("tiles/run-1")).unwrap();
    let out = run(&[
        "pyramid",
        s(&input),
        "--sink",
        "s3://tiles/run-1",
        "--object-store-root",
        s(&root),
        "--tile-size",
        "64",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        files_with(&outside, ".png").is_empty(),
        "the store followed a symlink out of its root"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The store holds exactly the tiles a directory run writes, byte for byte,
/// under `ROOT/bucket/prefix/<stem>_files`, and no staging file is left over.
#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn the_s3_sink_stores_the_tree_run_bytes() {
    let dir = unique_dir("store-bytes");
    let input = off_grid(&dir);
    let root = dir.join("store");
    let tree = dir.join("tree");
    ok(
        &run(&[
            "pyramid",
            s(&input),
            "--sink",
            "s3://tiles/run-1",
            "--object-store-root",
            s(&root),
            "--tile-size",
            "64",
        ]),
        "the s3 run",
    );
    ok(
        &run(&[
            "pyramid",
            s(&input),
            s(&tree),
            "--storage",
            "directory",
            "--tile-size",
            "64",
        ]),
        "the tree run",
    );
    let tiles = |base: &Path| {
        let mut out: Vec<(PathBuf, Vec<u8>)> = files_with(base, ".png")
            .into_iter()
            .map(|p| {
                (
                    p.strip_prefix(base).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    };
    let stored = tiles(&root.join("tiles/run-1/off-grid_files"));
    assert!(!stored.is_empty(), "nothing landed in the store");
    assert_eq!(
        stored,
        tiles(&tree),
        "the stored tiles differ from the tree run's"
    );
    assert!(
        files_with(&root, "part").is_empty(),
        "a staging file was left in the store"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stub store is the core's now; the CLI keeps no copy of its own.
#[test]
fn the_cli_keeps_no_private_object_store() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline.rs");
    let source = std::fs::read_to_string(&path).unwrap();
    assert!(
        !source.contains("LocalStore") && !source.contains("impl ObjectStore for"),
        "src/pipeline.rs still carries its own ObjectStore"
    );
    assert!(
        source.contains("DirectoryObjectStore::for_bucket"),
        "src/pipeline.rs does not build the core's DirectoryObjectStore"
    );
}

// ---------------------------------------------------------------------------
// --manifest-source-hash records the source file's bytes
// ---------------------------------------------------------------------------

#[test]
fn manifest_source_hash_hashes_the_file_bytes() {
    let dir = unique_dir("source-hash");
    let ppm = off_grid(&dir);
    // Same pixels, different bytes: one PNG stored, one compressed.
    let a = dir.join("a.png");
    let b = dir.join("b.png");
    ok(
        &run(&["pngsave", s(&ppm), s(&a), "--compression", "0"]),
        "pngsave 0",
    );
    ok(
        &run(&["pngsave", s(&ppm), s(&b), "--compression", "9"]),
        "pngsave 9",
    );
    assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

    let hash_of = |input: &Path, tag: &str| -> String {
        let tree = dir.join(tag);
        ok(
            &run(&[
                "pyramid",
                s(input),
                s(&tree),
                "--storage",
                "directory",
                "--manifest-source-hash",
            ]),
            tag,
        );
        let m: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tree.join("manifest.json")).unwrap()).unwrap();
        m["source"]["bytes_hash"]
            .as_str()
            .unwrap_or_else(|| panic!("no source.bytes_hash in {m}"))
            .to_string()
    };
    assert_ne!(
        hash_of(&a, "tree-a"),
        hash_of(&b, "tree-b"),
        "two files with the same pixels got one hash, so it is not a hash of the file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manifest_source_hash_refuses_stdin() {
    let dir = unique_dir("source-hash-stdin");
    let tree = dir.join("tree");
    let out = viprs()
        .args([
            "pyramid",
            "-",
            s(&tree),
            "--storage",
            "directory",
            "--manifest-source-hash",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("stdin"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --resume refuses a checkpoint made from a different source (#86)
// ---------------------------------------------------------------------------

/// 1024x1024, so a 32-pixel tile gives well over a thousand tiles to stop
/// part way through. `flip` inverts every pixel: the same size, the same
/// header, the same byte count, and different bytes.
fn square(path: &Path, flip: bool) {
    write_ppm(path, 1024, 1024, |x, y| {
        let p = [(x * 7) as u8, (y * 13) as u8, (x ^ y) as u8];
        if flip { p.map(|c| !c) } else { p }
    });
}

/// Start a one-worker, checkpoint-every-tile tree run of `input` into `tree`,
/// Ctrl-C it once `after` tiles are reported done, and require that it stopped
/// with 130 and left a checkpoint for `--resume` to find.
#[cfg(unix)]
fn interrupt_tree_run(input: &Path, tree: &Path, after: usize) {
    use std::io::{BufRead as _, BufReader};
    use std::sync::mpsc::{self, RecvTimeoutError};

    let mut child = viprs()
        .args([
            "pyramid",
            s(input),
            s(tree),
            "--storage",
            "directory",
            "--tile-size",
            "32",
            "--concurrency",
            "1",
            "--checkpoint-every",
            "1",
            "--events",
            "json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let pid = child.id().to_string();
    let lines = BufReader::new(child.stdout.take().expect("piped stdout"));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in lines.lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(300);
    let (mut done, mut signalled) = (0usize, false);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                done += usize::from(line.contains("\"tile_completed\""));
                if done >= after && !signalled {
                    let kill = Command::new("kill").args(["-INT", &pid]).status().unwrap();
                    assert!(kill.success(), "kill -INT {pid} failed");
                    signalled = true;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                panic!("the run was still going after 300 s ({done} tiles reported)");
            }
        }
    }
    let status = child.wait().unwrap();
    assert!(
        signalled,
        "the run finished before {after} tiles, so it was never interrupted"
    );
    assert_eq!(
        status.code(),
        Some(130),
        "a Ctrl-C'd run must exit 130, got {status}"
    );
    assert!(
        files_with(tree, ".libviprs-job.json")
            .iter()
            .any(|p| p.is_file()),
        "the interrupted run left no checkpoint, so there is nothing to resume"
    );
}

/// `--resume` against a different image of the same size must be refused by
/// the plan-hash check, before it touches the tree, rather than finish the
/// job with tiles from two images. The digest of the source file is what
/// tells them apart: the geometry, the flags and the byte count all match.
/// The control is the resume with the image the job was started from, which
/// must still go through.
#[cfg(unix)]
#[test]
fn resume_refuses_a_different_source_of_the_same_size() {
    let dir = unique_dir("resume-other-source");
    let input = dir.join("input.ppm");
    let other = dir.join("other.ppm");
    square(&input, false);
    square(&other, true);
    assert_eq!(
        std::fs::metadata(&input).unwrap().len(),
        std::fs::metadata(&other).unwrap().len()
    );
    let tree = dir.join("tree");
    interrupt_tree_run(&input, &tree, 20);
    let tiles_before = files_with(&tree, ".png").len();

    let resume = |source: &Path| {
        run(&[
            "pyramid",
            s(source),
            s(&tree),
            "--storage",
            "directory",
            "--tile-size",
            "32",
            "--concurrency",
            "1",
            "--resume",
        ])
    };
    let refused = resume(&other);
    let err = stderr(&refused);
    assert_eq!(
        code(&refused),
        1,
        "--resume with a different source of the same size must fail, not stitch two \
         images into one tree:\n{err}"
    );
    assert!(err.contains("plan hash mismatch"), "{err}");
    assert!(
        err.contains("input"),
        "the hint must say the input has to be the one the job started from:\n{err}"
    );
    assert_eq!(
        files_with(&tree, ".png").len(),
        tiles_before,
        "a refused resume wrote tiles"
    );

    ok(&resume(&input), "--resume with the job's own source");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The source digest is the core's to record now (libviprs#1164): the CLI
/// hands it to the run and the manifest builder, and no longer patches
/// `manifest.json` after the sink has written it.
#[test]
fn the_cli_does_not_patch_the_manifest_after_the_run() {
    let source = pipeline_rs();
    assert!(
        !source.contains("fn record_source_hash"),
        "src/pipeline.rs still rewrites the manifest's bytes_hash itself"
    );
    assert!(
        source.contains("with_source_content_hash"),
        "src/pipeline.rs does not hand the source digest to the run"
    );
}

/// The `src/pipeline.rs` this crate is built from.
fn pipeline_rs() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "src/pipeline.rs must be readable at {}: {e}",
            path.display()
        )
    })
}

// ---------------------------------------------------------------------------
// Memory: the estimate counts what the flags cost, and the dedupe window
// stops at what the plan can use
// ---------------------------------------------------------------------------

#[test]
fn ordered_emission_counts_the_level_rasters_in_the_memory_estimate() {
    let dir = unique_dir("ordered-estimate");
    let input = dir.join("big.ppm");
    // 2048x2048: the base estimate is 32.0 MB (source plus its copy at four
    // bytes a pixel), and holding every lower level as well adds a few more.
    write_ppm(&input, 2048, 2048, |x, y| [x as u8, y as u8, 0]);
    let plain = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("plain.pmtiles")),
        "--memory-limit",
        "33",
    ]);
    ok(&plain, "the plain run under --memory-limit 33");
    let ordered = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("ordered.pmtiles")),
        "--memory-limit",
        "33",
        "--ordered-emission",
    ]);
    assert_eq!(
        code(&ordered),
        1,
        "--ordered-emission holds every level at once and the estimate must say so:\n{}",
        stderr(&ordered)
    );
    assert!(
        stderr(&ordered).contains("--memory-limit"),
        "{}",
        stderr(&ordered)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dedupe_memory_bytes_beyond_the_plan_is_lowered_and_said() {
    let dir = unique_dir("dedupe-clamp");
    let input = off_grid(&dir);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("out.pmtiles")),
        "--dedupe-memory-bytes",
        "268435456",
    ]);
    ok(&out, "a huge --dedupe-memory-bytes");
    let err = stderr(&out);
    assert!(
        err.contains("--dedupe-memory-bytes") && err.contains("lowered"),
        "a window wider than the plan can use must be lowered, and the run must say so:\n{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
