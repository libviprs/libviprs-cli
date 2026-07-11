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
