//! libviprs-cli#151 (CV2.2), `viprs pvec convert` for a PDF page, the real binary: the page choice, the
//! image limits and the `jbig2` feature. Every cell reads what the conversion wrote (the report beside
//! the artifact, PVec's reader over it), since an exit code alone is not evidence. The PDFs are built
//! here, byte by byte, and are tiny: nothing from anyone's drawings is in this file.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
use serde_json::Value;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("viprs must spawn")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the process must exit normally")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-pdf-151-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// One image XObject: its dictionary entries (beyond type, subtype, size) and its stream bytes.
struct Image {
    width: u32,
    height: u32,
    entries: &'static str,
    data: Vec<u8>,
}

/// A PDF whose pages draw `pages[n]` as their content stream. Every image is in every page's
/// resources as `/Im0`, `/Im1` and so on, so a content stream paints it with `/ImN Do`.
fn pdf(pages: &[&str], images: &[Image]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    let stream = |dict: &str, data: &[u8]| {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        body
    };
    // 1 catalog, 2 pages, 3 resources, then a page and its content per page, then the images.
    let first_image = 4 + 2 * pages.len();
    let kids: Vec<String> = (0..pages.len())
        .map(|n| format!("{} 0 R", 4 + 2 * n))
        .collect();
    object(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(
        &mut out,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );
    let xobjects: Vec<String> = (0..images.len())
        .map(|n| format!("/Im{n} {} 0 R", first_image + n))
        .collect();
    object(
        &mut out,
        format!("<< /XObject << {} >> >>", xobjects.join(" ")).as_bytes(),
    );
    for (n, content) in pages.iter().enumerate() {
        object(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources 3 0 R /Contents {} 0 R >>",
                5 + 2 * n
            )
            .as_bytes(),
        );
        object(&mut out, &stream("", content.as_bytes()));
    }
    for image in images {
        let dict = format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} {}",
            image.width, image.height, image.entries
        );
        object(&mut out, &stream(&dict, &image.data));
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for at in &offsets {
        out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// An 8 by 8 gray picture, one byte a pixel.
fn gray() -> Image {
    Image {
        width: 8,
        height: 8,
        entries: "/ColorSpace /DeviceGray /BitsPerComponent 8",
        data: (0..64).map(|n| (n * 4) as u8).collect(),
    }
}

const LINE: &str = "10 10 m 200 10 l S";
const PAINT: &str = "q 80 0 0 80 100 100 cm /Im0 Do Q";

fn write_pdf(dir: &Path, bytes: &[u8]) -> PathBuf {
    let path = dir.join("page.pdf");
    std::fs::write(&path, bytes).unwrap();
    path
}

/// `pvec convert` over a PDF into `dir/out`, plus `extra`: the process, the output directory and
/// the report if one was written.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
fn convert(bytes: &[u8], tag: &str, extra: &[&str]) -> (Output, PathBuf, Value) {
    let dir = scratch(tag);
    let pdf = write_pdf(&dir, bytes);
    let out_dir = dir.join("out");
    let mut args = vec!["pvec", "convert", s(&pdf), s(&out_dir)];
    args.extend_from_slice(extra);
    let out = run(&args);
    let report = std::fs::read(out_dir.join("page.pvec.report.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    (out, out_dir, report)
}

#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
fn info(file: &Path) -> Value {
    let out = run(&["pvec", "info", s(file), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    serde_json::from_slice(&out.stdout).expect("info --json is JSON")
}

#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
fn cleanup(out_dir: &Path) {
    let _ = std::fs::remove_dir_all(out_dir.parent().unwrap());
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// A PDF page becomes `<stem>.pvec` and its report, and PVec's own reader reads the vector in it.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_pdf_page_converts_to_a_pvec_and_its_report() {
    let (out, dir, report) = convert(&pdf(&[LINE], &[]), "page", &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(listing(&dir), ["page.pvec", "page.pvec.report.json"]);
    assert_eq!(report["pdf"]["page"], 1, "{report}");
    assert_eq!(info(&dir.join("page.pvec"))["entities"], 1);
    // The point is the unit, and the artifact says so: the scale is a calibration, not the manifest's.
    assert_eq!(
        info(&dir.join("page.pvec"))["scale"]["provenance"],
        "declared-unit"
    );
    let valid = run(&["pvec", "validate", s(&dir.join("page.pvec"))]);
    assert_eq!(code(&valid), 0, "{}", stderr(&valid));
    cleanup(&dir);
}

/// `--page` picks the page, 1-based, and the stem names the files.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn page_picks_the_page_and_stem_names_the_files() {
    let two = pdf(&[LINE, "10 10 m 200 10 l S 10 20 m 200 20 l S"], &[]);
    let (out, dir, _) = convert(&two, "page-2", &["--page", "2", "--stem", "second"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(listing(&dir), ["second.pvec", "second.pvec.report.json"]);
    assert_eq!(info(&dir.join("second.pvec"))["entities"], 2);
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.join("second.pvec.report.json")).unwrap())
            .unwrap();
    assert_eq!(report["pdf"]["page"], 2, "{report}");
    cleanup(&dir);
}

/// A page the document does not have is a failure (exit 1) that leaves nothing behind, and the
/// directory the run made goes again.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_page_past_the_end_fails_and_leaves_nothing() {
    let (out, dir, _) = convert(&pdf(&[LINE], &[]), "page-3", &["--page", "3"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("page"), "{}", stderr(&out));
    assert!(!dir.exists(), "{:?}", listing(&dir));
    cleanup(&dir);
}

/// A page with no path has nothing PVec can hold, which is the core's own refusal.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_page_with_no_path_is_refused() {
    let (out, dir, _) = convert(&pdf(&["BT ET"], &[]), "no-path", &[]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("no vector path"), "{}", stderr(&out));
    assert!(!dir.exists());
    cleanup(&dir);
}

/// Not a PDF at all, under a `.pdf` name, is a failure the core names, not a DWG error.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn a_file_that_is_not_a_pdf_fails_as_a_pdf() {
    let (out, dir, _) = convert(b"this is not a pdf", "not-a-pdf", &[]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("page could not be extracted"),
        "{}",
        stderr(&out)
    );
    assert!(!dir.exists());
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// Image limits
// ---------------------------------------------------------------------------

/// With no limit given the page's image is baked into a raster block.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn an_image_is_baked_by_default() {
    let (out, dir, report) = convert(&pdf(&[&format!("{LINE} {PAINT}")], &[gray()]), "img", &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["images"], 1, "{report}");
    assert_eq!(report["pdf"]["rasters"], 1, "{report}");
    assert_eq!(report["pdf"]["images_baked"], true, "{report}");
    assert_eq!(info(&dir.join("page.pvec"))["rasters"], 1);
    cleanup(&dir);
}

/// More images than `--max-images` is refused by the extraction, and the message names the limit.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn max_images_refuses_a_page_with_more_images() {
    let two = pdf(
        &[&format!("{LINE} {PAINT} q 80 0 0 80 300 100 cm /Im1 Do Q")],
        &[gray(), gray()],
    );
    let (out, dir, _) = convert(&two, "max-images-1", &["--max-images", "1"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("max_images"), "{}", stderr(&out));
    cleanup(&dir);
    let (out, dir, report) = convert(&two, "max-images-2", &["--max-images", "2"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["rasters"], 2, "{report}");
    cleanup(&dir);
}

/// `--max-image-bytes` is the page's total of encoded image bytes; 64 bytes of picture is over 10.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn max_image_bytes_refuses_a_page_with_more_image_data() {
    let page = pdf(&[&format!("{LINE} {PAINT}")], &[gray()]);
    let (out, dir, _) = convert(&page, "max-bytes", &["--max-image-bytes", "10"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("max_image_bytes"), "{}", stderr(&out));
    cleanup(&dir);
}

/// `--max-image-stream-bytes` refuses one stream and the page still converts, without that image.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn max_image_stream_bytes_leaves_one_image_out_and_converts_the_page() {
    let page = pdf(&[&format!("{LINE} {PAINT}")], &[gray()]);
    let (out, dir, report) = convert(&page, "max-stream", &["--max-image-stream-bytes", "10"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["rasters"], 0, "{report}");
    assert_eq!(report["pdf"]["images_baked"], false, "{report}");
    cleanup(&dir);
}

/// `--max-pixels-per-image` refuses a picture over it, by name in the report, and the page converts.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn max_pixels_per_image_refuses_a_big_picture_in_the_report() {
    let page = pdf(&[&format!("{LINE} {PAINT}")], &[gray()]);
    let (out, dir, report) = convert(&page, "max-px", &["--max-pixels-per-image", "4"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["rasters"], 0, "{report}");
    assert!(
        report["pdf"]["images_refused"]
            .as_object()
            .is_some_and(|m| !m.is_empty()),
        "{report}"
    );
    cleanup(&dir);
}

/// `--max-image-pixels` is the page's total: over it the bake is skipped and the report says why.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
#[test]
fn max_image_pixels_skips_the_bake_for_a_page_over_it() {
    let page = pdf(&[&format!("{LINE} {PAINT}")], &[gray()]);
    let (out, dir, report) = convert(&page, "max-total", &["--max-image-pixels", "1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["rasters"], 0, "{report}");
    assert_eq!(
        report["pdf"]["images_not_baked_reason"], "max-image-pixels",
        "{report}"
    );
    cleanup(&dir);
}

/// A limit of 0 is a usage error, and so is a page 0.
#[cfg(feature = "pvec-convert")]
#[test]
fn zero_is_a_usage_error_for_the_limits_and_the_page() {
    for flag in [
        "--max-images",
        "--max-image-bytes",
        "--max-image-stream-bytes",
        "--max-image-pixels",
        "--max-pixels-per-image",
        "--page",
    ] {
        let dir = scratch("zero");
        let pdf = write_pdf(&dir, &pdf(&[LINE], &[]));
        let out_dir = dir.join("out");
        let out = run(&["pvec", "convert", s(&pdf), s(&out_dir), flag, "0"]);
        assert_eq!(code(&out), 2, "{flag}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains("unexpected argument"),
            "{flag}: {}",
            stderr(&out)
        );
        assert!(stderr(&out).contains(flag), "{flag}: {}", stderr(&out));
        assert!(!out_dir.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}

// ---------------------------------------------------------------------------
// What belongs to which input
// ---------------------------------------------------------------------------

/// A flag that only a DWG has is a usage error on a PDF, before anything is read or written.
#[cfg(feature = "pvec-convert")]
#[test]
fn dwg_flags_are_usage_errors_on_a_pdf() {
    let cases: [&[&str]; 22] = [
        &["--layout", "A-101"],
        &["--model"],
        &["--unit", "m"],
        &["--geometry-only"],
        &["--regions"],
        &["--annotations"],
        &["--thumbnail", "64"],
        &["--profile", "strict"],
        &["--capability-policy", "downgrade"],
        &["--allow-capability", "5"],
        &["--scale", "0.5"],
        &["--off-layers", "draw"],
        &["--layer-hidden", "X"],
        &["--max-block-depth", "3"],
        &["--max-entities", "3"],
        &["--leaf-capacity", "8"],
        &["--spill-dir", "x", "--spill-budget", "10"],
        &["--regions", "--region-gap", "1"],
        &["--max-insert-depth", "3"],
        &["--no-plot-layers", "draw"],
        &["--plan-scale", "1:2", "--paper-unit", "1"],
        &["--scale", "0.5", "--georef-sidecar", "x"],
    ];
    for extra in cases {
        let dir = scratch("dwg-flags");
        let pdf = write_pdf(&dir, &pdf(&[LINE], &[]));
        let out_dir = dir.join("out");
        let mut args = vec!["pvec", "convert", s(&pdf), s(&out_dir)];
        args.extend_from_slice(extra);
        let out = run(&args);
        assert_eq!(code(&out), 2, "{extra:?}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains("unexpected argument"),
            "{extra:?} is not a flag yet: {}",
            stderr(&out)
        );
        assert!(stderr(&out).contains("DWG"), "{extra:?}: {}", stderr(&out));
        for flag in extra.iter().filter(|a| a.starts_with("--")) {
            assert!(
                stderr(&out).contains(flag),
                "{flag} in {extra:?}: {}",
                stderr(&out)
            );
        }
        assert!(!out_dir.exists(), "{extra:?} wrote something");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A flag that only a PDF has is a usage error on a DWG, and nothing is written.
#[cfg(feature = "pvec-convert")]
#[test]
fn pdf_flags_are_usage_errors_on_a_dwg() {
    let drawing = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vector/vc_layers.dwg");
    let cases: [&[&str]; 6] = [
        &["--page", "2"],
        &["--max-images", "3"],
        &["--max-image-bytes", "3"],
        &["--max-image-stream-bytes", "3"],
        &["--max-image-pixels", "3"],
        &["--max-pixels-per-image", "3"],
    ];
    for extra in cases {
        let dir = scratch("pdf-flags");
        let out_dir = dir.join("out");
        let mut args = vec![
            "pvec",
            "convert",
            s(&drawing),
            s(&out_dir),
            "--model",
            "--unit",
            "m",
        ];
        args.extend_from_slice(extra);
        let out = run(&args);
        assert_eq!(code(&out), 2, "{extra:?}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains("unexpected argument"),
            "{extra:?} is not a flag yet: {}",
            stderr(&out)
        );
        assert!(stderr(&out).contains("PDF"), "{extra:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(extra[0]),
            "{extra:?}: {}",
            stderr(&out)
        );
        assert!(!out_dir.exists(), "{extra:?} wrote something");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A build with the DWG converter but not PVec's raster half cannot write a PDF page, and says which
/// feature to rebuild with.
#[cfg(all(feature = "pvec-convert", not(feature = "pvec-raster")))]
#[test]
fn a_build_without_pvec_raster_names_it_for_a_pdf() {
    let dir = scratch("no-raster");
    let pdf = write_pdf(&dir, &pdf(&[LINE], &[]));
    let out_dir = dir.join("out");
    let out = run(&["pvec", "convert", s(&pdf), s(&out_dir)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-raster"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// JBIG2
// ---------------------------------------------------------------------------

/// The stream as a PDF holds it (T.88 Annex D.3): the segments of the file, with the file header and
/// the end-of-file segment dropped.
fn jbig2_embedded() -> Vec<u8> {
    let file = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jbig2/generic_template0.jbig2"),
    )
    .unwrap();
    assert_eq!(&file[..8], b"\x97JB2\r\n\x1a\n", "a JBIG2 file header");
    assert_eq!(file[8] & 1, 1, "sequential organisation");
    let mut at = if file[8] & 2 == 0 { 13 } else { 9 };
    let mut out = Vec::new();
    while at < file.len() {
        let start = at;
        let number = u32::from_be_bytes(file[at..at + 4].try_into().unwrap());
        let flags = file[at + 4];
        at += 5;
        let refs = (file[at] >> 5) as usize;
        assert!(refs < 5, "this fixture has no long reference form");
        at += 1 + refs * if number <= 256 { 1 } else { 2 };
        at += if flags & 0x40 != 0 { 4 } else { 1 };
        let len = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        at += 4 + len;
        if flags & 0x3f != 51 {
            out.extend_from_slice(&file[start..at]);
        }
    }
    out
}

/// A page with one JBIG2 picture, 163 by 109 pixels, and one line.
fn jbig2_page() -> Vec<u8> {
    pdf(
        &[&format!("{LINE} q 163 0 0 109 100 100 cm /Im0 Do Q")],
        &[Image {
            width: 163,
            height: 109,
            entries: "/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode",
            data: jbig2_embedded(),
        }],
    )
}

/// Without the `jbig2` feature a page that needs the decoder exits 1 and names the feature, and no
/// file is written: the artifact is not left behind to look like a success.
#[cfg(all(
    feature = "pvec-convert",
    feature = "pvec-raster",
    not(feature = "jbig2")
))]
#[test]
fn a_build_without_jbig2_refuses_a_page_that_needs_it() {
    let dir = scratch("jbig2-off");
    let pdf = write_pdf(&dir, &jbig2_page());
    let out_dir = dir.join("out");
    std::fs::create_dir(&out_dir).unwrap();
    let out = run(&["pvec", "convert", s(&pdf), s(&out_dir)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("jbig2"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--features jbig2"),
        "{}",
        stderr(&out)
    );
    assert_eq!(listing(&out_dir), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(dir);
}

/// A build without `jbig2` still converts a page whose images are not JBIG2.
#[cfg(all(
    feature = "pvec-convert",
    feature = "pvec-raster",
    not(feature = "jbig2")
))]
#[test]
fn a_build_without_jbig2_still_converts_other_images() {
    let (out, dir, report) = convert(
        &pdf(&[&format!("{LINE} {PAINT}")], &[gray()]),
        "jbig2-other",
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["rasters"], 1, "{report}");
    cleanup(&dir);
}

/// With the feature the stream decodes and is baked as a one-bit raster, and nothing is refused.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster", feature = "jbig2"))]
#[test]
fn a_build_with_jbig2_decodes_the_stream() {
    let (out, dir, report) = convert(&jbig2_page(), "jbig2-on", &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(report["pdf"]["images"], 1, "{report}");
    assert_eq!(report["pdf"]["rasters"], 1, "{report}");
    assert_eq!(
        report["pdf"]["images_refused"],
        serde_json::json!({}),
        "{report}"
    );
    assert_eq!(
        report["pdf"]["image_notes"]["bilevel-1-bit-tiles"], 1,
        "{report}"
    );
    assert_eq!(info(&dir.join("page.pvec"))["rasters"], 1);
    cleanup(&dir);
}

/// `viprs features` lists `jbig2` exactly when it is built, and `full` does not bring it.
#[test]
fn jbig2_is_listed_when_built_and_is_not_in_full() {
    let out = run(&["features"]);
    let listed = String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.trim() == "jbig2");
    assert_eq!(listed, cfg!(feature = "jbig2"));
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let section = |name: &str| {
        let from = manifest
            .find(&format!("\n{name} = ["))
            .unwrap_or_else(|| panic!("no {name}"));
        let rest = &manifest[from + 1..];
        rest[..rest.find(']').unwrap()].to_owned()
    };
    assert!(manifest.contains("jbig2 = [\"libviprs/jbig2\"]"));
    assert!(!section("default").contains("jbig2"));
    assert!(!section("full").contains("jbig2"));
}
