//! Cells for libviprs-cli#93: `viprs pyramid` reads the input's header before
//! it decodes, so an out-of-bounds `--region` and a run over `--memory-limit`
//! are refused without spending the decode.
//!
//! The proof that no decode happened is the input itself: every refusal cell
//! hands over a valid header with the pixel data cut off behind it, which the
//! decode can only fail on. A refusal that names the region or the memory
//! limit, with no `Decoding` line before it, can only have come from the
//! header.
//!
//! A container the core can't describe without decoding (GIF here) answers
//! `ProbeUnsupported`, and for those the run decodes first and checks after,
//! exactly as before; the fallback cells pin that.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
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

fn s(p: &Path) -> &str {
    p.to_str().expect("temp paths are UTF-8")
}

/// A temp directory of this cell's own (cargo runs cells as threads of one
/// process, so the pid alone would be shared).
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-probe93-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

/// A binary PPM header for `width` x `height` with a few bytes of pixels
/// behind it and nothing more.
fn truncated_ppm(path: &Path, width: u32, height: u32) {
    let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
    bytes.extend_from_slice(&[7; 12]);
    std::fs::write(path, bytes).expect("the PPM must be writable");
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG with a valid RGBA8 `IHDR` for `width` x `height` and an `IDAT` that
/// stops a few bytes in.
fn truncated_png(path: &Path, width: u32, height: u32) {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(&ihdr);
    bytes.extend_from_slice(&crc32(&ihdr).to_be_bytes());
    bytes.extend_from_slice(&1000u32.to_be_bytes());
    bytes.extend_from_slice(b"IDAT\x78\x9c\x00\x00");
    std::fs::write(path, bytes).expect("the PNG must be writable");
}

/// A baseline JPEG that ends right after its frame header: three
/// components, so the decode would give `Rgb8`.
fn truncated_jpeg(path: &Path, width: u16, height: u16) {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    std::fs::write(path, bytes).expect("the JPEG must be writable");
}

/// The classic 1x1 GIF89a, with its logical screen widened to `width` x
/// `height`. The core reads GIF only by decoding it.
fn gif(path: &Path, width: u16, height: u16) {
    let mut bytes = vec![
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xFF, 0xFF,
        0xFF, 0x00, 0x00, 0x00, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    ];
    bytes[6..8].copy_from_slice(&width.to_le_bytes());
    bytes[8..10].copy_from_slice(&height.to_le_bytes());
    std::fs::write(path, bytes).expect("the GIF must be writable");
}

/// The refusal came from the header: the decode never started.
fn assert_not_decoded(out: &Output) {
    let err = stderr(out);
    assert!(
        !err.contains("Decoding ") && !err.contains("Error decoding image"),
        "the input was decoded before the refusal:\n{err}"
    );
}

fn region_run(input: &Path, out: &Path, region: &str) -> Output {
    run(&[
        "pyramid",
        s(input),
        s(out),
        "--storage",
        "directory",
        "--region",
        region,
    ])
}

// ---------------------------------------------------------------------------
// --region: refused from the header with the usage error
// ---------------------------------------------------------------------------

#[test]
fn issue_93_an_out_of_bounds_region_on_a_ppm_is_refused_before_decoding() {
    let dir = unique_dir("ppm-region");
    let input = dir.join("cut.ppm");
    truncated_ppm(&input, 4000, 3000);
    let out = region_run(&input, &dir.join("out"), "3990,0,20,20");
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--region 3990,0,20,20 falls outside the 4000x3000 input"),
        "{}",
        stderr(&out)
    );
    assert_not_decoded(&out);
    assert!(!dir.join("out").exists(), "nothing may be written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn issue_93_an_out_of_bounds_region_on_a_png_is_refused_before_decoding() {
    let dir = unique_dir("png-region");
    let input = dir.join("cut.png");
    truncated_png(&input, 3000, 2000);
    let out = region_run(&input, &dir.join("out"), "0,1999,10,2");
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--region 0,1999,10,2 falls outside the 3000x2000 input"),
        "{}",
        stderr(&out)
    );
    assert_not_decoded(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn issue_93_an_out_of_bounds_region_on_a_jpeg_is_refused_before_decoding() {
    let dir = unique_dir("jpeg-region");
    let input = dir.join("cut.jpg");
    truncated_jpeg(&input, 640, 480);
    let out = region_run(&input, &dir.join("out"), "600,400,41,10");
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--region 600,400,41,10 falls outside the 640x480 input"),
        "{}",
        stderr(&out)
    );
    assert_not_decoded(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The header is only a precheck: a region that fits still goes to the
/// decode, and the decode's own error is what the person sees.
#[test]
fn issue_93_a_region_that_fits_still_decodes_and_reports_the_decode_error() {
    let dir = unique_dir("ppm-region-fits");
    let input = dir.join("cut.ppm");
    truncated_ppm(&input, 4000, 3000);
    let out = region_run(&input, &dir.join("out"), "0,0,64,64");
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Error decoding image"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An SVG over the decode's pixel ceiling still has a size the header gives,
/// and a region outside it is the person's mistake whatever the ceiling says.
#[cfg(feature = "svg")]
#[test]
fn issue_93_an_out_of_bounds_region_on_an_svg_is_refused_before_rendering() {
    let dir = unique_dir("svg-region");
    let input = dir.join("huge.svg");
    std::fs::write(
        &input,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40000" height="40000"><rect width="10" height="10"/></svg>"#,
    )
    .unwrap();
    let out = region_run(&input, &dir.join("out"), "0,0,50000,10");
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--region 0,0,50000,10 falls outside the 40000x40000 input"),
        "{}",
        stderr(&out)
    );
    assert_not_decoded(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --memory-limit: priced from the header
// ---------------------------------------------------------------------------

#[test]
fn issue_93_memory_limit_is_priced_from_the_header_before_decoding() {
    let dir = unique_dir("ppm-memory");
    let input = dir.join("cut.ppm");
    // 8000x8000 at the planner's four bytes a pixel, source plus its copy:
    // about 488 MB, far over 64.
    truncated_ppm(&input, 8000, 8000);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("out.pmtiles")),
        "--memory-limit",
        "64",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("exceeds --memory-limit (64 MB)"),
        "{}",
        stderr(&out)
    );
    assert_not_decoded(&out);
    assert!(!dir.join("out.pmtiles").exists(), "nothing may be written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn issue_93_memory_limit_on_a_jpeg_is_priced_from_the_header_before_decoding() {
    let dir = unique_dir("jpeg-memory");
    let input = dir.join("cut.jpg");
    truncated_jpeg(&input, 9000, 9000);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("out.pmtiles")),
        "--memory-limit",
        "100",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("exceeds --memory-limit (100 MB)"),
        "{}",
        stderr(&out)
    );
    assert_not_decoded(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The price is the region's, as it is after a decode, so a small region of
/// a huge header fits the limit and goes on to the decode.
#[test]
fn issue_93_the_header_price_is_the_regions_not_the_whole_image() {
    let dir = unique_dir("ppm-memory-region");
    let input = dir.join("cut.ppm");
    truncated_ppm(&input, 8000, 8000);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("out")),
        "--storage",
        "directory",
        "--region",
        "0,0,100,100",
        "--memory-limit",
        "1",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Error decoding image")
            && !stderr(&out).contains("exceeds --memory-limit"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A run that fits prints its estimate once, after the decode, as before.
#[test]
fn issue_93_a_run_that_fits_prints_one_memory_estimate() {
    let dir = unique_dir("ppm-fits");
    let input = dir.join("small.ppm");
    let mut bytes = b"P6\n64 64\n255\n".to_vec();
    bytes.resize(bytes.len() + 64 * 64 * 3, 90);
    std::fs::write(&input, bytes).unwrap();
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("out.pmtiles")),
        "--memory-limit",
        "64",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stderr(&out).matches("Memory estimate:").count(),
        1,
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// ProbeUnsupported: decode first, check after, as before
// ---------------------------------------------------------------------------

#[test]
fn issue_93_a_gif_region_is_still_checked_after_the_decode() {
    let dir = unique_dir("gif-region");
    let input = dir.join("one.gif");
    gif(&input, 1, 1);
    let out = region_run(&input, &dir.join("out"), "0,0,2,2");
    let err = stderr(&out);
    assert_eq!(code(&out), 2, "{err}");
    assert!(
        err.contains("--region 0,0,2,2 falls outside the 1x1 input"),
        "{err}"
    );
    assert!(
        err.contains("Decoding ") && err.contains("Source: 1x1"),
        "a GIF is decoded before the check:\n{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn issue_93_a_gif_is_still_priced_after_the_decode() {
    let dir = unique_dir("gif-memory");
    let input = dir.join("wide.gif");
    gif(&input, 2000, 2000);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("out.pmtiles")),
        "--memory-limit",
        "1",
    ]);
    let err = stderr(&out);
    assert_eq!(code(&out), 1, "{err}");
    assert!(err.contains("exceeds --memory-limit (1 MB)"), "{err}");
    assert!(
        err.contains("Decoding ") && err.contains("Source: 2000x2000"),
        "a GIF is decoded before it is priced:\n{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
