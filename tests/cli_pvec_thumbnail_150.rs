//! libviprs-cli#150 (CV2.1), `viprs pvec thumbnail FILE OUT.png [--width N]`, the real binary: a
//! PNG of a published `.pvec`, strokes only. Exit codes are not evidence, so the cells decode the
//! PNG and look at its size and its pixels, and the usage cells are held in every build.

// A build without a feature leaves some helpers unused, and every cell is gated on what it needs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    let dir = std::env::temp_dir().join(format!("viprs-thumb-150-{}-{tag}", std::process::id()));
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

/// A `.pvec` made by `pvec convert --model` of a fixture, and its path.
#[cfg(feature = "pvec-convert")]
fn model_pvec(dir: &Path, drawing: &str) -> PathBuf {
    let drawing = fixture(drawing);
    let out = run(&[
        "pvec",
        "convert",
        s(&drawing),
        s(dir),
        "--model",
        "--unit",
        "m",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    dir.join(format!(
        "{}.pvec",
        drawing.file_stem().unwrap().to_string_lossy()
    ))
}

/// The artifact's declared bounds, from `pvec info --json`.
#[cfg(feature = "pvec-raster")]
fn bounds(pvec: &Path) -> (f64, f64) {
    let out = run(&["pvec", "info", s(pvec), "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let b = &json["bounds"];
    let at = |k: &str, i: usize| b[k][i].as_f64().unwrap();
    (at("max", 0) - at("min", 0), at("max", 1) - at("min", 1))
}

/// A drawing whose bounds are 100 wide and 1000 tall, written through the facade.
#[cfg(feature = "pvec-raster")]
fn tall(dir: &Path) -> PathBuf {
    use pvec_facade::{Bounds2, Curve, EntityId, Manifest, PvecWriter};
    use pvec_geometry::{Point2, Polyline};
    let mut w = PvecWriter::new(Manifest {
        metres_per_unit: 1.0,
        bounds: Bounds2 {
            min: [0.0, 0.0],
            max: [100.0, 1000.0],
        },
        capabilities: Vec::new(),
        producer_id: "viprs-cli-150".into(),
        producer_version: "1".into(),
    })
    .unwrap();
    let line = Polyline::new(vec![
        Point2 { x: 0.0, y: 0.0 },
        Point2 {
            x: 100.0,
            y: 1000.0,
        },
    ])
    .unwrap();
    w.push(EntityId::new(0).unwrap(), &Curve::Polyline(line))
        .unwrap();
    let path = dir.join("tall.pvec");
    std::fs::write(&path, w.finish().unwrap()).unwrap();
    path
}

/// The PNG's width and height, and whether it has a pixel that is not white.
#[cfg(feature = "pvec-raster-thumbnail")]
fn png(path: &Path) -> (u32, u32, bool) {
    let raster = libviprs::decode_file(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (
        raster.width(),
        raster.height(),
        raster.data().iter().any(|&v| v != 255),
    )
}

#[cfg(all(
    feature = "pvec-convert",
    feature = "pvec-raster",
    feature = "pvec-raster-thumbnail"
))]
#[test]
fn the_width_is_what_was_asked_and_the_height_follows_the_bounds() {
    let dir = scratch("size");
    let pvec = model_pvec(&dir, "vc_layers.dwg");
    let (w, h) = bounds(&pvec);
    for width in [64u32, 300, 1024] {
        let out_png = dir.join(format!("w{width}.png"));
        let out = run(&[
            "pvec",
            "thumbnail",
            s(&pvec),
            s(&out_png),
            "--width",
            &width.to_string(),
        ]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        let (pw, ph, _) = png(&out_png);
        assert_eq!(pw, width);
        assert_eq!(
            ph,
            ((f64::from(width) * h / w).round() as u32).max(1),
            "{width}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(all(feature = "pvec-convert", feature = "pvec-raster-thumbnail"))]
#[test]
fn the_default_width_is_512_and_strokes_are_drawn() {
    let dir = scratch("default");
    let pvec = model_pvec(&dir, "sample_AC1018.dwg");
    let out_png = dir.join("plan.png");
    let out = run(&["pvec", "thumbnail", s(&pvec), s(&out_png)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let (w, h, dark) = png(&out_png);
    assert_eq!(w, 512);
    assert!(h >= 1);
    assert!(dark, "a drawing with strokes drew an all white picture");
    let _ = std::fs::remove_dir_all(dir);
}

/// A drawing with no stroke in it is a white picture, not an error.
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster-thumbnail"))]
#[test]
fn the_picture_is_black_strokes_on_opaque_white() {
    let dir = scratch("white");
    let pvec = model_pvec(&dir, "sample_AC1018.dwg");
    let out_png = dir.join("plan.png");
    run(&["pvec", "thumbnail", s(&pvec), s(&out_png), "--width", "200"]);
    let raster = libviprs::decode_file(&out_png).unwrap();
    assert_eq!(
        raster.data().len() as u64,
        u64::from(raster.width()) * u64::from(raster.height()) * 3,
        "an opaque RGB picture"
    );
    // Only the black of the strokes and the white of the paper, and the grey that anti-aliasing
    // makes between them.
    assert!(raster.data().contains(&255));
    assert!(raster.data().iter().any(|&v| v < 128));
    let _ = std::fs::remove_dir_all(dir);
}

/// A derived height past the cap is refused, naming the width to lower, and writes nothing.
#[cfg(all(feature = "pvec-raster", feature = "pvec-raster-thumbnail"))]
#[test]
fn a_height_past_the_cap_is_refused_and_writes_nothing() {
    let dir = scratch("cap");
    let pvec = tall(&dir);
    let out_png = dir.join("tall.png");
    // 4096 wide on a 1:10 sheet is 40960 tall.
    let out = run(&[
        "pvec",
        "thumbnail",
        s(&pvec),
        s(&out_png),
        "--width",
        "4096",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("--width"), "{}", stderr(&out));
    assert!(stderr(&out).contains("16384"), "{}", stderr(&out));
    assert!(!out_png.exists());
    // The same sheet narrower is fine, and the height is the aspect's.
    let out = run(&["pvec", "thumbnail", s(&pvec), s(&out_png), "--width", "512"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(png(&out_png).0, 512);
    assert_eq!(png(&out_png).1, 5120);
    let _ = std::fs::remove_dir_all(dir);
}

/// A zero width, a width past the cap or not a number is exit 2 in every build, and the file is
/// never opened: these name a file that is not there and still get 2, not 1.
#[test]
fn a_width_that_cannot_be_drawn_exits_2_before_the_file_is_opened() {
    let dir = scratch("usage");
    for width in ["0", "4097", "99999999999", "wide"] {
        let out = run(&[
            "pvec",
            "thumbnail",
            s(&dir.join("not-there.pvec")),
            s(&dir.join("out.png")),
            "--width",
            width,
        ]);
        assert_eq!(code(&out), 2, "{width}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("invalid value"),
            "{width}: {}",
            stderr(&out)
        );
        assert!(!dir.join("out.png").exists());
    }
    let out = run(&["pvec", "thumbnail", s(&dir.join("a.pvec"))]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("unrecognized subcommand"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_that_is_not_there_exits_1_and_writes_nothing() {
    let dir = scratch("missing");
    let out = run(&[
        "pvec",
        "thumbnail",
        s(&dir.join("nope.pvec")),
        s(&dir.join("o.png")),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("nope.pvec"), "{}", stderr(&out));
    assert!(!dir.join("o.png").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-raster-thumbnail")]
#[test]
fn a_corrupt_artifact_exits_1_and_writes_nothing() {
    let dir = scratch("corrupt");
    let junk = dir.join("junk.pvec");
    std::fs::write(&junk, b"this is not a pvec artifact").unwrap();
    let out = run(&["pvec", "thumbnail", s(&junk), s(&dir.join("o.png"))]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("junk.pvec"), "{}", stderr(&out));
    assert!(!dir.join("o.png").exists());
    // A real artifact with its tail cut off is as unreadable.
    #[cfg(feature = "pvec-convert")]
    {
        let pvec = model_pvec(&dir, "vc_layers.dwg");
        let bytes = std::fs::read(&pvec).unwrap();
        let cut = dir.join("cut.pvec");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        let out = run(&["pvec", "thumbnail", s(&cut), s(&dir.join("o.png"))]);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert!(!dir.join("o.png").exists());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(all(feature = "pvec-convert", feature = "pvec-raster-thumbnail"))]
#[test]
fn an_output_that_cannot_be_written_exits_1() {
    let dir = scratch("unwritable");
    let pvec = model_pvec(&dir, "vc_layers.dwg");
    let out = run(&[
        "pvec",
        "thumbnail",
        s(&pvec),
        s(&dir.join("no-such-dir/o.png")),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("o.png"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_help_says_strokes_only() {
    let out = run(&["pvec", "thumbnail", "--help"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let help = stdout(&out);
    for needle in [
        "--width",
        "strokes only",
        "no text, no fills",
        "4096",
        "16384",
    ] {
        assert!(help.contains(needle), "{needle} not in:\n{help}");
    }
}

/// A build without the feature exits 1 and names it, once the arguments are fine.
#[cfg(not(feature = "pvec-raster-thumbnail"))]
#[test]
fn a_build_without_the_feature_exits_1_and_names_it() {
    let dir = scratch("nofeature");
    let pvec = dir.join("a.pvec");
    std::fs::write(&pvec, b"x").unwrap();
    let out = run(&["pvec", "thumbnail", s(&pvec), s(&dir.join("o.png"))]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("pvec-raster-thumbnail"),
        "{}",
        stderr(&out)
    );
    assert!(!dir.join("o.png").exists());
    let _ = std::fs::remove_dir_all(dir);
}
