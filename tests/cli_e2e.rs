//! End-to-end tests that drive the compiled `viprs` binary.
//!
//! Cargo exposes the built binary path through `CARGO_BIN_EXE_viprs` for
//! integration tests, so these exercise the real argument parsing, command
//! dispatch, and process exit codes without any extra test dependency.
//!
//! The cases here deliberately stay on code paths that never touch the native
//! pdfium runtime (numeric `plan`, missing-file `info`, `test-image` PNG
//! round-trip, clap usage errors), so they run anywhere the crate builds.

use std::path::PathBuf;
use std::process::{Command, Output};

/// Absolute path to the freshly built `viprs` binary under test.
fn viprs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
}

/// Run `viprs` with the given arguments and capture the result.
fn run(args: &[&str]) -> Output {
    viprs()
        .args(args)
        .output()
        .expect("the viprs binary must be spawnable")
}

/// Extract the numeric exit code, failing loudly on signal termination.
fn code(out: &Output) -> i32 {
    out.status
        .code()
        .expect("the process must exit normally rather than via a signal")
}

#[test]
fn plan_with_numeric_dimensions_succeeds() {
    // `plan W --height H` is a pure planning path: no image decode, no pdfium.
    let out = run(&["plan", "1024", "--height", "768"]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Levels:") && stdout.contains("Image: 1024x768"),
        "plan output must summarise the pyramid, got:\n{stdout}"
    );
}

#[test]
fn plan_numeric_width_without_height_exits_2() {
    // A numeric width with no --height is a missing argument, which the
    // exit-code contract calls a usage mistake (#81).
    let out = run(&["plan", "1024"]);
    assert_eq!(code(&out), 2);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--height is required"),
        "expected the missing-height diagnostic, got:\n{stderr}"
    );
}

#[test]
fn info_on_missing_file_exits_1() {
    // A non-existent input is a runtime error surfaced as exit 1.
    let out = run(&["info", "/no/such/file/definitely-missing.png"]);
    assert_eq!(code(&out), 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("File not found"),
        "expected the not-found diagnostic, got:\n{stderr}"
    );
}

#[test]
fn no_subcommand_is_a_usage_error_exit_2() {
    // clap reports a missing subcommand as a usage error, which maps to exit 2.
    let out = run(&[]);
    assert_eq!(code(&out), 2);
}

