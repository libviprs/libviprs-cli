//! End-to-end cells for `viprs pdf`, `viprs geo` and the `viprs plan` query
//! flags (#68): the parse-time refusals and the layout and password surface.
//!
//! Like `cli_e2e.rs` these stay off the native pdfium runtime. Every refusal
//! here happens while the arguments are parsed, before a PDF is opened, so the
//! PDF they name only has to exist. The password cells that need a real
//! encrypted file and a libpdfium live in libviprs-tests
//! (`tests/cli_pdf_geo_plan.rs`), where CI provides one.

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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A temp directory of this cell's own (cargo runs cells as threads of one
/// process, so the pid alone would be shared).
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-pgp-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

/// A file called `in.pdf`. Its bytes do not matter: every cell using it
/// expects a refusal before the file is read.
fn dummy_pdf(dir: &Path) -> PathBuf {
    let path = dir.join("in.pdf");
    std::fs::write(&path, b"not read").expect("the dummy input must be writable");
    path
}

/// A small PNG through `viprs test-image`, for the `pyramid` cells.
fn png_input(dir: &Path) -> PathBuf {
    let png = dir.join("gen.png");
    let out = run(&[
        "test-image",
        png.to_str().unwrap(),
        "--width",
        "64",
        "--height",
        "64",
    ]);
    assert_eq!(code(&out), 0, "test-image stderr:\n{}", stderr(&out));
    png
}

