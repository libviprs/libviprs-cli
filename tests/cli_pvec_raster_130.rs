//! Issue #130 (RL1.4), the real binary. Exit codes are not evidence, so every cell asserts on an
//! artifact: a `.pvec` that PVec's reader opens, a tile that comes back out byte for byte, a
//! placement equal to the hand-composed one. Needs `--features pvec-raster`.

#![cfg(feature = "pvec-raster")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pvec_facade::unstable::{RasterId, TileCodec};
use pvec_facade::{Bounds2, Curve, EntityId, Manifest, PvecReader, PvecWriter};
use pvec_geometry::{Point2, Polyline};

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

#[track_caller]
fn ok(args: &[&str]) -> Output {
    let out = run(args);
    assert_eq!(code(&out), 0, "{args:?}:\n{}", stderr(&out));
    out
}

/// Exit 1 with every needle on stderr, and nothing at `out`.
#[track_caller]
fn refused(args: &[&str], needles: &[&str], out: &Path) {
    let result = run(args);
    let err = stderr(&result);
    assert_eq!(code(&result), 1, "{args:?}:\n{err}");
    for needle in needles {
        assert!(err.contains(needle), "{args:?}: {needle:?} not in:\n{err}");
    }
    assert!(!out.exists(), "a refused run must write nothing, found {}", out.display());
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-pvec-raster-130-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A drawing of two entities, written through the facade.
fn drawing(dir: &Path) -> PathBuf {
    let mut w = PvecWriter::new(Manifest {
        metres_per_unit: 1.0,
        bounds: Bounds2 {
            min: [0.0, 0.0],
            max: [4000.0, 4000.0],
        },
        capabilities: Vec::new(),
        producer_id: "viprs-cli-130".into(),
        producer_version: "1".into(),
    })
    .unwrap();
    for (i, y) in [(0u32, 10.0), (1, 20.0)] {
        let line = Polyline::new(vec![
            Point2 { x: 0.0, y },
            Point2 { x: 400.0, y },
            Point2 { x: 400.0, y: y + 300.0 },
        ])
        .unwrap();
        w.push(EntityId::new(i).unwrap(), &Curve::Polyline(line)).unwrap();
    }
    let path = dir.join("drawing.pvec");
    std::fs::write(&path, w.finish().unwrap()).unwrap();
    path
}

/// A 700 by 500 gradient, so a pyramid of tiles of 256 has a partial last column and row.
fn scan(dir: &Path) -> PathBuf {
    let path = dir.join("scan.png");
    ok(&["test-image", s(&path), "--width", "700", "--height", "500"]);
    path
}

fn open(path: &Path) -> PvecReader {
    PvecReader::open(std::fs::read(path).unwrap()).expect("PVec's reader must open the output")
}

/// `pixel_to_source` for a north-up raster at (100, 200) with 0.5 units a pixel.
const PIXEL_TO_GEO: [&str; 4] = ["--geo-origin", "100,200", "--geo-scale", "0.5,-0.5"];

#[test]
fn a_scan_goes_into_a_pvec_and_every_tile_comes_back_equal_to_the_directory_run() {
    let dir = scratch("roundtrip");
    let (input, drawing) = (scan(&dir), drawing(&dir));
    let out = dir.join("out.pvec");
    ok(&[
        "pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", s(&drawing),
        "--pixel-to-source", "1,0,0,-1,0,500",
    ]);

    let reader = open(&out);
    assert_eq!(reader.entity_count().unwrap(), 2, "the drawing's entities ride along");
    let placements = reader.raster_placements().unwrap();
    assert_eq!(placements.len(), 1);
    let p = &placements[0];
    assert_eq!((p.width, p.height, p.tile_edge), (700, 500, 256));
    assert_eq!(p.levels, 3);
    assert_eq!(p.tile_codec, TileCodec::Png);

    // The same input into a tree, in the same grid: the tiles are the same bytes.
    let tree = dir.join("tree");
    ok(&["pyramid", s(&input), s(&tree), "--storage", "directory", "--layout", "google"]);
    let mut compared = 0;
    // 700 by 500 halves to 350 by 250 and 175 by 125: grids of 1 by 1, 2 by 1 and 3 by 2.
    for (level, cols, rows) in [(0u32, 1u32, 1u32), (1, 2, 1), (2, 3, 2)] {
        for row in 0..rows {
            for col in 0..cols {
                let tile = dir.join(format!("tile-{level}-{col}-{row}.bin"));
                ok(&[
                    "pvec", "rasters", s(&out), "--tile", "0", &level.to_string(), &col.to_string(),
                    &row.to_string(), s(&tile),
                ]);
                let from_pvec = std::fs::read(&tile).unwrap();
                let from_tree = std::fs::read(tree.join(format!("{level}/{row}/{col}.png"))).unwrap();
                assert_eq!(from_pvec, from_tree, "tile {level}/{col}/{row}");
                assert_eq!(
                    from_pvec,
                    reader.raster_tile(RasterId::new(0).unwrap(), level, col, row).unwrap().unwrap(),
                    "the command and the library read the same bytes"
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 1 + 2 + 6);
}

#[test]
fn a_composed_placement_equals_the_hand_composed_one() {
    let dir = scratch("composed");
    let (input, drawing) = (scan(&dir), drawing(&dir));
    let out = dir.join("out.pvec");
    // pixel -> geo is [0.5 0 100 / 0 -0.5 200]; the drawing's calibration shifts source space by
    // (100, 100) and keeps the scale, so pixel -> source is [0.5 0 0 -0.5 0 100] in PVec's order.
    let mut args = vec!["pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", s(&drawing)];
    args.extend(PIXEL_TO_GEO);
    args.extend(["--source-to-geo", "1,0,100,0,1,100"]);
    ok(&args);
    let p = &open(&out).raster_placements().unwrap()[0];
    assert_eq!(p.pixel_to_source.map(f64::to_bits), [0.5, 0.0, 0.0, -0.5, 0.0, 100.0].map(f64::to_bits));
}

#[test]
fn affine_composes_like_the_origin_and_scale_it_generalises() {
    let dir = scratch("affine");
    let (input, drawing) = (scan(&dir), drawing(&dir));
    let (a, b) = (dir.join("a.pvec"), dir.join("b.pvec"));
    let mut args = vec!["pyramid", s(&input), s(&a), "--storage", "pvec", "--pvec-into", s(&drawing)];
    args.extend(PIXEL_TO_GEO);
    args.extend(["--source-to-geo", "1,0,0,0,1,0"]);
    ok(&args);
    ok(&[
        "pyramid", s(&input), s(&b), "--storage", "pvec", "--pvec-into", s(&drawing),
        "--affine", "0.5,0,100,0,-0.5,200", "--source-to-geo", "1,0,0,0,1,0",
    ]);
    assert_eq!(
        open(&a).raster_placements().unwrap()[0].pixel_to_source,
        open(&b).raster_placements().unwrap()[0].pixel_to_source
    );
    assert_eq!(
        open(&a).raster_placements().unwrap()[0].pixel_to_source,
        [0.5, 0.0, 0.0, -0.5, 100.0, 200.0]
    );
}

#[test]
fn band_opacity_attribution_and_layer_reach_the_placement_and_the_listing() {
    let dir = scratch("looks");
    let (input, drawing) = (scan(&dir), drawing(&dir));
    let out = dir.join("out.pvec");
    ok(&[
        "pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", s(&drawing),
        "--pixel-to-source", "1,0,0,-1,0,500", "--band", "1", "--opacity", "128",
        "--attribution", "Survey office 2026", "--layer", "Scan",
    ]);
    let p = open(&out).raster_placements().unwrap().remove(0);
    assert_eq!((p.opacity, p.layer_index, p.attribution.as_str()), (128, Some(0), "Survey office 2026"));
    assert_eq!(format!("{:?}", p.band), "Overlay");

    let listed = ok(&["pvec", "rasters", s(&out), "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(json["v"], 1);
    assert_eq!(json["skipped"], serde_json::json!([]));
    let r = &json["rasters"][0];
    assert_eq!(r["id"], 0);
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["tile_edge"].as_u64()), (Some(700), Some(500), Some(256)));
    assert_eq!(r["codec"], "png");
    assert_eq!(r["pixel_format"], "rgb");
    assert_eq!(r["band"], "overlay");
    assert_eq!(r["opacity"], 128);
    assert_eq!(r["layer"], 0);
    assert_eq!(r["attribution"], "Survey office 2026");
    assert_eq!(r["pixel_to_source"], serde_json::json!(p.pixel_to_source));
    assert_eq!(r["visible_box"], serde_json::json!(p.visible_box));

    let text = String::from_utf8_lossy(&ok(&["pvec", "rasters", s(&out)]).stdout).into_owned();
    assert!(text.contains("700x500") && text.contains("Survey office 2026"), "{text}");
}

#[test]
fn a_file_with_no_raster_lists_none() {
    let dir = scratch("none");
    let d = drawing(&dir);
    let json: serde_json::Value =
        serde_json::from_slice(&ok(&["pvec", "rasters", s(&d), "--json"]).stdout).unwrap();
    assert_eq!(json["rasters"], serde_json::json!([]));
}

#[test]
fn blank_tiles_are_left_out_with_drop_blanks_and_stored_without() {
    let dir = scratch("blanks");
    let d = drawing(&dir);
    let black = dir.join("black.png");
    ok(&["black", s(&black), "600", "600", "--bands", "3"]);
    let (kept, dropped) = (dir.join("kept.pvec"), dir.join("dropped.pvec"));
    let base = ["--storage", "pvec", "--pvec-into", s(&d), "--pixel-to-source", "1,0,0,-1,0,600"];
    let mut a = vec!["pyramid", s(&black), s(&kept)];
    a.extend(base);
    ok(&a);
    let mut b = vec!["pyramid", s(&black), s(&dropped)];
    b.extend(base);
    b.push("--drop-blanks");
    ok(&b);
    let id = RasterId::new(0).unwrap();
    assert!(open(&kept).raster_tile(id, 2, 0, 0).unwrap().is_some());
    assert!(open(&dropped).raster_tile(id, 2, 0, 0).unwrap().is_none(), "a blank tile is not stored");
}

#[test]
fn jpeg_and_webp_tiles_are_stored_as_that_codec() {
    let dir = scratch("codecs");
    let (input, d) = (scan(&dir), drawing(&dir));
    for (format, codec) in [("jpeg", TileCodec::Jpeg), ("webp", TileCodec::Webp)] {
        let out = dir.join(format!("{format}.pvec"));
        ok(&[
            "pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", s(&d),
            "--pixel-to-source", "1,0,0,-1,0,500", "--format", format,
        ]);
        assert_eq!(open(&out).raster_placements().unwrap()[0].tile_codec, codec);
    }
}

#[test]
fn a_refused_run_leaves_nothing_behind() {
    let dir = scratch("nothing");
    let (input, d) = (scan(&dir), drawing(&dir));
    let out = dir.join("out.pvec");
    // A drawing that is not there.
    refused(
        &[
            "pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", "/nonexistent/d.pvec",
            "--pixel-to-source", "1,0,0,-1,0,500",
        ],
        &["/nonexistent/d.pvec"],
        &out,
    );
    // A singular placement.
    refused(
        &[
            "pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", s(&d),
            "--pixel-to-source", "0,0,0,0,0,0",
        ],
        &[],
        &out,
    );
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("staging") || n.starts_with(".pvec-sink"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// An archive of the scan at `layout`, with the extra flags.
fn archive(dir: &Path, input: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let out = dir.join(name);
    let mut args = vec!["pyramid", s(input), s(&out)];
    args.extend(extra);
    ok(&args);
    out
}

#[test]
fn to_pvec_passes_every_tile_through_byte_for_byte_and_composes_the_stored_transform() {
    let dir = scratch("passthrough");
    let (input, d) = (scan(&dir), drawing(&dir));
    let mut flags = vec!["--layout", "google"];
    flags.extend(PIXEL_TO_GEO);
    let pm = archive(&dir, &input, "scan.pmtiles", &flags);
    let out = dir.join("out.pvec");
    ok(&[
        "pmtiles", "to-pvec", s(&pm), s(&out), "--pvec-into", s(&d), "--source-to-geo", "1,0,100,0,1,100",
    ]);

    let reader = open(&out);
    assert_eq!(reader.entity_count().unwrap(), 2);
    let p = reader.raster_placements().unwrap().remove(0);
    assert_eq!(p.pixel_to_source.map(f64::to_bits), [0.5, 0.0, 0.0, -0.5, 0.0, 100.0].map(f64::to_bits));
    assert_eq!((p.width, p.height, p.tile_edge, p.levels), (700, 500, 256, 3));

    let mut compared = 0;
    for (level, cols, rows) in [(0u32, 1u32, 1u32), (1, 2, 1), (2, 3, 2)] {
        for row in 0..rows {
            for col in 0..cols {
                let from_archive = run(&["pmtiles", "tile", s(&pm), &level.to_string(), &col.to_string(), &row.to_string(), "-"]).stdout;
                let from_pvec = reader
                    .raster_tile(RasterId::new(0).unwrap(), level, col, row)
                    .unwrap()
                    .unwrap();
                assert_eq!(from_archive, from_pvec, "tile {level}/{col}/{row}");
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 9);
}

#[test]
fn to_pvec_takes_pixel_to_source_for_an_archive_that_stores_no_transform() {
    let dir = scratch("nogeo");
    let (input, d) = (scan(&dir), drawing(&dir));
    let pm = archive(&dir, &input, "plain.pmtiles", &["--layout", "google"]);
    let out = dir.join("out.pvec");
    refused(
        &["pmtiles", "to-pvec", s(&pm), s(&out), "--pvec-into", s(&d), "--source-to-geo", "1,0,0,0,1,0"],
        &["stores no georeference", "--pixel-to-source"],
        &out,
    );
    ok(&[
        "pmtiles", "to-pvec", s(&pm), s(&out), "--pvec-into", s(&d), "--pixel-to-source", "1,0,0,-1,0,500",
    ]);
    assert_eq!(open(&out).raster_placements().unwrap()[0].pixel_to_source, [1.0, 0.0, 0.0, -1.0, 0.0, 500.0]);
}

#[test]
fn to_pvec_refuses_what_a_pvec_cannot_hold_by_name() {
    let dir = scratch("refusals");
    let (input, d) = (scan(&dir), drawing(&dir));
    let out = dir.join("out.pvec");
    let direct = ["--pixel-to-source", "1,0,0,-1,0,500"];
    let try_archive = |name: &str, flags: &[&str], needles: &[&str]| {
        let pm = archive(&dir, &input, name, flags);
        let mut args = vec!["pmtiles", "to-pvec", s(&pm), s(&out), "--pvec-into", s(&d)];
        args.extend(direct);
        refused(&args, needles, &out);
    };
    try_archive("xyz.pmtiles", &["--layout", "xyz"], &["Xyz", "--layout google"]);
    try_archive("centred.pmtiles", &["--layout", "google", "--centre"], &["centred"]);
    try_archive("overlap.pmtiles", &["--layout", "google", "--overlap", "2"], &["overlap of 2"]);
    try_archive("big.pmtiles", &["--layout", "google", "--tile-size", "2048"], &["2048", "256, 512 or 1024"]);
}

#[test]
fn to_pvec_refuses_a_vector_archive_by_its_tile_type() {
    let dir = scratch("mvt");
    let d = drawing(&dir);
    let out = dir.join("out.pvec");
    let ladder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vector/ladder.pmtiles");
    refused(
        &[
            "pmtiles", "to-pvec", s(&ladder), s(&out), "--pvec-into", s(&d), "--pixel-to-source",
            "1,0,0,-1,0,500",
        ],
        &["PNG, JPEG or WebP"],
        &out,
    );
}

#[test]
fn the_drawing_can_be_the_output() {
    let dir = scratch("inplace");
    let (input, d) = (scan(&dir), drawing(&dir));
    ok(&[
        "pyramid", s(&input), s(&d), "--storage", "pvec", "--pvec-into", s(&d),
        "--pixel-to-source", "1,0,0,-1,0,500",
    ]);
    let reader = open(&d);
    assert_eq!(reader.entity_count().unwrap(), 2);
    assert_eq!(reader.raster_placements().unwrap().len(), 1);
    // A second raster is not what this command joins.
    let again = run(&[
        "pyramid", s(&input), s(&d), "--storage", "pvec", "--pvec-into", s(&d),
        "--pixel-to-source", "1,0,0,-1,0,500",
    ]);
    assert_eq!(code(&again), 1, "{}", stderr(&again));
    assert_eq!(open(&d).raster_placements().unwrap().len(), 1, "the refused run left the file as it was");
}

#[test]
fn a_tile_that_is_not_there_is_an_error_and_prints_nothing_on_stdout() {
    let dir = scratch("missing-tile");
    let (input, d) = (scan(&dir), drawing(&dir));
    let out = dir.join("out.pvec");
    ok(&[
        "pyramid", s(&input), s(&out), "--storage", "pvec", "--pvec-into", s(&d),
        "--pixel-to-source", "1,0,0,-1,0,500",
    ]);
    let result = run(&["pvec", "rasters", s(&out), "--tile", "0", "2", "9", "9", "-"]);
    assert_eq!(code(&result), 1);
    assert!(result.stdout.is_empty());
    let result = run(&["pvec", "rasters", s(&out), "--tile", "5", "0", "0", "0", "-"]);
    assert_eq!(code(&result), 1);
    assert!(result.stdout.is_empty());
}
