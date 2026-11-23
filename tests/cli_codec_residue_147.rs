//! EPIC CV (#147), CV1.5: the codec options the core has and `viprs` did not reach.
//!
//! * `radsave --exposure --aspect` (`radiance::SaveOptions`)
//! * `jxlload --page --n` (`decode_jxl_with`)
//! * `jp2ksave --ratio` (`jp2k::Compression::Lossy`)
//! * `pngsave --bitdepth 1` on a one-band image without `--palette` (`encode_png_bilevel`)
//! * `info --fields` (`Raster::get_fields`, the `vipsheader -a` view)
//!
//! Each cell reads the artifact back: a header line, a decoded sample, a field value.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn viprs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
}

fn run(args: &[&str]) -> Output {
    viprs().args(args).output().expect("viprs must spawn")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the process must exit normally")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[track_caller]
fn ok(args: &[&str]) -> Output {
    let out = run(args);
    assert_eq!(code(&out), 0, "{args:?}:\n{}", stderr(&out));
    out
}

#[track_caller]
fn exits(want: i32, args: &[&str], needle: &str) {
    let out = run(args);
    let err = stderr(&out);
    assert_eq!(code(&out), want, "{args:?}:\n{err}");
    assert!(err.contains(needle), "{args:?}: {needle:?} not in:\n{err}");
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-cv15-147-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rgb(dir: &Path, w: u32, h: u32) -> PathBuf {
    let path = dir.join(format!("rgb-{w}x{h}.png"));
    ok(&[
        "test-image",
        s(&path),
        "--width",
        &w.to_string(),
        "--height",
        &h.to_string(),
    ]);
    path
}

/// A binary PGM of one row with the given samples.
fn gray_row(dir: &Path, samples: &[u8]) -> PathBuf {
    let path = dir.join("row.pgm");
    let mut bytes = format!("P5\n{} 1\n255\n", samples.len()).into_bytes();
    bytes.extend_from_slice(samples);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The samples of a binary PGM, after its three header lines.
fn pgm_samples(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let mut newlines = 0;
    let start = bytes
        .iter()
        .position(|&b| {
            newlines += usize::from(b == b'\n');
            newlines == 3
        })
        .expect("a PGM header has three lines")
        + 1;
    bytes[start..].to_vec()
}

/// The Radiance header, up to the blank line that ends it.
fn radiance_header(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    let end = bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .expect("a Radiance header ends with a blank line");
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

// radsave

fn float_rgb(dir: &Path) -> PathBuf {
    let float = dir.join("float.v");
    ok(&["cast", s(&rgb(dir, 8, 8)), s(&float), "float"]);
    float
}

#[test]
fn radsave_writes_the_exposure_and_pixel_aspect_asked_for() {
    let dir = scratch("rad");
    let float = float_rgb(&dir);
    let out = dir.join("asked.hdr");
    ok(&[
        "radsave",
        s(&float),
        s(&out),
        "--exposure",
        "2.5",
        "--aspect",
        "1.25",
    ]);
    let header = radiance_header(&out);
    assert!(
        header.lines().any(|l| l == "EXPOSURE=2.500000e+00"),
        "{header}"
    );
    assert!(
        header.lines().any(|l| l == "PIXASPECT=1.250000"),
        "{header}"
    );

    let plain = dir.join("plain.hdr");
    ok(&["radsave", s(&float), s(&plain)]);
    let header = radiance_header(&plain);
    assert!(
        header.lines().any(|l| l == "EXPOSURE=1.000000e+00"),
        "{header}"
    );
    assert!(
        header.lines().any(|l| l == "PIXASPECT=1.000000"),
        "{header}"
    );
}

#[test]
fn radsave_refuses_an_exposure_or_aspect_that_is_not_positive() {
    let dir = scratch("rad-bad");
    let float = float_rgb(&dir);
    let out = dir.join("out.hdr");
    for (flag, bad) in [
        ("--exposure", "0"),
        ("--exposure", "-1"),
        ("--aspect", "0"),
        ("--aspect", "nan"),
    ] {
        exits(2, &["radsave", s(&float), s(&out), flag, bad], flag);
        assert!(!out.exists(), "{flag} {bad}: a usage error writes nothing");
    }
}

// jxlload

#[cfg(feature = "jxl")]
#[test]
fn jxlload_takes_page_and_n_and_the_core_refuses_a_page_a_still_does_not_have() {
    let dir = scratch("jxl");
    let still = dir.join("still.jxl");
    ok(&["jxlsave", s(&rgb(&dir, 16, 16)), s(&still), "--lossless"]);
    let plain = dir.join("plain.v");
    let paged = dir.join("paged.v");
    ok(&["jxlload", s(&still), s(&plain)]);
    ok(&["jxlload", s(&still), s(&paged), "--page", "0", "--n", "1"]);
    assert_eq!(
        std::fs::read(&plain).unwrap(),
        std::fs::read(&paged).unwrap(),
        "page 0, one page, is the whole still"
    );
    let beyond = dir.join("beyond.v");
    exits(
        1,
        &["jxlload", s(&still), s(&beyond), "--page", "1"],
        "page",
    );
    assert!(!beyond.exists());
}

#[cfg(not(feature = "jxl"))]
#[test]
fn jxlload_page_in_a_build_without_jxl_names_the_feature() {
    let dir = scratch("jxl-off");
    let input = dir.join("in.jxl");
    std::fs::write(&input, b"not read").unwrap();
    exits(
        1,
        &[
            "jxlload",
            s(&input),
            s(&dir.join("out.v")),
            "--page",
            "0",
            "--n",
            "1",
        ],
        "jxl",
    );
}

// jp2ksave

#[cfg(feature = "jp2k")]
#[test]
fn jp2ksave_ratio_writes_a_smaller_lossy_file_that_decodes() {
    let dir = scratch("jp2k");
    let input = rgb(&dir, 64, 64);
    let lossless = dir.join("lossless.jp2");
    let lossy = dir.join("lossy.jp2");
    ok(&["jp2ksave", s(&input), s(&lossless), "--lossless"]);
    ok(&["jp2ksave", s(&input), s(&lossy), "--ratio", "20"]);
    let (big, small) = (
        std::fs::metadata(&lossless).unwrap().len(),
        std::fs::metadata(&lossy).unwrap().len(),
    );
    assert!(
        small < big,
        "ratio 20 ({small} bytes) must be smaller than lossless ({big} bytes)"
    );
    let back = dir.join("back.v");
    ok(&["jp2kload", s(&lossy), s(&back)]);
    let info: serde_json::Value =
        serde_json::from_slice(&ok(&["info", s(&back), "--json"]).stdout).unwrap();
    assert_eq!(
        (info["width"].as_u64(), info["height"].as_u64()),
        (Some(64), Some(64))
    );
}

#[cfg(not(feature = "jp2k"))]
#[test]
fn jp2ksave_ratio_in_a_build_without_jp2k_names_the_feature() {
    let dir = scratch("jp2k-off");
    exits(
        1,
        &[
            "jp2ksave",
            s(&rgb(&dir, 8, 8)),
            s(&dir.join("out.jp2")),
            "--ratio",
            "20",
        ],
        "jp2k",
    );
}

#[test]
fn jp2ksave_takes_lossless_or_a_ratio_never_both_and_never_zero() {
    let dir = scratch("jp2k-usage");
    let input = rgb(&dir, 8, 8);
    let out = dir.join("out.jp2");
    exits(
        2,
        &[
            "jp2ksave",
            s(&input),
            s(&out),
            "--lossless",
            "--ratio",
            "20",
        ],
        "--ratio",
    );
    exits(
        2,
        &["jp2ksave", s(&input), s(&out), "--ratio", "0"],
        "--ratio",
    );
    exits(2, &["jp2ksave", s(&input), s(&out)], "--lossless");
    assert!(!out.exists());
}

// pngsave --bitdepth 1

#[test]
fn pngsave_bitdepth_1_on_one_band_writes_a_bilevel_png_thresholded_at_128() {
    let dir = scratch("png1");
    let input = gray_row(&dir, &[0, 100, 127, 128, 200, 255, 3, 129, 64]);
    let out = dir.join("bilevel.png");
    ok(&["pngsave", s(&input), s(&out), "--bitdepth", "1"]);
    let png = std::fs::read(&out).unwrap();
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(
        (png[24], png[25]),
        (1, 0),
        "bit depth 1, colour type 0 (gray)"
    );
    let back = dir.join("back.pgm");
    ok(&["pngload", s(&out), s(&back)]);
    assert_eq!(pgm_samples(&back), [0, 0, 0, 255, 255, 255, 0, 255, 0]);
}

#[test]
fn pngsave_bitdepth_1_on_colour_without_a_palette_is_refused() {
    let dir = scratch("png1-rgb");
    let out = dir.join("out.png");
    exits(
        1,
        &["pngsave", s(&rgb(&dir, 8, 8)), s(&out), "--bitdepth", "1"],
        "one band",
    );
    assert!(!out.exists());
}

// info --fields

#[test]
fn info_fields_lists_the_header_fields_with_their_values() {
    let dir = scratch("fields");
    let input = rgb(&dir, 70, 30);
    let text = stdout(&ok(&["info", s(&input), "--fields"]));
    for line in [
        "width: 70",
        "height: 30",
        "bands: 3",
        "xoffset: 0",
        "yoffset: 0",
    ] {
        assert!(
            text.lines().any(|l| l.trim() == line),
            "{line:?} not in:\n{text}"
        );
    }
    let names: Vec<&str> = text
        .lines()
        .filter_map(|l| l.split_once(':').map(|(name, _)| name.trim()))
        .collect();
    let at = |n: &str| names.iter().position(|x| *x == n);
    assert!(
        at("width") < at("height") && at("height") < at("bands"),
        "libvips order:\n{text}"
    );

    let v: serde_json::Value =
        serde_json::from_slice(&ok(&["info", s(&input), "--fields", "--json"]).stdout).unwrap();
    assert_eq!(v["v"], 1);
    assert_eq!(v["fields"]["width"], 70);
    assert_eq!(v["fields"]["height"], 30);
    assert_eq!(v["fields"]["bands"], 3);
}