#[test]
fn unknown_flag_is_a_usage_error_exit_2() {
    let out = run(&["plan", "1024", "--height", "768", "--totally-unknown"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn help_exits_0() {
    let out = run(&["--help"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("viprs"), "help must name the binary");
}

#[test]
fn test_image_then_info_round_trip() {
    // `test-image` generates a PNG with no pdfium involvement, and `info` on a
    // PNG decodes via the image path (also pdfium-free), so this drives two
    // real subcommands end to end and asserts both succeed.
    let dir = std::env::temp_dir().join(format!("viprs-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    let png: PathBuf = dir.join("gen.png");

    let generated = run(&[
        "test-image",
        png.to_str().unwrap(),
        "--width",
        "64",
        "--height",
        "48",
    ]);
    assert_eq!(
        code(&generated),
        0,
        "test-image stderr:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert!(png.exists(), "test-image must write the PNG");

    let info = run(&["info", png.to_str().unwrap()]);
    assert_eq!(
        code(&info),
        0,
        "info stderr:\n{}",
        String::from_utf8_lossy(&info.stderr)
    );
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(
        stdout.contains("64x48"),
        "info must report the generated dimensions, got:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// `viprs features` and the missing-feature refusal (#64)
// ---------------------------------------------------------------------------

/// What `viprs features` must print for the configuration this test binary was
/// compiled in. Integration tests build with the package's features, so
/// `cfg!` here answers for the binary under test too.
fn expected_features() -> Vec<&'static str> {
    [
        ("avif", cfg!(feature = "avif")),
        ("jp2k", cfg!(feature = "jp2k")),
        ("jxl", cfg!(feature = "jxl")),
        ("object-store-sink", cfg!(feature = "object-store-sink")),
        ("packfile", cfg!(feature = "packfile")),
        ("pdfium", cfg!(feature = "pdfium")),
        ("pdfium-static", cfg!(feature = "pdfium-static")),
        ("s3", cfg!(feature = "s3")),
        ("svg", cfg!(feature = "svg")),
        ("tracing", cfg!(feature = "tracing")),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(name, _)| name)
    .collect()
}

#[test]
fn features_lists_what_this_build_was_compiled_with() {
    let out = run(&["features"]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let listed: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(listed, expected_features());

    let out = run(&["features", "--json"]);
    assert_eq!(code(&out), 0);
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("features --json prints JSON");
    let listed: Vec<&str> = json["features"]
        .as_array()
        .expect("a `features` array")
        .iter()
        .map(|v| v.as_str().expect("feature names are strings"))
        .collect();
    assert_eq!(listed, expected_features());
}

/// Without `svg`, an SVG input is refused naming the feature, through both the
/// built-in `info` and the op harness. Before #64 it never reached the SVG
/// decoder at all and failed as an unrecognised format.
#[cfg(not(feature = "svg"))]
#[test]
fn without_svg_an_svg_input_is_refused_naming_the_feature() {
    let dir = unique_dir("svg-refusal");
    let svg = dir.join("in.svg");
    std::fs::write(
        &svg,
        "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>",
    )
    .unwrap();
    let png = dir.join("out.png");
    for args in [
        vec!["info", svg.to_str().unwrap()],
        vec!["copy", svg.to_str().unwrap(), png.to_str().unwrap()],
    ] {
        let out = run(&args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(code(&out), 1, "{args:?} stderr:\n{stderr}");
        assert!(
            stderr.contains("`svg`") && stderr.contains("--features svg"),
            "{args:?} must name the feature to rebuild with, got:\n{stderr}"
        );
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
    assert!(!png.exists(), "a refused load must write nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "svg")]
#[test]
fn with_svg_an_svg_input_decodes() {
    let dir = unique_dir("svg-decode");
    let svg = dir.join("in.svg");
    std::fs::write(
        &svg,
        "<svg xmlns='http://www.w3.org/2000/svg' width='5' height='3'><rect width='5' height='3' fill='red'/></svg>",
    )
    .unwrap();
    let out = run(&["info", svg.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Dimensions: 5x3"), "got:\n{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `viprs features --help` says what the list is and what it is not. It
/// reports this crate's cargo features, not capabilities, and an `s3` build
/// has the core's object-store sink without listing `object-store-sink`.
#[test]
fn features_help_says_it_lists_cargo_features_not_capabilities() {
    let out = run(&["features", "--help"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("cargo features") && stdout.contains("not a list of capabilities"),
        "the help must say what the list is, got:\n{stdout}"
    );
    assert!(
        stdout.contains("object-store-sink"),
        "the help must say an `s3` build has the object-store sink unlisted, got:\n{stdout}"
    );
}

// The exit-code contract (README, "Exit codes"): a usage mistake is 2, an
// operational failure is 1, and a feature this build left out is an
// operational failure, whichever flag reached it. These three used to exit 2
// while a missing codec exited 1.

#[cfg(not(feature = "tracing"))]
#[test]
fn trace_level_without_the_tracing_feature_exits_1() {
    let dir = unique_dir("no-tracing");
    let png = make_input(&dir, 64, 64);
    let tree = dir.join("tree");
    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        tree.to_str().unwrap(),
        "--storage",
        "directory",
        "--trace-level",
        "info",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code(&out), 1, "stderr:\n{stderr}");
    assert!(stderr.contains("--features tracing"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(feature = "packfile"))]
#[test]
fn packfile_sink_without_the_packfile_feature_exits_1() {
    let dir = unique_dir("no-packfile");
    let png = make_input(&dir, 64, 64);
    let tar = dir.join("out.tar");
    let sink = format!("packfile://{}", tar.display());
    let out = run(&["pyramid", png.to_str().unwrap(), "--sink", &sink]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code(&out), 1, "stderr:\n{stderr}");
    assert!(stderr.contains("--features packfile"), "{stderr}");
    assert!(!tar.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(feature = "s3"))]
#[test]
fn s3_sink_without_the_s3_feature_exits_1() {
    let dir = unique_dir("no-s3");
    let png = make_input(&dir, 64, 64);
    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        "--sink",
        "s3://bucket/prefix",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code(&out), 1, "stderr:\n{stderr}");
    assert!(stderr.contains("--features s3"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A required mode flag left out is a usage mistake like any other missing
/// argument (README, "Exit codes"): `webpsave` writes lossless only and
/// insists on being told so.
#[test]
fn a_missing_required_mode_flag_exits_2_and_writes_nothing() {
    let dir = unique_dir("webpsave-no-lossless");
    let png = make_input(&dir, 16, 16);
    let webp = dir.join("out.webp");
    let out = run(&["webpsave", png.to_str().unwrap(), webp.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code(&out), 2, "stderr:\n{stderr}");
    assert!(stderr.contains("--lossless"), "{stderr}");
    assert!(!webp.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// An op that refuses a value it parsed is a 1 today, and the README says
/// so. This cell is here so the table and the binary cannot drift apart
/// silently: whoever moves these to 2 changes both.
#[test]
fn an_op_refusing_a_value_it_parsed_exits_1() {
    let dir = unique_dir("clamp-inverted");
    let png = make_input(&dir, 16, 16);
    let res = dir.join("out.png");
    let out = run(&[
        "clamp",
        png.to_str().unwrap(),
        res.to_str().unwrap(),
        "--min",
        "200",
        "--max",
        "50",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code(&out), 1, "stderr:\n{stderr}");
    assert!(!res.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// PMTiles: the default storage flip and the `viprs pmtiles` group (#54)
// ---------------------------------------------------------------------------
//
// Everything below drives the real binary. The assertions read the artefact
// rather than the exit code wherever an artefact exists, because "the command
// exited 0" is also what a binary that wrote nothing does.
//
// Nothing here links `libviprs`: an integration test only sees the crate under
// test and its dev-dependencies, and this crate has none. That is a feature
// rather than a limitation. The PMTiles assertions below are made against the
// raw bytes of the archive (the 7-byte magic, the version byte, the header's
// little-endian u64 columns at the spec's fixed offsets), so a bug shared by
// our writer and our reader cannot hide from them.

/// The first seven bytes of every PMTiles archive, followed by the spec
/// version at offset 7 (`libviprs::pmtiles::header`).
const PMTILES_MAGIC: &[u8] = b"PMTiles";
/// PMTiles v3 header offsets we read directly, per the v3 specification.
const OFF_ROOT_OFFSET: usize = 8;
const OFF_ROOT_LENGTH: usize = 16;
const OFF_TILE_DATA_LENGTH: usize = 64;
const OFF_TILE_ENTRIES: usize = 80;
const OFF_MIN_ZOOM: usize = 100;
const OFF_MAX_ZOOM: usize = 101;
/// PNG's 8-byte signature, so a "tile" that is not a PNG cannot pass.
const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A temp directory nobody else in this suite will pick.
///
/// The pid alone is not enough: cargo runs these tests as threads in one
/// process, so every case needs its own tag or two cases racing on the same
/// path look like a flake in whichever lost.
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-e2e-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

/// Generate a synthetic PNG input through `viprs test-image` (pdfium-free).
fn make_input(dir: &std::path::Path, width: u32, height: u32) -> PathBuf {
    let png = dir.join("gen.png");
    let out = run(&[
        "test-image",
        png.to_str().unwrap(),
        "--width",
        &width.to_string(),
        "--height",
        &height.to_string(),
    ]);
    assert_eq!(
        code(&out),
        0,
        "test-image stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    png
}

/// Read a little-endian `u64` out of a PMTiles header.
fn header_u64(bytes: &[u8], off: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[off..off + 8]);
    u64::from_le_bytes(buf)
}

/// Assert the file at `path` is a PMTiles v3 archive and hand back its bytes.
fn read_archive(path: &std::path::Path) -> Vec<u8> {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("the archive at {} must be readable: {e}", path.display()));
    assert!(
        bytes.len() >= 127,
        "a PMTiles archive is at least a 127-byte header, got {} bytes",
        bytes.len()
    );
    assert_eq!(
        &bytes[0..7],
        PMTILES_MAGIC,
        "the archive must open with the PMTiles magic"
    );
    assert_eq!(bytes[7], 3, "the archive must declare spec version 3");
    bytes
}

/// Every file under `root`, as `(path relative to root, bytes)`, sorted.
fn collect_tree(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
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
            } else if let Ok(bytes) = std::fs::read(&path) {
                let rel = path
                    .strip_prefix(root)
                    .expect("every walked path sits under the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, bytes));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The `{z}/{x}/{y}.png` tiles of an XYZ tree, as `(z, x, y, bytes)`.
fn xyz_tiles(root: &std::path::Path) -> Vec<(u32, u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    for (rel, bytes) in collect_tree(root) {
        let Some(stem) = rel.strip_suffix(".png") else {
            continue;
        };
        let parts: Vec<&str> = stem.split('/').collect();
        if parts.len() != 3 {
            continue;
        }
        let (Ok(z), Ok(x), Ok(y)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) else {
            continue;
        };
        out.push((z, x, y, bytes));
    }
    out.sort_by_key(|(z, x, y, _)| (*z, *x, *y));
    out
}

#[test]
fn pyramid_default_output_is_a_pmtiles_archive() {
    // The headline flip: no output argument, no --storage, and what lands is a
    // single `.pmtiles` file named after the input stem, not a directory tree.
    let dir = unique_dir("default-pmtiles");
    let png = make_input(&dir, 700, 500);

    let out = run(&["pyramid", png.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "pyramid stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let archive = dir.join("gen.pmtiles");
    let bytes = read_archive(&archive);
    assert!(
        bytes.len() > 127,
        "the archive must carry tiles as well as a header, got {} bytes",
        bytes.len()
    );
    assert!(
        header_u64(&bytes, OFF_ROOT_LENGTH) > 0,
        "the archive must carry a non-empty root directory"
    );
    assert!(
        !dir.join("gen").exists(),
        "the default run must not also leave a loose tile tree behind"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_storage_directory_restores_the_tile_tree() {
    // The escape hatch has to keep working, and it has to produce real tiles.
    let dir = unique_dir("storage-directory");
    let png = make_input(&dir, 700, 500);
    let tree = dir.join("tiles");

    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        tree.to_str().unwrap(),
        "--storage",
        "directory",
    ]);
    assert_eq!(
        code(&out),
        0,
        "pyramid stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(tree.is_dir(), "--storage directory must write a directory");

    let files = collect_tree(&tree);
    let pngs: Vec<_> = files.iter().filter(|(p, _)| p.ends_with(".png")).collect();
    assert!(
        pngs.len() > 1,
        "the tree must hold more than one tile, got {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    for (path, bytes) in &pngs {
        assert!(
            bytes.starts_with(PNG_MAGIC),
            "{path} must be a real PNG, got {} bytes starting {:?}",
            bytes.len(),
            &bytes[..bytes.len().min(8)]
        );
    }
    assert!(
        !dir.join("gen.pmtiles").exists(),
        "--storage directory must not also write an archive"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_directory_target_without_storage_flag_exits_2() {
    // The migration case: an old invocation naming a directory, with the new
    // default. It has to name the flag that fixes it, or the exit code alone
    // passes for any usage error from any cause.
    let dir = unique_dir("dir-target-no-flag");
    let png = make_input(&dir, 320, 240);
    // Spelled with the archive extension *and* already a directory, so the
    // only signal that can catch it is that it is a directory. The
    // extensionless spelling gets its own case below, which keeps the two
    // signals independently observable.
    let target = dir.join("tiles.pmtiles");
    std::fs::create_dir_all(&target).expect("the target directory must be creatable");

    let out = run(&["pyramid", png.to_str().unwrap(), target.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        2,
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--storage directory"),
        "the diagnostic must name the flag that fixes it, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_extensionless_target_without_storage_flag_exits_2() {
    // Same refusal for a target that does not exist yet but is spelled like a
    // directory. This is the shape every README example used before the flip.
    let dir = unique_dir("extensionless-target");
    let png = make_input(&dir, 320, 240);
    let target = dir.join("out_tiles");

    let out = run(&["pyramid", png.to_str().unwrap(), target.to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--storage directory"),
        "the diagnostic must name the flag that fixes it, got:\n{stderr}"
    );
    assert!(
        !target.exists(),
        "a refused run must not have written anything"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_default_layout_with_pmtiles_is_xyz() {
    // PMTiles v3 addresses tiles as ZXY and nothing else, so the flip has to
    // move the default layout as well as the default sink.
    //
    // The archive records the layout it was built with in its `vnd.libviprs`
    // metadata, which is what makes this observable. Comparing two archives
    // byte for byte would not: the metadata also carries the archive's own
    // name, so two runs of the same command to different files differ anyway,
    // and Deep Zoom and XYZ produce the *same* tile grid (both come from
    // `compute_levels`), so the tile set cannot tell them apart either.
    let dir = unique_dir("default-layout-xyz");
    let png = make_input(&dir, 700, 500);

    let implicit = dir.join("implicit.pmtiles");
    let a = run(&["pyramid", png.to_str().unwrap(), implicit.to_str().unwrap()]);
    assert_eq!(
        code(&a),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&a.stderr)
    );
    read_archive(&implicit);

    let info = run(&["pmtiles", "info", implicit.to_str().unwrap()]);
    assert_eq!(code(&info), 0);
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(
        stdout.contains("\"layout\":\"xyz\""),
        "the default archive must record the xyz layout, got:\n{stdout}"
    );

    // The negative control: the recorded field tracks the flag rather than
    // being a constant, so the assertion above is worth something.
    let google = dir.join("google.pmtiles");
    let b = run(&[
        "pyramid",
        png.to_str().unwrap(),
        google.to_str().unwrap(),
        "--layout",
        "google",
    ]);
    assert_eq!(
        code(&b),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&b.stderr)
    );
    let google_info = run(&["pmtiles", "info", google.to_str().unwrap()]);
    assert_eq!(code(&google_info), 0);
    let google_stdout = String::from_utf8_lossy(&google_info.stdout);
    assert!(
        google_stdout.contains("\"layout\":\"google\""),
        "an explicit --layout google must be recorded as google, got:\n{google_stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_explicit_deep_zoom_with_pmtiles_exits_2() {
    // Deep Zoom tiers are not slippy zooms, so writing them into a ZXY archive
    // produces something every PMTiles viewer renders as nonsense. Refuse, and
    // name both flags so the message says which two things disagree.
    let dir = unique_dir("deepzoom-refusal");
    let png = make_input(&dir, 320, 240);

    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        dir.join("a.pmtiles").to_str().unwrap(),
        "--layout",
        "deep-zoom",
    ]);
    assert_eq!(
        code(&out),
        2,
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--layout deep-zoom") && stderr.contains("--storage"),
        "the diagnostic must name both flags, got:\n{stderr}"
    );
    assert!(
        !dir.join("a.pmtiles").exists(),
        "a refused run must not have written an archive"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_explicit_deep_zoom_with_storage_directory_still_works() {
    // The positive control for the refusal above: deep-zoom is not broken, it
    // is only incompatible with the archive. Without this, the refusal test
    // would also pass against a build that rejected --layout outright.
    let dir = unique_dir("deepzoom-positive");
    let png = make_input(&dir, 320, 240);
    let tree = dir.join("dz");

    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        tree.to_str().unwrap(),
        "--storage",
        "directory",
        "--layout",
        "deep-zoom",
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Deep Zoom names a tile `{level}/{col}_{row}.{ext}`; XYZ names it
    // `{z}/{x}/{y}.{ext}`. The path shape is the only thing that separates the
    // two here, because both layouts plan the same grid.
    let files = collect_tree(&tree);
    let deep_zoom_tiles = files
        .iter()
        .filter(|(p, _)| p.ends_with(".png") && p.matches('/').count() == 1 && p.contains('_'))
        .count();
    assert!(
        deep_zoom_tiles > 1,
        "a deep-zoom tree names tiles {{level}}/{{col}}_{{row}}.png, got {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert!(
        xyz_tiles(&tree).is_empty(),
        "a deep-zoom tree must not also carry xyz-shaped paths"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_format_raw_with_pmtiles_exits_2() {
    // PMTiles v3 has no raw tile type: a raw blob carries neither dimensions
    // nor pixel format, so there is nothing a viewer could do with it. This is
    // a flag combination that worked before the flip, so it gets a usage error
    // that names the way out rather than a sink failure deep in the run.
    let dir = unique_dir("raw-refusal");
    let png = make_input(&dir, 320, 240);

    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        dir.join("a.pmtiles").to_str().unwrap(),
        "--format",
        "raw",
    ]);
    assert_eq!(
        code(&out),
        2,
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--format raw") && stderr.contains("--storage directory"),
        "the diagnostic must name the combination and the way out, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_format_raw_with_storage_directory_still_works() {
    // The positive control for the refusal above.
    let dir = unique_dir("raw-positive");
    let png = make_input(&dir, 320, 240);
    let tree = dir.join("raw");

    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        tree.to_str().unwrap(),
        "--storage",
        "directory",
        "--format",
        "raw",
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !collect_tree(&tree).is_empty(),
        "--format raw must still produce a tree"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_offers_webp_and_the_archive_really_holds_webp_tiles() {
    // The inverse of what this test used to assert. It used to pin
    // `--format webp` as a usage error, and it was right to: core's
    // `TileFormat` had no WebP variant, so offering the flag would have
    // advertised a capability that did not exist. libviprs#1123 added the
    // variant, so the premise is gone and the refusal became the lie.
    //
    // Checking the exit code is not enough. A `FormatArg::Webp` that parsed
    // and then fell through to PNG would pass an exit-code assertion and write
    // PNG tiles, which is exactly the shape of "advertising a capability that
    // does not exist" that the old test existed to prevent. So this reads the
    // archive back and looks at the tile type byte.
    let out = run(&["pyramid", "--help"]);
    assert_eq!(code(&out), 0);
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.contains("png")
            && help.contains("jpeg")
            && help.contains("raw")
            && help.contains("webp"),
        "all four encodable formats must be listed, got:\n{help}"
    );

    let dir = unique_dir("webp-reachable");
    let png = make_input(&dir, 64, 64);
    let archive = dir.join("a.pmtiles");
    let made = run(&[
        "pyramid",
        png.to_str().unwrap(),
        archive.to_str().unwrap(),
        "--format",
        "webp",
    ]);
    assert_eq!(
        code(&made),
        0,
        "--format webp must work now that the encoder is reachable.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&made.stdout),
        String::from_utf8_lossy(&made.stderr)
    );

    // The archive's own account of itself, which is the part an exit code
    // cannot fake.
    let info = run(&["pmtiles", "info", archive.to_str().unwrap()]);
    assert_eq!(code(&info), 0);
    let reported = String::from_utf8_lossy(&info.stdout);
    assert!(
        reported.to_lowercase().contains("webp"),
        "the archive must report a WebP tile type, got:\n{reported}"
    );

    // And the bytes themselves. A WebP file is RIFF....WEBP, so this cannot be
    // satisfied by a PNG the header merely claims is WebP.
    let extracted = dir.join("out");
    let ex = run(&[
        "pmtiles",
        "extract",
        archive.to_str().unwrap(),
        extracted.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&ex),
        0,
        "extract failed:\n{}",
        String::from_utf8_lossy(&ex.stderr)
    );

    let mut checked = 0usize;
    for (rel, bytes) in collect_tree(&extracted) {
        if !rel.ends_with(".webp") {
            continue;
        }
        assert!(
            bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            "{rel} is not a WebP file, first bytes: {:?}",
            &bytes[..bytes.len().min(16)]
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "extract produced no .webp tiles, so nothing was actually checked"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn webp_ignores_quality_rather_than_pretending_to_use_it() {
    // `--quality` is documented as JPEG-only and `TileFormat::Webp` is
    // fieldless on purpose: the encoder is lossless and has no quality knob,
    // so a quality that appeared to apply would be an argument thrown away.
    // Two runs at different qualities must produce byte-identical archives,
    // which is the assertion that would notice a quality field being wired in
    // later without anyone deciding to.
    let dir = unique_dir("webp-quality-inert");
    let png = make_input(&dir, 64, 64);

    // Same BASENAME in different directories, deliberately. The archive
    // records a `name` derived from the output filename, and the metadata
    // section is gzipped, so two different names compress to different lengths
    // and shift every header offset after them. Comparing `a.pmtiles` against
    // `b.pmtiles` would therefore fail on the name and look exactly like the
    // quality leaking through, which is what it did when I first wrote this.
    let left_dir = dir.join("q10");
    let right_dir = dir.join("q95");
    std::fs::create_dir_all(&left_dir).expect("left dir");
    std::fs::create_dir_all(&right_dir).expect("right dir");
    let a = left_dir.join("same.pmtiles");
    let b = right_dir.join("same.pmtiles");
    for (out, q) in [(&a, "10"), (&b, "95")] {
        let r = run(&[
            "pyramid",
            png.to_str().unwrap(),
            out.to_str().unwrap(),
            "--format",
            "webp",
            "--quality",
            q,
        ]);
        assert_eq!(
            code(&r),
            0,
            "--format webp --quality {q} should run.\nstderr:\n{}",
            String::from_utf8_lossy(&r.stderr)
        );
    }

    let left = std::fs::read(&a).expect("read a");
    let right = std::fs::read(&b).expect("read b");
    assert_eq!(
        left, right,
        "quality 10 and quality 95 produced different WebP archives, so the \
         flag is reaching the encoder when it has nothing to reach. Both were \
         written to the same basename, so the archive `name` is identical and \
         cannot account for a difference"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_packfile_without_output_exits_2() {
    // `--packfile` composes `packfile://<output>.tar`, so with no output there
    // is no archive name to compose. Defined rather than left to a panic.
    let dir = unique_dir("packfile-no-output");
    let png = make_input(&dir, 64, 64);

    let out = run(&["pyramid", png.to_str().unwrap(), "--packfile"]);
    assert_eq!(
        code(&out),
        2,
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--packfile"),
        "the diagnostic must name --packfile, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_stdin_without_output_exits_2() {
    // `viprs pyramid -` has no stem to derive a default archive name from, so
    // the output argument stops being optional for that one input.
    let out = run(&["pyramid", "-"]);
    assert_eq!(
        code(&out),
        2,
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stdin"),
        "the diagnostic must explain that stdin has no name to derive, got:\n{stderr}"
    );
}

#[test]
fn pyramid_storage_directory_without_output_exits_2() {
    // There is no sensible default directory name, so asking for a tree with no
    // target is a usage error rather than a guess.
    let dir = unique_dir("storage-dir-no-output");
    let png = make_input(&dir, 64, 64);

    let out = run(&["pyramid", png.to_str().unwrap(), "--storage", "directory"]);
    assert_eq!(code(&out), 2);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--storage directory"),
        "the diagnostic must name the flag, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_sink_pmtiles_uri_writes_the_archive() {
    // `--sink pmtiles://…` is the explicit spelling and it has to keep working
    // next to `--storage`.
    let dir = unique_dir("sink-uri");
    let png = make_input(&dir, 320, 240);
    let archive = dir.join("via-uri.pmtiles");

    let out = run(&[
        "pyramid",
        png.to_str().unwrap(),
        "--sink",
        &format!("pmtiles://{}", archive.display()),
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    read_archive(&archive);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_info_reports_the_archive_summary() {
    let dir = unique_dir("info");
    let png = make_input(&dir, 700, 500);
    let archive = dir.join("gen.pmtiles");
    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);
    let bytes = read_archive(&archive);

    let out = run(&["pmtiles", "info", archive.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    for needle in [
        "Version: 3",
        "Tile type: png",
        "Zoom:",
        "Addressed tiles:",
        "Unique payloads:",
        "Archive size:",
        "Bounds:",
    ] {
        assert!(
            stdout.contains(needle),
            "info must report {needle:?}, got:\n{stdout}"
        );
    }

    // The numbers have to be the archive's, not a constant. Cross-check the two
    // that the raw header hands us independently of anything we wrote.
    let zoom_line = format!("Zoom: {}-{}", bytes[OFF_MIN_ZOOM], bytes[OFF_MAX_ZOOM]);
    assert!(
        stdout.contains(&zoom_line),
        "info must report the header's own zoom range ({zoom_line}), got:\n{stdout}"
    );
    let size_line = format!("Archive size: {} bytes", bytes.len());
    assert!(
        stdout.contains(&size_line),
        "info must report the real file size ({size_line}), got:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_info_on_a_non_archive_exits_1() {
    // A PNG is not an archive. Operational failure, exit 1, nothing on stdout.
    let dir = unique_dir("info-not-an-archive");
    let png = make_input(&dir, 64, 64);

    let out = run(&["pmtiles", "info", png.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    assert!(
        out.stdout.is_empty(),
        "a failed info must write nothing to stdout, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.stderr.is_empty(),
        "a failed info must say why on stderr"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_tile_writes_identical_bytes_to_stdout_and_to_a_file() {
    // The data path. stdout carries raw tile bytes and nothing else, so a
    // stray `println!` anywhere in the command shows up here as a mismatch.
    let dir = unique_dir("tile-bytes");
    let png = make_input(&dir, 700, 500);
    let archive = dir.join("gen.pmtiles");
    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);

    let piped = run(&["pmtiles", "tile", archive.to_str().unwrap(), "0", "0", "0"]);
    assert_eq!(
        code(&piped),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&piped.stderr)
    );
    assert!(
        piped.stdout.starts_with(PNG_MAGIC),
        "stdout must be the raw PNG tile, got {} bytes starting {:?}",
        piped.stdout.len(),
        &piped.stdout[..piped.stdout.len().min(8)]
    );

    let file = dir.join("tile.png");
    let written = run(&[
        "pmtiles",
        "tile",
        archive.to_str().unwrap(),
        "0",
        "0",
        "0",
        "--output",
        file.to_str().unwrap(),
    ]);
    assert_eq!(code(&written), 0);
    let from_file = std::fs::read(&file).expect("--output must write the tile");
    assert_eq!(
        piped.stdout, from_file,
        "the piped bytes and the written file must be identical"
    );

    // `--output -` is the explicit spelling of the default.
    let dash = run(&[
        "pmtiles",
        "tile",
        archive.to_str().unwrap(),
        "0",
        "0",
        "0",
        "--output",
        "-",
    ]);
    assert_eq!(code(&dash), 0);
    assert_eq!(
        dash.stdout, from_file,
        "--output - must write the same bytes to stdout"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_tile_bytes_match_the_directory_pyramid() {
    // The cross-backend claim, in its cheap in-repo form: the same input into
    // both sinks gives the same tile bytes at the same coordinate. The count
    // assertion is the positive control, because a loop over an empty tile set
    // passes without comparing anything.
    let dir = unique_dir("tile-cross-backend");
    let png = make_input(&dir, 700, 500);
    let archive = dir.join("gen.pmtiles");
    let tree = dir.join("tree");

    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);
    assert_eq!(
        code(&run(&[
            "pyramid",
            png.to_str().unwrap(),
            tree.to_str().unwrap(),
            "--storage",
            "directory",
            "--layout",
            "xyz",
        ])),
        0
    );

    let tiles = xyz_tiles(&tree);
    assert!(
        tiles.len() > 1,
        "the xyz tree must hold more than one tile for this comparison to mean anything, got {}",
        tiles.len()
    );

    for (z, x, y, expected) in &tiles {
        let out = run(&[
            "pmtiles",
            "tile",
            archive.to_str().unwrap(),
            &z.to_string(),
            &x.to_string(),
            &y.to_string(),
        ]);
        assert_eq!(
            code(&out),
            0,
            "tile {z}/{x}/{y} must be in the archive, stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            &out.stdout, expected,
            "tile {z}/{x}/{y} must be byte-identical across the two sinks"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_tile_missing_exits_1_with_empty_stdout() {
    // An absent tile is an operational failure, and the empty stdout is half
    // the contract: a caller piping this into a decoder must not get a
    // diagnostic where the bytes should be.
    let dir = unique_dir("tile-missing");
    let png = make_input(&dir, 320, 240);
    let archive = dir.join("gen.pmtiles");
    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);

    let out = run(&[
        "pmtiles",
        "tile",
        archive.to_str().unwrap(),
        "20",
        "1000",
        "1000",
    ]);
    assert_eq!(code(&out), 1);
    assert!(
        out.stdout.is_empty(),
        "a missing tile must write nothing to stdout, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("20/1000/1000"),
        "the diagnostic must name the coordinate, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_verify_accepts_a_generated_archive() {
    let dir = unique_dir("verify-good");
    let png = make_input(&dir, 700, 500);
    let archive = dir.join("gen.pmtiles");
    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);

    let out = run(&["pmtiles", "verify", archive.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("OK") && stdout.contains("entries"),
        "verify must report what it checked, got:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_verify_rejects_a_corrupt_root_directory() {
    // Corrupt inside the root directory region, whose offset comes from the
    // header rather than a guess. A random byte in the tile data may well
    // verify clean, and then the test passes for the wrong reason.
    let dir = unique_dir("verify-corrupt");
    let png = make_input(&dir, 700, 500);
    let archive = dir.join("gen.pmtiles");
    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);

    let mut bytes = read_archive(&archive);
    let root_offset = header_u64(&bytes, OFF_ROOT_OFFSET) as usize;
    let root_length = header_u64(&bytes, OFF_ROOT_LENGTH) as usize;
    assert!(
        root_length > 2 && root_offset + root_length <= bytes.len(),
        "the root directory must be a real region before corrupting it \
         (offset {root_offset}, length {root_length}, file {})",
        bytes.len()
    );
    let target = root_offset + root_length / 2;
    let before = bytes[target];
    bytes[target] ^= 0xff;
    assert_ne!(bytes[target], before, "the corruption must change the byte");

    let corrupt = dir.join("corrupt.pmtiles");
    std::fs::write(&corrupt, &bytes).expect("the corrupt archive must be writable");

    let out = run(&["pmtiles", "verify", corrupt.to_str().unwrap()]);
    assert_ne!(
        code(&out),
        0,
        "verify must refuse a corrupt root directory, stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.stderr.is_empty(),
        "verify must say what it found on stderr"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_extract_reproduces_the_directory_tree() {
    // The compat route: unpack an archive back into the loose tree, and get the
    // same files with the same bytes as generating straight into a directory.
    let dir = unique_dir("extract");
    let png = make_input(&dir, 700, 500);
    let archive = dir.join("gen.pmtiles");
    let tree = dir.join("tree");
    let unpacked = dir.join("unpacked");

    assert_eq!(code(&run(&["pyramid", png.to_str().unwrap()])), 0);
    assert_eq!(
        code(&run(&[
            "pyramid",
            png.to_str().unwrap(),
            tree.to_str().unwrap(),
            "--storage",
            "directory",
            "--layout",
            "xyz",
        ])),
        0
    );

    let out = run(&[
        "pmtiles",
        "extract",
        archive.to_str().unwrap(),
        unpacked.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let expected = xyz_tiles(&tree);
    let actual = xyz_tiles(&unpacked);
    assert!(
        expected.len() > 1,
        "the comparison needs more than one tile to mean anything"
    );
    assert_eq!(
        expected
            .iter()
            .map(|(z, x, y, _)| (*z, *x, *y))
            .collect::<Vec<_>>(),
        actual
            .iter()
            .map(|(z, x, y, _)| (*z, *x, *y))
            .collect::<Vec<_>>(),
        "extract must reproduce exactly the generated coordinate set"
    );
    assert_eq!(
        expected, actual,
        "extract must reproduce the tile bytes too"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn help_lists_the_pmtiles_group() {
    let out = run(&["--help"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("pmtiles"),
        "the top-level help must list the pmtiles group, got:\n{stdout}"
    );
}

#[test]
fn pmtiles_help_lists_the_five_subcommands() {
    let out = run(&["pmtiles", "--help"]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for name in ["info", "tile", "verify", "extract", "pack"] {
        assert!(
            stdout.contains(name),
            "pmtiles --help must list {name}, got:\n{stdout}"
        );
    }
}

#[test]
fn pmtiles_without_a_subcommand_is_a_usage_error_exit_2() {
    let out = run(&["pmtiles"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn dump_commands_json_reaches_the_pmtiles_subcommands() {
    // `__dump-commands` walked one level, so a command group landed in the
    // site's data as a name with no positionals and no flags, and its
    // subcommands were invisible to the generated reference entirely. The dump
    // now recurses, and this is the assertion that keeps it recursing.
    let out = run(&["__dump-commands", "--json"]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // No serde in an integration test of a binary-only crate, so the assertions
    // are made on the document's text. Each is specific enough that a dump
    // without the recursion cannot satisfy it.
    assert!(
        stdout.contains("\"subcommands\""),
        "every command entry must carry a subcommands array, got:\n{stdout}"
    );
    for name in ["\"info\"", "\"tile\"", "\"verify\"", "\"extract\""] {
        assert!(
            stdout.contains(name),
            "the dump must reach the pmtiles subcommand {name}, got:\n{stdout}"
        );
    }
    // `tile` takes four positionals; a non-recursing dump has none of them.
    for positional in ["\"z\"", "\"x\"", "\"y\""] {
        assert!(
            stdout.contains(positional),
            "the dump must carry the {positional} positional of `pmtiles tile`, got:\n{stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// The two archives go-pmtiles wrote
// ---------------------------------------------------------------------------
//
// `verify` and `extract` walk directories, and every other case in this file
// walks a directory our own writer produced. A misreading of the spec shared by
// our writer and our reader survives all of them. These two files were written
// by the reference implementation and have never been through any libviprs
// code, so they cannot agree with us by construction. See
// `tests/fixtures/pmtiles/PROVENANCE.md`.

/// Path to a committed golden archive.
fn golden(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pmtiles")
        .join(name);
    assert!(
        path.is_file(),
        "the committed golden {name} must exist at {}",
        path.display()
    );
    path
}

#[test]
fn pmtiles_info_reports_the_recorded_golden_counts() {
    // Recorded reference values, not our own output read back. 85 addressed
    // tiles in 67 entries is what makes `dupes-z0z3` worth committing: the
    // three numbers differ from each other, so a reader that conflated any two
    // of them reds here.
    let archive = golden("dupes-z0z3.pmtiles");
    let out = run(&["pmtiles", "info", archive.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "Tile type: png",
        "Zoom: 0-3",
        "Addressed tiles: 85",
        "Tile entries: 67",
        "Unique payloads: 63",
        "Leaf directories: no",
    ] {
        assert!(
            stdout.contains(needle),
            "info must report {needle:?} for the go-pmtiles golden, got:\n{stdout}"
        );
    }
}

#[test]
fn pmtiles_verify_accepts_an_archive_with_leaf_directories() {
    // 6 root entries pointing at 6 leaves holding 21844 tile entries. The walk
    // has to follow every pointer and add up what it finds, and the header's
    // own counts are the cross-check: 21845 addressed tiles from 21844 entries
    // only agrees if every leaf was read and every run length counted.
    let archive = golden("leaves-z0z7.pmtiles");

    let info = run(&["pmtiles", "info", archive.to_str().unwrap()]);
    assert_eq!(code(&info), 0);
    let summary = String::from_utf8_lossy(&info.stdout);
    assert!(
        summary.contains("Leaf directories: yes") && summary.contains("Root entries: 6"),
        "this fixture earns its place by having leaves, got:\n{summary}"
    );

    let out = run(&["pmtiles", "verify", archive.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "verify must accept what the reference implementation wrote, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "Leaf directories: 6",
        "Tile entries: 21844",
        "Addressed tiles: 21845",
        "OK",
    ] {
        assert!(
            stdout.contains(needle),
            "verify must report {needle:?}, got:\n{stdout}"
        );
    }
}

#[test]
fn pmtiles_extract_expands_a_run_into_one_file_per_coordinate() {
    // A run is one entry covering consecutive tile ids that share a payload,
    // which is how the format stores a deduplicated tile. Extract has to write
    // every coordinate in the run, so 67 entries have to become 85 files. An
    // extract that ignored run lengths writes 67 and looks perfectly healthy.
    let dir = unique_dir("extract-runs");
    let archive = golden("dupes-z0z3.pmtiles");
    let unpacked = dir.join("unpacked");

    let out = run(&[
        "pmtiles",
        "extract",
        archive.to_str().unwrap(),
        unpacked.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Extracted 85 tiles"),
        "extract must report the addressed count, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let tiles = xyz_tiles(&unpacked);
    assert_eq!(
        tiles.len(),
        85,
        "the archive addresses 85 tiles through 67 entries, so 85 files have to land"
    );
    for (z, x, y, bytes) in &tiles {
        assert!(
            bytes.starts_with(PNG_MAGIC),
            "{z}/{x}/{y} must be a real PNG"
        );
    }

    // The duplicates are the point: fewer distinct payloads than files.
    let mut distinct: Vec<&Vec<u8>> = tiles.iter().map(|(_, _, _, b)| b).collect();
    distinct.sort();
    distinct.dedup();
    assert!(
        distinct.len() < tiles.len(),
        "the fixture is chosen for its duplicate payloads, got {} distinct of {}",
        distinct.len(),
        tiles.len()
    );
}

#[test]
fn pmtiles_verify_rejects_a_header_count_that_the_directories_contradict() {
    // The corrupt-root case fails while the archive is being opened, so it
    // proves the reader rather than the walk. This one opens cleanly and is
    // caught only by verify adding up what the directories hold and comparing
    // it with what the header claims.
    let dir = unique_dir("verify-count-mismatch");
    let mut bytes =
        std::fs::read(golden("dupes-z0z3.pmtiles")).expect("the golden must be readable");
    let before = header_u64(&bytes, OFF_TILE_ENTRIES);
    assert_eq!(before, 67, "the fixture's recorded entry count");
    bytes[OFF_TILE_ENTRIES..OFF_TILE_ENTRIES + 8].copy_from_slice(&(before + 1).to_le_bytes());

    let tampered = dir.join("miscounted.pmtiles");
    std::fs::write(&tampered, &bytes).expect("the tampered archive must be writable");

    let out = run(&["pmtiles", "verify", tampered.to_str().unwrap()]);
    assert_ne!(
        code(&out),
        0,
        "verify must refuse a header that disagrees with its own directories, stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("68") && stderr.contains("67"),
        "the diagnostic must name both counts, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_verify_rejects_entries_past_the_tile_data_section() {
    // Shrink the tile data section in the header. Every section still fits
    // inside the file, so the archive opens, and the only thing that catches it
    // is the check the reference implementation does not make: an entry's
    // offset plus its length against the section that owns it.
    let dir = unique_dir("verify-entry-bounds");
    let mut bytes =
        std::fs::read(golden("dupes-z0z3.pmtiles")).expect("the golden must be readable");
    let before = header_u64(&bytes, OFF_TILE_DATA_LENGTH);
    assert!(
        before > 64,
        "the fixture must have a real tile data section"
    );
    bytes[OFF_TILE_DATA_LENGTH..OFF_TILE_DATA_LENGTH + 8].copy_from_slice(&16u64.to_le_bytes());

    let tampered = dir.join("short-section.pmtiles");
    std::fs::write(&tampered, &bytes).expect("the tampered archive must be writable");

    let out = run(&["pmtiles", "verify", tampered.to_str().unwrap()]);
    assert_ne!(
        code(&out),
        0,
        "verify must refuse an entry addressing bytes outside its section, stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tile data section"),
        "the diagnostic must name the section, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_round_trips_a_tree_into_the_archive_it_came_from() {
    // `pack` is the inverse of `extract`, so packing a generated tree and
    // extracting the result has to give the tree back, coordinate for
    // coordinate and byte for byte.
    //
    // The input is deliberately NOT square. Every placement bug this command
    // can have is a permutation of (col, row): a transposed tile id, a flipped
    // row axis, a level base off by one. On a square grid a transposition is a
    // permutation of the same coordinate set, so the *set* still matches and
    // only the bytes move, and any tile that happens to be blank on both sides
    // matches anyway. At 700x500 the grids are not square, so a transposition
    // cannot even produce the same coordinate set. See libviprs#1118, which
    // says a wrong plan yields a structurally perfect archive that
    // `pmtiles verify` passes.
    let dir = unique_dir("pack-roundtrip");
    let png = make_input(&dir, 700, 500);
    let tree = dir.join("tree");
    let packed = dir.join("packed.pmtiles");
    let unpacked = dir.join("unpacked");

    assert_eq!(
        code(&run(&[
            "pyramid",
            png.to_str().unwrap(),
            tree.to_str().unwrap(),
            "--storage",
            "directory",
            "--layout",
            "xyz",
        ])),
        0
    );

    let out = run(&[
        "pmtiles",
        "pack",
        tree.to_str().unwrap(),
        packed.to_str().unwrap(),
        "--width",
        "700",
        "--height",
        "500",
        "--layout",
        "xyz",
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run(&[
        "pmtiles",
        "extract",
        packed.to_str().unwrap(),
        unpacked.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let expected = xyz_tiles(&tree);
    let actual = xyz_tiles(&unpacked);
    assert!(
        expected.len() > 1,
        "the comparison needs more than one tile to mean anything"
    );
    // The control for the paragraph above: if the grid were square this test
    // could not tell a transposition from a correct pack.
    let widest = expected.iter().map(|(_, x, _, _)| *x).max().unwrap_or(0);
    let tallest = expected.iter().map(|(_, _, y, _)| *y).max().unwrap_or(0);
    assert_ne!(
        widest, tallest,
        "the fixture has gone square, so a transposed tile id would survive this test"
    );

    assert_eq!(
        expected
            .iter()
            .map(|(z, x, y, _)| (*z, *x, *y))
            .collect::<Vec<_>>(),
        actual
            .iter()
            .map(|(z, x, y, _)| (*z, *x, *y))
            .collect::<Vec<_>>(),
        "pack then extract must reproduce exactly the generated coordinate set"
    );
    assert_eq!(expected, actual, "and the tile bytes with it");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_to_run_without_a_plan() {
    // The CLI-level restatement of the trap. A directory of tiles does not say
    // what its level indices mean, how big a tile is, or which layout placed
    // it, so the command has to stop rather than pick something plausible.
    let dir = unique_dir("pack-no-plan");
    let png = make_input(&dir, 700, 500);
    let tree = dir.join("tree");
    let packed = dir.join("packed.pmtiles");

    assert_eq!(
        code(&run(&[
            "pyramid",
            png.to_str().unwrap(),
            tree.to_str().unwrap(),
            "--storage",
            "directory",
            "--layout",
            "xyz",
        ])),
        0
    );

    let out = run(&[
        "pmtiles",
        "pack",
        tree.to_str().unwrap(),
        packed.to_str().unwrap(),
        "--layout",
        "xyz",
    ]);
    // Exactly 2, not merely non-zero: a missing plan is a usage mistake, and
    // a crash or a failed read (1) passing this test would hide that the
    // refusal never ran.
    assert_eq!(code(&out), 2, "a pack with no dimensions is a usage error");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("width") || stderr.contains("manifest"),
        "the refusal must name what is missing, got:\n{stderr}"
    );
    assert!(
        !packed.exists(),
        "a refused pack must not leave an archive behind"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_reads_the_plan_from_a_manifest() {
    // The good path: a tree written with --manifest already carries tile size,
    // overlap, layout, format and the source dimensions, so packing it is
    // reporting rather than guessing and needs no dimension flags at all.
    let dir = unique_dir("pack-manifest");
    let png = make_input(&dir, 700, 500);
    let tree = dir.join("tree");
    // `viprs pyramid` has no `--manifest <FILE>` flag: the manifest is emitted
    // under a convention when checksums are asked for, both beside the tree and
    // inside it. libviprs-cli#59 assumed a flag that does not exist.
    let manifest = tree.join("manifest.json");
    let packed = dir.join("packed.pmtiles");
    let unpacked = dir.join("unpacked");

    assert_eq!(
        code(&run(&[
            "pyramid",
            png.to_str().unwrap(),
            tree.to_str().unwrap(),
            "--storage",
            "directory",
            "--layout",
            "xyz",
            "--manifest-emit-checksums",
        ])),
        0
    );
    assert!(manifest.is_file(), "the manifest must have been written");

    let out = run(&[
        "pmtiles",
        "pack",
        tree.to_str().unwrap(),
        packed.to_str().unwrap(),
        "--manifest",
        manifest.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        code(&run(&[
            "pmtiles",
            "extract",
            packed.to_str().unwrap(),
            unpacked.to_str().unwrap(),
        ])),
        0
    );
    assert_eq!(
        xyz_tiles(&tree),
        xyz_tiles(&unpacked),
        "the manifest route must land every tile where the explicit route does"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_a_manifest_and_explicit_dimensions_together() {
    // Mutually exclusive by ArgGroup rather than by a precedence rule nobody
    // reads. Two descriptions of one plan that disagree is the case that
    // produces a wrong archive quietly.
    let dir = unique_dir("pack-both");
    let out = run(&[
        "pmtiles",
        "pack",
        dir.to_str().unwrap(),
        dir.join("packed.pmtiles").to_str().unwrap(),
        "--manifest",
        dir.join("manifest.json").to_str().unwrap(),
        "--width",
        "700",
    ]);
    assert_ne!(code(&out), 0, "--manifest with --width must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("conflict"),
        "the refusal should come from the parser, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Run `viprs` and give up after `secs`, killing it, rather than hanging the
/// suite. For the refusals that exist to stop a walk over billions of
/// coordinates: without them the binary never comes back, and a test that
/// hangs reports nothing useful.
fn run_bounded(args: &[&str], secs: u64) -> Output {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = viprs()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the viprs binary must be spawnable");
    // Drained on threads so a chatty child cannot block on a full pipe while
    // this side waits for it to exit.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait must not fail") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("viprs {args:?} was still running after {secs}s");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Output {
        status,
        stdout: out_thread.join().expect("stdout reader"),
        stderr: err_thread.join().expect("stderr reader"),
    }
}

/// Generate a 700x500 XYZ PNG tree under `dir/tree` and hand back its path.
fn make_xyz_tree(dir: &std::path::Path, extra: &[&str]) -> PathBuf {
    let png = make_input(dir, 700, 500);
    let tree = dir.join("tree");
    let mut args = vec![
        "pyramid",
        png.to_str().unwrap(),
        tree.to_str().unwrap(),
        "--storage",
        "directory",
        "--layout",
        "xyz",
    ];
    args.extend_from_slice(extra);
    let out = run(&args);
    assert_eq!(
        code(&out),
        0,
        "pyramid stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    tree
}

/// `pmtiles pack TREE ARCHIVE --width 700 --height 500 --layout xyz`, plus
/// whatever else the case needs.
fn pack_700x500(tree: &std::path::Path, archive: &std::path::Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "pmtiles",
        "pack",
        tree.to_str().unwrap(),
        archive.to_str().unwrap(),
        "--width",
        "700",
        "--height",
        "500",
        "--layout",
        "xyz",
    ];
    args.extend_from_slice(extra);
    run_bounded(&args, 120)
}

#[test]
fn pmtiles_pack_refuses_an_invalid_plan_as_a_usage_error() {
    // A plan that describes no pyramid at all came off the command line, so
    // it is the person's typo and exits 2, before anything is read or written.
    let dir = unique_dir("pack-invalid-plan");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    for bad in [
        ["--width", "0", "--height", "500", "--tile-size", "256"],
        ["--width", "700", "--height", "500", "--tile-size", "0"],
    ] {
        let mut args = vec![
            "pmtiles",
            "pack",
            tree.to_str().unwrap(),
            packed.to_str().unwrap(),
        ];
        args.extend_from_slice(&bad);
        let out = run_bounded(&args, 60);
        assert_eq!(
            code(&out),
            2,
            "{bad:?} must be a usage error, stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !packed.exists(),
            "{bad:?}: a refused pack must not leave an archive behind"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_a_plan_too_deep_to_address_before_walking_it() {
    // `--width 4294967295 --tile-size 1` is one slipped digit away from a real
    // command, plans fine, and puts its top level past zoom 31, which PMTiles
    // cannot address. Walked blind it stats billions of absent coordinates and
    // never comes back, so the refusal has to happen before the walk, and it
    // has to be the usage error a typo deserves.
    let dir = unique_dir("pack-too-deep");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let out = run_bounded(
        &[
            "pmtiles",
            "pack",
            tree.to_str().unwrap(),
            packed.to_str().unwrap(),
            "--width",
            "4294967295",
            "--height",
            "1",
            "--tile-size",
            "1",
        ],
        60,
    );
    assert_eq!(
        code(&out),
        2,
        "an unaddressable plan is a usage error, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("zoom") || stderr.contains("address"),
        "the refusal must say the plan is not addressable, got:\n{stderr}"
    );
    assert!(
        !packed.exists(),
        "nothing may be written for a refused plan"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_a_layout_pmtiles_cannot_address() {
    let dir = unique_dir("pack-deep-zoom");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let out = run_bounded(
        &[
            "pmtiles",
            "pack",
            tree.to_str().unwrap(),
            packed.to_str().unwrap(),
            "--width",
            "700",
            "--height",
            "500",
            "--layout",
            "deep-zoom",
        ],
        60,
    );
    assert_eq!(
        code(&out),
        2,
        "a deep-zoom plan named on the command line is a usage error, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("layout"),
        "the refusal must name the layout, got:\n{stderr}"
    );
    assert!(!packed.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_format_raw() {
    // PMTiles has no tile type for raw pixels, so `--format raw` can never
    // produce an archive and is refused up front as the usage mistake it is.
    let dir = unique_dir("pack-raw");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let out = pack_700x500(&tree, &packed, &["--format", "raw"]);
    assert_eq!(
        code(&out),
        2,
        "--format raw is a usage error, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("--format raw"),
        "the refusal must name the format, got:\n{stderr}"
    );
    assert!(!packed.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_round_trips_a_centred_tree_with_centre() {
    // A manifest cannot say whether the image was centred, so `--centre` is how
    // a centred tree gets packed onto the grid that placed it. The effective
    // plan line is what lets a person see the flag took.
    let dir = unique_dir("pack-centre");
    let tree = make_xyz_tree(&dir, &["--centre"]);
    let packed = dir.join("packed.pmtiles");
    let unpacked = dir.join("unpacked");

    let out = pack_700x500(&tree, &packed, &["--centre"]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout
            .lines()
            .any(|l| l.trim_start().starts_with("centre") && l.trim_end().ends_with("yes")),
        "the effective plan must say the grid is centred, got:\n{stdout}"
    );

    assert_eq!(
        code(&run(&[
            "pmtiles",
            "extract",
            packed.to_str().unwrap(),
            unpacked.to_str().unwrap(),
        ])),
        0
    );
    let expected = xyz_tiles(&tree);
    assert!(expected.len() > 1);
    assert_eq!(expected, xyz_tiles(&unpacked));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_prints_the_effective_plan_before_packing() {
    // Four of the six plan fields have defaults, so the run has to say what it
    // actually used: a person who forgot `--format jpeg` sees `png` here.
    let dir = unique_dir("pack-plan-banner");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let out = pack_700x500(&tree, &packed, &[]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
    for needle in [
        "tile size",
        "256",
        "overlap",
        "layout",
        "xyz",
        "format",
        "png",
        "700",
        "500",
    ] {
        assert!(
            stdout.contains(needle),
            "the effective plan must mention {needle:?}, got:\n{stdout}"
        );
    }
    // Printed before the pack report, not after it.
    let plan_at = stdout.find("tile size").unwrap();
    let report_at = stdout
        .find("tiles written")
        .expect("the pack report must still be printed");
    assert!(plan_at < report_at, "the plan must come first:\n{stdout}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_fails_when_the_tree_is_in_another_format() {
    // The default `--format png` over a JPEG tree (or `--format jpeg` over a
    // PNG one) visits every coordinate, finds nothing, and used to write a
    // valid empty archive and exit 0. Nothing packed is a failure.
    let dir = unique_dir("pack-wrong-format");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let out = pack_700x500(&tree, &packed, &["--format", "jpeg"]);
    assert_eq!(
        code(&out),
        1,
        "packing nothing must fail, stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("no tiles"),
        "the failure must say nothing was found, got:\n{stderr}"
    );
    assert!(
        stderr.contains(".png"),
        "and point at the tiles that are there, got:\n{stderr}"
    );
    assert!(
        !packed.exists(),
        "a failed pack must not leave an empty archive under the final name"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_warns_about_absent_and_unvisited_tiles() {
    // A sparse tree is legitimate (skip-blanks leaves holes), so absence is a
    // warning and the run still succeeds. A tile-shaped file the plan never
    // reaches is a sign the plan is wrong, and says so too.
    let dir = unique_dir("pack-warnings");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let tiles = xyz_tiles(&tree);
    let (z, x, y, _) = tiles.last().expect("the tree has tiles");
    std::fs::remove_file(tree.join(format!("{z}/{x}/{y}.png"))).unwrap();
    std::fs::create_dir_all(tree.join("0/7")).unwrap();
    std::fs::write(tree.join("0/7/7.png"), b"not visited").unwrap();

    let out = pack_700x500(&tree, &packed, &[]);
    assert_eq!(
        code(&out),
        0,
        "holes alone must not fail the pack, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Matched on whole phrases, because the temp directory's own name carries
    // "warnings" and a bare word would match the path.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Warning: 1 of the") && stderr.contains("absent from the tree"),
        "the absent tile must be warned about, got:\n{stderr}"
    );
    assert!(
        stderr.contains("never visited"),
        "the stray tile-shaped file must be warned about, got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn pmtiles_pack_refuses_a_symlinked_tile() {
    // Following a symlink would copy whatever it points at into the archive:
    // a file outside the tree, a FIFO that blocks forever, /dev/zero. The tile
    // reads take the link itself and refuse anything that is not a regular
    // file.
    let dir = unique_dir("pack-symlink");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");
    let secret = dir.join("outside.txt");
    std::fs::write(&secret, b"outside the tree").unwrap();

    let tiles = xyz_tiles(&tree);
    let (z, x, y, _) = tiles.first().expect("the tree has tiles");
    let tile = tree.join(format!("{z}/{x}/{y}.png"));
    std::fs::remove_file(&tile).unwrap();
    std::os::unix::fs::symlink(&secret, &tile).unwrap();

    let out = pack_700x500(&tree, &packed, &[]);
    assert_eq!(
        code(&out),
        1,
        "a symlinked tile must be refused, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    // "is a symlink", not just "symlink": the temp directory's own name
    // carries the word, so the bare word matched the path and passed with the
    // guard switched off.
    assert!(
        stderr.contains("is a symlink"),
        "the refusal must say why, got:\n{stderr}"
    );
    assert!(!packed.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_a_tile_path_that_is_not_a_regular_file() {
    let dir = unique_dir("pack-not-regular");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let tiles = xyz_tiles(&tree);
    let (z, x, y, _) = tiles.first().expect("the tree has tiles");
    let tile = tree.join(format!("{z}/{x}/{y}.png"));
    std::fs::remove_file(&tile).unwrap();
    std::fs::create_dir(&tile).unwrap();

    let out = pack_700x500(&tree, &packed, &[]);
    assert_eq!(
        code(&out),
        1,
        "a directory where a tile should be must be refused, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("not a regular file"),
        "the refusal must say why, got:\n{stderr}"
    );
    assert!(!packed.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_refuses_a_tile_larger_than_any_tile_of_its_size() {
    // One tile is capped at what a tile of the plan's size could possibly
    // weigh, so a stray multi-gigabyte file at a tile path is refused rather
    // than read into memory.
    let dir = unique_dir("pack-oversized");
    let tree = make_xyz_tree(&dir, &[]);
    let packed = dir.join("packed.pmtiles");

    let tiles = xyz_tiles(&tree);
    let (z, x, y, _) = tiles.first().expect("the tree has tiles");
    let tile = tree.join(format!("{z}/{x}/{y}.png"));
    let f = std::fs::File::create(&tile).unwrap();
    f.set_len(4 * 1024 * 1024).unwrap();
    drop(f);

    let out = pack_700x500(&tree, &packed, &[]);
    assert_eq!(
        code(&out),
        1,
        "an oversized tile must be refused, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("larger than"),
        "the refusal must say why, got:\n{stderr}"
    );
    assert!(!packed.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_help_names_flags_that_exist() {
    // The help used to send people to `viprs pyramid --manifest`, which is not
    // a flag. The manifest comes from a pyramid run that wrote one.
    let out = run(&["pmtiles", "pack", "--help"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("pyramid --manifest`"),
        "the help must not name a nonexistent flag, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--manifest-emit-checksums"),
        "the help must name the flag that really writes a manifest, got:\n{stdout}"
    );
}

#[test]
fn pmtiles_pack_takes_the_manifest_inside_the_tree_when_nothing_else_is_said() {
    // A manifest the pyramid run wrote into the tree is reporting, not
    // guessing, so with no plan flags at all pack uses it and says so.
    let dir = unique_dir("pack-auto-manifest");
    let tree = make_xyz_tree(&dir, &["--manifest-emit-checksums"]);
    assert!(tree.join("manifest.json").is_file());
    let packed = dir.join("packed.pmtiles");
    let unpacked = dir.join("unpacked");

    let out = run_bounded(
        &[
            "pmtiles",
            "pack",
            tree.to_str().unwrap(),
            packed.to_str().unwrap(),
        ],
        120,
    );
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("manifest.json"),
        "the run must say where its plan came from, got:\n{stdout}"
    );
    assert_eq!(
        code(&run(&[
            "pmtiles",
            "extract",
            packed.to_str().unwrap(),
            unpacked.to_str().unwrap(),
        ])),
        0
    );
    assert_eq!(xyz_tiles(&tree), xyz_tiles(&unpacked));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Built-in values out of range are usage mistakes (#81)
// ---------------------------------------------------------------------------

/// Run a command that must be refused as a usage mistake, and check the
/// refusal names the flag (or value) that fixes it and wrote nothing.
fn assert_usage_refusal(args: &[&str], needle: &str, must_not_exist: &[&std::path::Path]) {
    let out = run_bounded(args, 60);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code(&out), 2, "{args:?} must exit 2, stderr:\n{stderr}");
    assert!(
        stderr.contains(needle),
        "{args:?}: the refusal must name `{needle}`, got:\n{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{args:?} panicked:\n{stderr}");
    for path in must_not_exist {
        assert!(
            !path.exists(),
            "{args:?}: a refused run must not write {}",
            path.display()
        );
    }
}

#[test]
fn plan_refuses_a_zero_grid_as_a_usage_error() {
    for (args, needle) in [
        (vec!["plan", "0", "--height", "500"], "width"),
        (vec!["plan", "700", "--height", "0"], "height"),
        (
            vec!["plan", "700", "--height", "500", "--tile-size", "0"],
            "--tile-size",
        ),
        (
            vec!["plan", "700", "--height", "500", "--overlap", "256"],
            "--overlap",
        ),
        (
            vec!["plan", "700", "--height", "500", "--page", "0"],
            "--page",
        ),
        (
            vec!["plan", "700", "--height", "500", "--dpi", "0"],
            "--dpi",
        ),
    ] {
        assert_usage_refusal(&args, needle, &[]);
    }
}

#[test]
fn pyramid_refuses_out_of_range_values_before_reading_the_input() {
    // The input does not exist. A value check that ran after the input was
    // opened would answer "not found" with exit 1; a usage check answers
    // first, with 2, because the command line was wrong whatever the file is.
    let dir = unique_dir("pyramid-out-of-range");
    let missing = dir.join("missing.png");
    let archive = dir.join("out.pmtiles");
    let m = missing.to_str().unwrap();
    let a = archive.to_str().unwrap();
    for (extra, needle) in [
        (vec!["--tile-size", "0"], "--tile-size"),
        (vec!["--overlap", "256"], "--overlap"),
        (vec!["--tile-size", "64", "--overlap", "64"], "--overlap"),
        (vec!["--page", "0"], "--page"),
        (vec!["--dpi", "0"], "--dpi"),
        (vec!["--format", "jpeg", "--quality", "0"], "--quality"),
        (vec!["--format", "jpeg", "--quality", "101"], "--quality"),
    ] {
        let mut args = vec!["pyramid", m, a];
        args.extend_from_slice(&extra);
        assert_usage_refusal(&args, needle, &[&archive]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_refuses_a_quality_out_of_range_on_a_real_input() {
    // Today quality 101 is taken and an archive is written. Same refusal as
    // above, on an input that decodes, so the check cannot be an accident of
    // the missing file.
    let dir = unique_dir("pyramid-quality");
    let input = make_input(&dir, 64, 64);
    let archive = dir.join("out.pmtiles");
    for q in ["0", "101", "255"] {
        assert_usage_refusal(
            &[
                "pyramid",
                input.to_str().unwrap(),
                archive.to_str().unwrap(),
                "--format",
                "jpeg",
                "--quality",
                q,
            ],
            "--quality",
            &[&archive],
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_keeps_the_values_at_the_edge_of_the_range() {
    // The other side of the boundary: the smallest legal values still run.
    let dir = unique_dir("pyramid-edges");
    let input = make_input(&dir, 64, 64);
    let archive = dir.join("out.pmtiles");
    let out = run_bounded(
        &[
            "pyramid",
            input.to_str().unwrap(),
            archive.to_str().unwrap(),
            "--tile-size",
            "1",
            "--overlap",
            "0",
            "--format",
            "jpeg",
            "--quality",
            "1",
            "--page",
            "1",
            "--dpi",
            "1",
        ],
        120,
    );
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(archive.is_file());
    let out = run_bounded(
        &[
            "pyramid",
            input.to_str().unwrap(),
            dir.join("q100.pmtiles").to_str().unwrap(),
            "--format",
            "jpeg",
            "--quality",
            "100",
            "--tile-size",
            "64",
            "--overlap",
            "63",
        ],
        120,
    );
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_image_refuses_a_zero_dimension_as_a_usage_error() {
    let dir = unique_dir("test-image-zero");
    let png = dir.join("zero.png");
    for (extra, needle) in [
        (["--width", "0"], "--width"),
        (["--height", "0"], "--height"),
    ] {
        let mut args = vec!["test-image", png.to_str().unwrap()];
        args.extend_from_slice(&extra);
        assert_usage_refusal(&args, needle, &[&png]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// `viprs info --json` (#82)
// ---------------------------------------------------------------------------

/// The smallest PDF lopdf opens: one page with the given MediaBox, no images.
/// The xref offsets are computed rather than typed, so the file is valid
/// rather than repaired on load.
fn write_minimal_pdf(path: &std::path::Path, width_pts: u32, height_pts: u32) {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width_pts} {height_pts}] >>"),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(path, pdf).expect("the PDF must be writable");
}

fn info_json(path: &std::path::Path) -> serde_json::Value {
    let out = run(&["info", "--json", path.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "info --json stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "info --json must print one JSON object ({e}), got:\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn info_json_describes_an_image() {
    let dir = unique_dir("info-json-image");
    let png = make_input(&dir, 64, 48);
    let json = info_json(&png);
    assert_eq!(json["v"], 1, "{json}");
    assert_eq!(json["kind"], "image", "{json}");
    assert_eq!(json["path"], png.to_str().unwrap(), "{json}");
    assert_eq!(json["width"], 64, "{json}");
    assert_eq!(json["height"], 48, "{json}");
    assert_eq!(json["format"], "Rgb8", "{json}");
    // Exact, where the text output rounds to a tenth of a megabyte.
    assert_eq!(json["bytes"], 64 * 48 * 3, "{json}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn info_json_describes_a_pdf() {
    let dir = unique_dir("info-json-pdf");
    let pdf = dir.join("page.pdf");
    write_minimal_pdf(&pdf, 200, 100);
    let json = info_json(&pdf);
    assert_eq!(json["v"], 1, "{json}");
    assert_eq!(json["kind"], "pdf", "{json}");
    assert_eq!(json["path"], pdf.to_str().unwrap(), "{json}");
    assert_eq!(json["pages"], 1, "{json}");
    let sizes = json["page_sizes"]
        .as_array()
        .expect("page_sizes is an array");
    assert_eq!(sizes.len(), 1, "{json}");
    assert_eq!(sizes[0]["page"], 1, "{json}");
    assert_eq!(sizes[0]["width_pts"].as_f64(), Some(200.0), "{json}");
    assert_eq!(sizes[0]["height_pts"].as_f64(), Some(100.0), "{json}");
    assert_eq!(sizes[0]["has_images"], false, "{json}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn info_json_failure_prints_nothing_on_stdout_and_exits_1() {
    let dir = unique_dir("info-json-bad");
    let bad = dir.join("bad.png");
    std::fs::write(&bad, b"not an image").unwrap();
    for path in [bad.clone(), dir.join("missing.png")] {
        let out = run(&["info", "--json", path.to_str().unwrap()]);
        assert_eq!(
            code(&out),
            1,
            "stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stdout.is_empty(),
            "a failed info --json must leave stdout empty, got:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(!out.stderr.is_empty(), "the failure must say why on stderr");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `svgload -` reads the document from stdin, and a gzipped one gets the same
/// exit 1 and the same "gunzip it" refusal a `.svgz` path gets from every
/// other command, with nothing written (libviprs-cli#64's refusal, now on
/// `svgload` too).
#[test]
fn svgload_refuses_a_gzipped_document_on_stdin() {
    use std::io::Write as _;
    let dir = unique_dir("svgload-svgz-stdin");
    let out = dir.join("out.png");
    // Gzip magic, deflate, then a body the renderer would choke on as XML:
    // the refusal has to come from the magic, not from a parse error.
    let gz = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\x01\x00\x00\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00";
    let mut child = viprs()
        .args(["svgload", "-", out.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the viprs binary must be spawnable");
    child.stdin.take().unwrap().write_all(gz).unwrap();
    let got = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&got.stderr);
    assert_eq!(code(&got), 1, "stderr:\n{stderr}");
    assert!(
        stderr.contains("gzip") && stderr.contains("gunzip"),
        "expected the .svgz refusal, got:\n{stderr}"
    );
    assert!(!out.exists(), "a refused load wrote {}", out.display());
    let _ = std::fs::remove_dir_all(&dir);
}
