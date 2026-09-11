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
fn plan_numeric_width_without_height_exits_1() {
    // A numeric width with no --height is a resolvable user error, exit 1.
    let out = run(&["plan", "1024"]);
    assert_eq!(code(&out), 1);
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
    let target = dir.join("tiles");
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
    // move the default layout as well as the default sink. Proven by producing
    // the same archive twice, once by default and once with --layout xyz
    // spelled out: if the default were still deep-zoom the two would differ.
    let dir = unique_dir("default-layout-xyz");
    let png = make_input(&dir, 700, 500);

    let implicit = dir.join("implicit.pmtiles");
    let a = run(&[
        "pyramid",
        png.to_str().unwrap(),
        implicit.to_str().unwrap(),
    ]);
    assert_eq!(
        code(&a),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&a.stderr)
    );

    let explicit = dir.join("explicit.pmtiles");
    let b = run(&[
        "pyramid",
        png.to_str().unwrap(),
        explicit.to_str().unwrap(),
        "--layout",
        "xyz",
    ]);
    assert_eq!(
        code(&b),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&b.stderr)
    );

    let left = read_archive(&implicit);
    let right = read_archive(&explicit);
    assert_eq!(
        left.len(),
        right.len(),
        "the implicit and explicit xyz archives must be the same size"
    );
    assert_eq!(
        left, right,
        "the default layout under --storage pmtiles must be xyz byte for byte"
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
    let files = collect_tree(&tree);
    assert!(
        files.iter().any(|(p, _)| p.ends_with(".dzi")),
        "a deep-zoom tree carries its .dzi manifest, got {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
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
fn pyramid_format_choices_do_not_advertise_webp() {
    // Core's `TileFormat` has no WebP variant, so no surface here can produce a
    // WebP tile. PMTiles can *store* one, and `pmtiles info` reports it when a
    // foreign archive carries it, but this CLI must not offer what it cannot
    // encode. This test is what stops "PMTiles supports WebP" leaking into the
    // flag as a documentation claim.
    let out = run(&["pyramid", "--help"]);
    assert_eq!(code(&out), 0);
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.contains("png") && help.contains("jpeg") && help.contains("raw"),
        "the three encodable formats must be listed, got:\n{help}"
    );

    let dir = unique_dir("webp-refusal");
    let png = make_input(&dir, 64, 64);
    let attempt = run(&[
        "pyramid",
        png.to_str().unwrap(),
        dir.join("a.pmtiles").to_str().unwrap(),
        "--format",
        "webp",
    ]);
    assert_eq!(
        code(&attempt),
        2,
        "--format webp must be a usage error while no encoder exists"
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
        expected.iter().map(|(z, x, y, _)| (*z, *x, *y)).collect::<Vec<_>>(),
        actual.iter().map(|(z, x, y, _)| (*z, *x, *y)).collect::<Vec<_>>(),
        "extract must reproduce exactly the generated coordinate set"
    );
    assert_eq!(expected, actual, "extract must reproduce the tile bytes too");

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
fn pmtiles_help_lists_the_four_subcommands() {
    let out = run(&["pmtiles", "--help"]);
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for name in ["info", "tile", "verify", "extract"] {
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