fn assert_usage_error(out: &Output, what: &str) {
    assert_eq!(
        code(out),
        2,
        "{what} must be a usage error (exit 2)\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

// ---------------------------------------------------------------------------
// viprs plan --dzi-manifest
// ---------------------------------------------------------------------------

#[test]
fn dzi_manifest_before_the_input_does_not_swallow_it() {
    // `--dzi-manifest` takes an optional format. Without `=` it used to take
    // the next word, so the input became the format and `plan` had no input.
    let out = run(&["plan", "--dzi-manifest", "5000", "--height", "3000"]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let xml = stdout(&out);
    assert!(
        xml.contains("<Image"),
        "expected a .dzi manifest, got:\n{xml}"
    );
    assert!(
        xml.contains("Format=\"png\""),
        "a bare --dzi-manifest means png, got:\n{xml}"
    );
}

#[test]
fn dzi_manifest_takes_its_format_after_an_equals_sign() {
    let out = run(&["plan", "5000", "--height", "3000", "--dzi-manifest=jpg"]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(
        stdout(&out).contains("Format=\"jpg\""),
        "got:\n{}",
        stdout(&out)
    );
}

// ---------------------------------------------------------------------------
// --layout: zoomify and iiif are plan-only
// ---------------------------------------------------------------------------

#[test]
fn pyramid_layout_does_not_take_the_plan_only_layouts() {
    // `plan` answers sidecar questions for zoomify and iiif, but `pyramid`
    // has no cells for writing them and an archive cannot hold them, so its
    // --layout stays deep-zoom, xyz or google, as the README says.
    let dir = unique_dir("pyramid-layout");
    let input = png_input(&dir);
    for layout in ["zoomify", "iiif"] {
        let target = dir.join(format!("{layout}.pmtiles"));
        let out = run(&[
            "pyramid",
            input.to_str().unwrap(),
            target.to_str().unwrap(),
            "--layout",
            layout,
        ]);
        assert_usage_error(&out, &format!("pyramid --layout {layout}"));
        assert!(
            stderr(&out).contains("invalid value"),
            "clap should name the bad value, got:\n{}",
            stderr(&out)
        );
        assert!(!target.exists(), "nothing may be written for a usage error");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plan_still_answers_for_zoomify_and_iiif() {
    let out = run(&[
        "plan",
        "5000",
        "--height",
        "3000",
        "--layout",
        "zoomify",
        "--properties-sidecar",
        "png",
    ]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("ImageProperties.xml"),
        "got:\n{}",
        stdout(&out)
    );

    let out = run(&[
        "plan",
        "5000",
        "--height",
        "3000",
        "--layout",
        "iiif",
        "--properties-sidecar",
        "png",
    ]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("info.json"),
        "got:\n{}",
        stdout(&out)
    );
}

// ---------------------------------------------------------------------------
// Numbers that are not numbers: NaN, infinities, out-of-range channels
// ---------------------------------------------------------------------------

#[test]
fn background_channels_outside_0_to_255_are_a_usage_error() {
    let dir = unique_dir("bg-range");
    let input = dummy_pdf(&dir);
    let output = dir.join("out.png");
    for bg in ["256,0,0", "0,-1,0", "0,0,0,300"] {
        let out = run(&[
            "pdf",
            "extract",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--background",
            bg,
        ]);
        assert_usage_error(&out, &format!("--background {bg}"));
        assert!(!output.exists(), "nothing may be written for a usage error");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_finite_background_channels_are_a_usage_error() {
    let dir = unique_dir("bg-finite");
    let input = dummy_pdf(&dir);
    let output = dir.join("out.png");
    for bg in ["nan,0,0", "inf,0,0", "0,0,-inf", "1e400,0,0"] {
        let out = run(&[
            "pdf",
            "extract",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--background",
            bg,
        ]);
        assert_usage_error(&out, &format!("--background {bg}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_finite_affine_coefficients_are_a_usage_error() {
    // These used to print NaN,NaN (or inf) and exit 0, and 1e200 tripped a
    // debug assertion in core.
    for affine in [
        "NaN,0,0,0,1,0",
        "1,0,inf,0,1,0",
        "1e400,0,0,0,1,0",
        "1,0,0,0,1,-infinity",
    ] {
        let out = run(&["geo", "pixel-to-geo", "1", "2", "--affine", affine]);
        assert_usage_error(&out, &format!("--affine {affine}"));
    }
}

#[test]
fn non_finite_geo_origin_or_scale_is_a_usage_error() {
    for (origin, scale) in [("nan,0", "1,1"), ("0,0", "inf,1"), ("0,1e999", "1,1")] {
        let out = run(&[
            "geo",
            "pixel-to-geo",
            "1",
            "2",
            "--geo-origin",
            origin,
            "--geo-scale",
            scale,
        ]);
        assert_usage_error(&out, &format!("--geo-origin {origin} --geo-scale {scale}"));
    }
}

#[test]
fn non_finite_geo_coordinates_are_a_usage_error() {
    for (x, y) in [("NaN", "2"), ("1", "inf"), ("-inf", "0")] {
        let out = run(&[
            "geo",
            "pixel-to-geo",
            x,
            y,
            "--geo-origin",
            "0,0",
            "--geo-scale",
            "1,1",
        ]);
        assert_usage_error(&out, &format!("pixel-to-geo {x} {y}"));
        let out = run(&[
            "geo",
            "geo-to-pixel",
            x,
            y,
            "--geo-origin",
            "0,0",
            "--geo-scale",
            "1,1",
        ]);
        assert_usage_error(&out, &format!("geo-to-pixel {x} {y}"));
    }
}

#[test]
fn pyramid_geo_origin_is_parsed_like_geo_s_and_exits_2() {
    // One parser for the pair, so a bad --geo-origin is the same usage error
    // (exit 2) on `pyramid` as on `geo`. `pyramid` used to exit 1.
    let dir = unique_dir("pyramid-geo");
    let input = png_input(&dir);
    for (origin, scale) in [
        ("abc,1", "1,1"),
        ("1", "1,1"),
        ("nan,0", "1,1"),
        ("0,0", "1,inf"),
    ] {
        let target = dir.join("tiles");
        let out = run(&[
            "pyramid",
            input.to_str().unwrap(),
            target.to_str().unwrap(),
            "--storage",
            "directory",
            "--geo-origin",
            origin,
            "--geo-scale",
            scale,
        ]);
        assert_usage_error(
            &out,
            &format!("pyramid --geo-origin {origin} --geo-scale {scale}"),
        );
        let _ = std::fs::remove_dir_all(&target);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Passwords: not only on argv
// ---------------------------------------------------------------------------

#[test]
fn password_help_names_the_sources_that_stay_off_argv() {
    for sub in ["info", "extract"] {
        let out = run(&["pdf", sub, "--help"]);
        assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
        let help = stdout(&out);
        for needle in ["--password-file", "VIPRS_PDF_PASSWORD", "ps"] {
            assert!(
                help.contains(needle),
                "`pdf {sub} --help` should mention {needle}, got:\n{help}"
            );
        }
    }
}

#[test]
fn password_and_password_file_cannot_both_be_given() {
    let dir = unique_dir("pw-conflict");
    let input = dummy_pdf(&dir);
    let file = dir.join("pw.txt");
    std::fs::write(&file, "secret\n").unwrap();
    let out = run(&[
        "pdf",
        "info",
        input.to_str().unwrap(),
        "--password",
        "secret",
        "--password-file",
        file.to_str().unwrap(),
    ]);
    assert_usage_error(&out, "--password with --password-file");
    assert!(
        stderr(&out).contains("cannot be used with"),
        "clap should report the conflict, got:\n{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_password_file_that_cannot_be_read_exits_1_and_names_it() {
    let dir = unique_dir("pw-missing");
    let input = dummy_pdf(&dir);
    let missing = dir.join("no-such-password-file");
    let out = run(&[
        "pdf",
        "info",
        input.to_str().unwrap(),
        "--password-file",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1, "stderr:\n{}", stderr(&out));
    assert!(
        stderr(&out).contains("no-such-password-file"),
        "the message should name the file, got:\n{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_password_file_with_render_options_is_a_usage_error() {
    // The render paths take no password, so a password from a file cannot
    // combine with them any more than --password can.
    let dir = unique_dir("pw-render");
    let input = dummy_pdf(&dir);
    let file = dir.join("pw.txt");
    std::fs::write(&file, "secret\n").unwrap();
    let output = dir.join("out.png");
    for render in [["--dpi", "150"], ["--background", "255,0,0"]] {
        let out = run(&[
            "pdf",
            "extract",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--password-file",
            file.to_str().unwrap(),
            render[0],
            render[1],
        ]);
        assert_usage_error(&out, &format!("--password-file with {}", render[0]));
        assert!(
            stderr(&out).contains("cannot be used with"),
            "clap should report the conflict, got:\n{}",
            stderr(&out)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
