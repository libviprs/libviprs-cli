//! CV2.3 (#152): the pyramid knobs the core already has at the pin, driven
//! through the real binary.
//!
//! Zoomify and IIIF output, an explicit engine, a background colour, the retry
//! multiplier and jitter, PMTiles metadata, internal compression, bounds and
//! centre, PDFium strip streaming with a DPI floor, `webpsave --keep`, and the
//! `object-store://` spelling of the object-store sink with `s3://` kept as a
//! deprecated alias. Each cell checks what lands on disk or what the archive
//! says about itself, not only the exit code.
//!
//! The inputs are hand-written PPMs (and one minimal PDF for the PDFium cells,
//! which skip when the machine has no libpdfium), so nothing here needs the
//! image crate.

use std::collections::BTreeMap;
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

fn s(p: &Path) -> &str {
    p.to_str().expect("temp paths are UTF-8")
}

/// A temp directory of this cell's own.
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-152-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

fn ok(out: &Output, what: &str) {
    assert_eq!(code(out), 0, "{what} failed:\n{}", stderr(out));
}

/// A usage error this crate decided on: exit 2, and not clap failing to know
/// the flag, which would exit 2 just the same.
fn refused(out: &Output, what: &str) {
    assert_eq!(code(out), 2, "{what}: {}", stderr(out));
    assert!(
        !stderr(out).contains("unexpected argument"),
        "{what}: the flag has to exist and refuse this on purpose:\n{}",
        stderr(out)
    );
}

/// Write a binary PPM whose pixel at `(x, y)` is `pixel(x, y)`.
fn write_ppm(path: &Path, width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) {
    let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&pixel(x, y));
        }
    }
    std::fs::write(path, bytes).expect("the PPM must be writable");
}

/// 200x100 of gradient where blue is always `(x + y) % 256`, so a pixel like
/// `[10, 20, 99]` cannot come from the image and has to be background.
fn off_grid(dir: &Path) -> PathBuf {
    let path = dir.join("off-grid.ppm");
    write_ppm(&path, 200, 100, |x, y| [x as u8, y as u8, (x + y) as u8]);
    path
}

/// 1024x1024 of gradient: at `--tile-size 1024` the streaming engine's smallest
/// strip is 1024 x 2048 x 3 bytes, far past a 1 MB budget.
fn big_square(dir: &Path) -> PathBuf {
    let path = dir.join("big.ppm");
    write_ppm(&path, 1024, 1024, |x, y| [x as u8, y as u8, (x ^ y) as u8]);
    path
}

/// Every regular file under `root`, keyed by its path relative to `root`.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .expect("under the base")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&path).expect("readable"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The tiles of a tree, keyed by relative path, without the sidecars.
fn tiles(root: &Path, ext: &str) -> BTreeMap<String, Vec<u8>> {
    tree(root)
        .into_iter()
        .filter(|(k, _)| k.ends_with(&format!(".{ext}")))
        .collect()
}

// ---------------------------------------------------------------------------
// object-store:// and the s3:// alias
// ---------------------------------------------------------------------------

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
fn store_run(dir: &Path, uri: &str, root: &Path, extra: &[&str]) -> Output {
    let input = off_grid(dir);
    let mut args = vec![
        "pyramid",
        s(&input),
        "--sink",
        uri,
        "--object-store-root",
        s(root),
        "--tile-size",
        "64",
        "--layout",
        "xyz",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn object_store_scheme_writes_the_same_tree_as_s3() {
    let dir = unique_dir("os-same");
    let (a, b) = (dir.join("a"), dir.join("b"));
    let canonical = store_run(&dir, "object-store://tiles/run-1", &a, &[]);
    ok(&canonical, "the object-store:// run");
    let alias = store_run(&dir, "s3://tiles/run-1", &b, &[]);
    ok(&alias, "the s3:// run");
    let (ta, tb) = (tree(&a), tree(&b));
    assert!(
        ta.keys().any(|k| k.ends_with(".png")),
        "the object-store:// run must write tiles under the root, got {:?}",
        ta.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        ta, tb,
        "object-store:// and s3:// must write the same keys with the same bytes"
    );
    assert!(
        !stderr(&canonical).contains("deprecated"),
        "the canonical spelling must not print the deprecation note:\n{}",
        stderr(&canonical)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn s3_alias_prints_one_deprecation_line_on_stderr() {
    let dir = unique_dir("os-note");
    let out = store_run(&dir, "s3://tiles/run-1", &dir.join("root"), &[]);
    ok(&out, "the s3:// run");
    let err = stderr(&out);
    let notes: Vec<&str> = err.lines().filter(|l| l.contains("deprecated")).collect();
    assert_eq!(
        notes.len(),
        1,
        "s3:// must print exactly one deprecation line, got:\n{err}"
    );
    assert!(
        notes[0].contains("object-store://"),
        "the note must name the spelling to use instead: {}",
        notes[0]
    );
    assert!(
        notes[0].contains("upload"),
        "the note must say nothing is uploaded: {}",
        notes[0]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn s3_alias_note_stays_off_stdout_under_events_json() {
    let dir = unique_dir("os-events");
    let out = store_run(
        &dir,
        "s3://tiles/run-1",
        &dir.join("root"),
        &["--events", "json"],
    );
    ok(&out, "the s3:// run with --events json");
    let text = stdout(&out);
    assert!(!text.trim().is_empty(), "--events json printed nothing");
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line:?}"));
        assert_eq!(
            value["v"], 1,
            "an event line without its schema version: {line}"
        );
    }
    assert!(stderr(&out).contains("deprecated"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The contract the panel settled on: viprs writes the object-store key
/// layout into a directory and the caller uploads it. So the keys under
/// `ROOT/bucket/prefix/image/` have to be exactly the tiles a directory run of the
/// same plan writes, key for key and byte for byte; that is what a bucket sync
/// of the root would upload. The object store keeps no manifest of its own
/// (the core's sink writes tiles only), so the tree is the reference.
#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn object_store_holds_the_tree_key_for_key() {
    let dir = unique_dir("os-keys");
    let input = off_grid(&dir);
    let reference = dir.join("tree");
    let out = run(&[
        "pyramid",
        s(&input),
        s(&reference),
        "--storage",
        "directory",
        "--tile-size",
        "64",
        "--layout",
        "xyz",
    ]);
    ok(&out, "the reference tree");
    let root = dir.join("root");
    let out = store_run(&dir, "object-store://tiles/run-1", &root, &[]);
    ok(&out, "the object-store:// run");

    let expected = tiles(&reference, "png");
    assert!(!expected.is_empty(), "the reference tree has no tiles");
    let store = tree(&root.join("tiles").join("run-1").join("off-grid"));
    assert_eq!(
        store.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the store must hold one key per tile of the tree, and nothing else"
    );
    for (key, bytes) in &expected {
        assert_eq!(
            &store[key], bytes,
            "{key} differs between the store and the tree"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn object_store_without_a_root_is_a_usage_error_naming_the_root() {
    let dir = unique_dir("os-noroot");
    let input = off_grid(&dir);
    let out = run(&["pyramid", s(&input), "--sink", "object-store://tiles/run-1"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--object-store-root"),
        "{}",
        stderr(&out)
    );
    assert!(
        !Path::new("object-store:").exists(),
        "an unknown scheme must not fall through to a directory named after the URI"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn object_store_refuses_a_bucket_that_climbs_out_of_the_root() {
    let dir = unique_dir("os-escape");
    let out = store_run(&dir, "object-store://../escaped", &dir.join("root"), &[]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not a bucket name"),
        "{}",
        stderr(&out)
    );
    assert!(!dir.join("escaped").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pyramid_help_says_the_object_store_uploads_nothing() {
    let out = run(&["pyramid", "--help"]);
    ok(&out, "pyramid --help");
    let text = stdout(&out);
    assert!(text.contains("object-store://"), "{text}");
    assert!(text.contains("--object-store-root"), "{text}");
    assert!(
        text.contains("aws s3 sync") && text.contains("rclone"),
        "the help must name the tools that do the upload:\n{text}"
    );
}

#[cfg(not(any(feature = "s3", feature = "object-store-sink")))]
#[test]
fn object_store_without_the_feature_exits_1_naming_it() {
    let dir = unique_dir("os-nofeature");
    let input = off_grid(&dir);
    let out = run(&[
        "pyramid",
        s(&input),
        "--sink",
        "object-store://tiles/run-1",
        "--object-store-root",
        s(&dir.join("root")),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--features object-store-sink"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --layout zoomify | iiif, --iiif-id
// ---------------------------------------------------------------------------

#[test]
fn zoomify_into_a_tree_writes_tile_groups_and_image_properties() {
    let dir = unique_dir("zoomify");
    let input = off_grid(&dir);
    let out_dir = dir.join("zoomify");
    let out = run(&[
        "pyramid",
        s(&input),
        s(&out_dir),
        "--storage",
        "directory",
        "--layout",
        "zoomify",
        "--tile-size",
        "64",
    ]);
    ok(&out, "the zoomify run");
    let all = tree(&out_dir);
    let props = all
        .iter()
        .find(|(k, _)| k.ends_with("ImageProperties.xml"))
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_else(|| panic!("no ImageProperties.xml in {:?}", all.keys()));
    assert!(
        props.contains("WIDTH=\"200\"") && props.contains("HEIGHT=\"100\""),
        "{props}"
    );
    assert!(
        all.keys()
            .any(|k| k.contains("TileGroup0/") && k.ends_with("0-0-0.png")),
        "a Zoomify tree keeps its tiles in TileGroupN/level-col-row: {:?}",
        all.keys()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn iiif_into_a_tree_writes_info_json_carrying_the_id() {
    let dir = unique_dir("iiif");
    let input = off_grid(&dir);
    let out_dir = dir.join("iiif");
    let id = "https://example.org/iiif/plan-a";
    let out = run(&[
        "pyramid",
        s(&input),
        s(&out_dir),
        "--storage",
        "directory",
        "--layout",
        "iiif",
        "--iiif-id",
        id,
        "--tile-size",
        "64",
    ]);
    ok(&out, "the iiif run");
    let all = tree(&out_dir);
    let info = all
        .iter()
        .find(|(k, _)| k.ends_with("info.json"))
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_else(|| panic!("no info.json in {:?}", all.keys()));
    let json: serde_json::Value = serde_json::from_str(&info).expect("info.json is JSON");
    let carried = json
        .get("id")
        .or_else(|| json.get("@id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        carried.starts_with(id),
        "info.json must carry --iiif-id: {info}"
    );
    assert_eq!(json["width"], 200, "{info}");
    assert!(
        all.keys().any(|k| k.ends_with("default.png")),
        "IIIF tiles are {{region}}/{{size}}/0/default.png: {:?}",
        all.keys()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zoomify_and_iiif_are_refused_into_an_archive() {
    let dir = unique_dir("layout-archive");
    let input = off_grid(&dir);
    for layout in ["zoomify", "iiif"] {
        let archive = dir.join(format!("{layout}.pmtiles"));
        let out = run(&["pyramid", s(&input), s(&archive), "--layout", layout]);
        assert_eq!(code(&out), 2, "{layout}: {}", stderr(&out));
        assert!(stderr(&out).contains(layout), "{layout}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("PMTiles archive"),
            "{layout}: the refusal must say why: {}",
            stderr(&out)
        );
        assert!(!archive.exists(), "a refused run must write nothing");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn iiif_id_without_the_iiif_layout_is_refused() {
    let dir = unique_dir("iiif-id-alone");
    let input = off_grid(&dir);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("tree")),
        "--storage",
        "directory",
        "--iiif-id",
        "https://example.org/iiif/x",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--layout iiif"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_pack_still_refuses_the_layouts_an_archive_cannot_address() {
    let dir = unique_dir("pack-zoomify");
    let tree_dir = dir.join("tree");
    std::fs::create_dir_all(&tree_dir).unwrap();
    let out = run(&[
        "pmtiles",
        "pack",
        s(&tree_dir),
        s(&dir.join("out.pmtiles")),
        "--layout",
        "zoomify",
        "--width",
        "200",
        "--height",
        "100",
        "--tile-size",
        "64",
        "--overlap",
        "0",
        "--format",
        "png",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --engine
// ---------------------------------------------------------------------------

fn tree_run(dir: &Path, input: &Path, name: &str, extra: &[&str]) -> (Output, PathBuf) {
    let out_dir = dir.join(name);
    let mut args = vec![
        "pyramid",
        s(input),
        s(&out_dir),
        "--storage",
        "directory",
        "--tile-size",
        "64",
        "--layout",
        "xyz",
    ];
    args.extend_from_slice(extra);
    (run(&args), out_dir)
}

/// The `event` names of the `--events json` lines on stdout.
fn event_names(out: &Output) -> Vec<String> {
    stdout(out)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["event"].as_str().map(str::to_string))
        .collect()
}

/// Every engine writes the same tiles, and the engine asked for is the one
/// that ran: only the strip engines report strips (`strip_*` events), so a run
/// whose --engine never reached the builder fails here.
#[test]
fn every_engine_writes_the_same_tiles() {
    let dir = unique_dir("engines");
    let input = off_grid(&dir);
    let (out, reference) = tree_run(&dir, &input, "default", &["--events", "json"]);
    ok(&out, "the default run");
    let names = event_names(&out);
    assert!(
        !names.iter().any(|e| e.starts_with("strip_")),
        "the default engine on a decoded image is monolithic and has no strips: {names:?}"
    );
    let expected = tiles(&reference, "png");
    assert!(!expected.is_empty());
    for (engine, strips) in [
        ("monolithic", false),
        ("streaming", true),
        ("map-reduce", true),
        ("map-reduce-hot-cache", true),
    ] {
        let (out, got) = tree_run(
            &dir,
            &input,
            engine,
            &["--engine", engine, "--events", "json"],
        );
        ok(&out, engine);
        assert_eq!(
            tiles(&got, "png"),
            expected,
            "--engine {engine} must write the tiles the default engine writes"
        );
        let names = event_names(&out);
        assert_eq!(
            names.iter().any(|e| e.starts_with("strip_")),
            strips,
            "--engine {engine} has to be the engine that ran, events: {names:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The strip engines cannot emit tiles in tile id order (the core's
/// `UnsupportedEmissionOrder`), and the command line alone says so, so the
/// refusal comes before the input is read: a missing input is never reported.
#[test]
fn a_strip_engine_with_ordered_emission_is_refused_before_the_input_is_read() {
    let dir = unique_dir("engine-ordered");
    let missing = dir.join("no-such-input.ppm");
    for engine in ["streaming", "map-reduce", "map-reduce-hot-cache"] {
        let out = run(&[
            "pyramid",
            s(&missing),
            s(&dir.join("o.pmtiles")),
            "--engine",
            engine,
            "--ordered-emission",
        ]);
        refused(&out, &format!("--engine {engine} --ordered-emission"));
        assert!(
            stderr(&out).contains("--ordered-emission"),
            "{engine}: {}",
            stderr(&out)
        );
    }
    let input = off_grid(&dir);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("m.pmtiles")),
        "--tile-size",
        "64",
        "--engine",
        "monolithic",
        "--ordered-emission",
    ]);
    ok(&out, "the monolithic engine emits in tile id order");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--storage pvec` builds its raster on its own path, so the knobs that path
/// would drop are refused up front instead of accepted and ignored.
#[test]
fn pvec_storage_refuses_the_knobs_it_would_drop() {
    let dir = unique_dir("pvec-knobs");
    let input = off_grid(&dir);
    let drawing = dir.join("drawing.pvec");
    let cases: [(&str, &[&str]); 4] = [
        ("--engine", &["--engine", "streaming"]),
        ("--iiif-id", &["--iiif-id", "https://example.org/iiif/x"]),
        (
            "--render-stream",
            &["--render", "--memory-budget", "8", "--render-stream"],
        ),
        (
            "--min-dpi",
            &["--render", "--memory-budget", "8", "--min-dpi", "36"],
        ),
    ];
    for (flag, extra) in cases {
        let out_path = dir.join("out.pvec");
        let mut args = vec![
            "pyramid",
            s(&input),
            s(&out_path),
            "--storage",
            "pvec",
            "--pvec-into",
            s(&drawing),
        ];
        args.extend_from_slice(extra);
        let out = run(&args);
        refused(&out, &format!("--storage pvec {extra:?}"));
        assert!(
            stderr(&out).contains(&format!("{flag} does not apply to --storage pvec")),
            "{flag}: {}",
            stderr(&out)
        );
        assert!(!out_path.exists(), "a refused run must write nothing");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The proof that `--engine` reaches the builder: into an archive, where
/// `--memory-budget` alone is noted and ignored, `--engine streaming` hands it
/// to the streaming engine, whose pre-flight refuses a budget smaller than one
/// strip.
#[test]
fn engine_streaming_takes_the_memory_budget_into_an_archive() {
    let dir = unique_dir("engine-budget");
    let input = big_square(&dir);
    let base = |name: &str| {
        let archive = dir.join(name);
        vec![
            "pyramid".to_string(),
            s(&input).to_string(),
            s(&archive).to_string(),
            "--tile-size".to_string(),
            "1024".to_string(),
            "--memory-budget".to_string(),
            "1".to_string(),
        ]
    };
    let plain: Vec<String> = base("plain.pmtiles");
    let plain: Vec<&str> = plain.iter().map(String::as_str).collect();
    let out = run(&plain);
    ok(&out, "the archive run without --engine");
    let mut streamed = base("streamed.pmtiles");
    streamed.extend(["--engine".to_string(), "streaming".to_string()]);
    let streamed: Vec<&str> = streamed.iter().map(String::as_str).collect();
    let out = run(&streamed);
    assert_eq!(
        code(&out),
        1,
        "the streaming engine must refuse a 1 MB budget for a 6 MB strip:\n{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).to_lowercase().contains("budget"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn engine_monolithic_with_a_memory_budget_is_refused() {
    let dir = unique_dir("engine-mono-budget");
    let input = off_grid(&dir);
    let (out, _) = tree_run(
        &dir,
        &input,
        "t",
        &["--engine", "monolithic", "--memory-budget", "64"],
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--memory-budget"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn engine_conflicts_with_parallel_and_region() {
    let dir = unique_dir("engine-conflicts");
    let input = off_grid(&dir);
    let (out, _) = tree_run(&dir, &input, "a", &["--engine", "streaming", "--parallel"]);
    refused(&out, "--engine with --parallel");
    let (out, _) = tree_run(
        &dir,
        &input,
        "b",
        &["--engine", "streaming", "--region", "0,0,64,64"],
    );
    refused(&out, "--engine with --region");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --background
// ---------------------------------------------------------------------------

/// The first pixel of tile (0, 0) at the deepest level of a centred, raw XYZ
/// tree. Centring 200x100 in a 256x128 grid leaves a 28x14 border, so that
/// pixel is padding.
fn corner_pixel(dir: &Path, input: &Path, name: &str, extra: &[&str]) -> [u8; 3] {
    let mut args = vec!["--centre", "--format", "raw"];
    args.extend_from_slice(extra);
    let (out, root) = tree_run(dir, input, name, &args);
    ok(&out, name);
    let deepest = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
        .max()
        .expect("an XYZ tree has numbered level directories");
    let tile = root.join(deepest.to_string()).join("0").join("0.raw");
    let bytes = std::fs::read(&tile).unwrap_or_else(|e| panic!("{}: {e}", tile.display()));
    [bytes[0], bytes[1], bytes[2]]
}

#[test]
fn background_fills_the_padding_of_a_centred_pyramid() {
    let dir = unique_dir("background");
    let input = off_grid(&dir);
    assert_eq!(corner_pixel(&dir, &input, "white", &[]), [255, 255, 255]);
    assert_eq!(
        corner_pixel(&dir, &input, "coloured", &["--background", "10,20,99"]),
        [10, 20, 99],
        "the padding must be the --background colour"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn background_needs_three_channels_of_0_to_255() {
    let dir = unique_dir("background-bad");
    let input = off_grid(&dir);
    for bad in ["10,20", "10,20,300", "a,b,c", "1,2,3,4"] {
        let (out, _) = tree_run(&dir, &input, "t", &["--background", bad]);
        refused(&out, &format!("--background {bad}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// --retry-multiplier, --no-retry-jitter
// ---------------------------------------------------------------------------

/// A pyramid into the stub object store with its root pointed at a regular
/// file, so every tile write fails, timed.
#[cfg(any(feature = "s3", feature = "object-store-sink"))]
fn failing_store_run(dir: &Path, extra: &[&str]) -> (Output, f64) {
    let input = off_grid(dir);
    let root = dir.join("not-a-directory");
    std::fs::write(&root, b"a file where the store wants a directory").unwrap();
    let mut args = vec![
        "pyramid",
        s(&input),
        "--sink",
        "object-store://bucket/run",
        "--object-store-root",
        s(&root),
        "--tile-size",
        "64",
    ];
    args.extend_from_slice(extra);
    let start = std::time::Instant::now();
    let out = run(&args);
    (out, start.elapsed().as_secs_f64())
}

/// Three retries from 100 ms with no jitter sleep 300 ms at a multiplier of 1
/// and 100 + 400 + 1600 = 2100 ms at 4, per failing tile.
#[cfg(any(feature = "s3", feature = "object-store-sink"))]
#[test]
fn retry_multiplier_grows_the_backoff() {
    let dir = unique_dir("retry-mult");
    let flags = |m: &'static str| {
        vec![
            "--retries",
            "3",
            "--retry-backoff-ms",
            "100",
            "--no-retry-jitter",
            "--retry-multiplier",
            m,
        ]
    };
    let (flat, flat_s) = failing_store_run(&dir, &flags("1"));
    assert_eq!(code(&flat), 1, "{}", stderr(&flat));
    let (steep, steep_s) = failing_store_run(&dir, &flags("4"));
    assert_eq!(code(&steep), 1, "{}", stderr(&steep));
    assert!(
        steep_s >= 2.0,
        "a multiplier of 4 must sleep at least 2.1 s before failing, took {steep_s:.2}s"
    );
    assert!(
        steep_s - flat_s >= 1.0,
        "a multiplier of 4 must take well over a multiplier of 1: {steep_s:.2}s vs {flat_s:.2}s"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retry_knobs_need_a_retrying_policy_and_a_sane_multiplier() {
    let dir = unique_dir("retry-bad");
    let input = off_grid(&dir);
    let cases: [&[&str]; 5] = [
        &["--retry-multiplier", "2"],
        &["--no-retry-jitter"],
        &["--retries", "2", "--retry-multiplier", "0.5"],
        &["--retries", "2", "--retry-multiplier", "NaN"],
        &["--retries", "2", "--retry-multiplier", "inf"],
    ];
    for extra in cases {
        let (out, _) = tree_run(&dir, &input, "t", extra);
        assert_eq!(code(&out), 2, "{extra:?}: {}", stderr(&out));
    }
    let (out, _) = tree_run(
        &dir,
        &input,
        "spec",
        &["--on-failure", "retry=2,1ms", "--retry-multiplier", "3"],
    );
    ok(&out, "--retry-multiplier on an --on-failure retry spec");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// PMTiles metadata, internal compression, bounds and centre
// ---------------------------------------------------------------------------

fn archive_run(dir: &Path, name: &str, extra: &[&str]) -> (Output, PathBuf) {
    let input = off_grid(dir);
    let archive = dir.join(name);
    let mut args = vec!["pyramid", s(&input), s(&archive), "--tile-size", "64"];
    args.extend_from_slice(extra);
    (run(&args), archive)
}

fn info(archive: &Path) -> String {
    let out = run(&["pmtiles", "info", s(archive)]);
    ok(&out, "pmtiles info");
    stdout(&out)
}

fn metadata(info: &str) -> serde_json::Value {
    let line = info
        .lines()
        .find_map(|l| l.strip_prefix("Metadata: "))
        .unwrap_or_else(|| panic!("no Metadata line:\n{info}"));
    serde_json::from_str(line).expect("the metadata is JSON")
}

#[test]
fn pmtiles_name_description_and_attribution_land_in_the_metadata() {
    let dir = unique_dir("pm-meta");
    let (out, archive) = archive_run(
        &dir,
        "meta.pmtiles",
        &[
            "--pmtiles-name",
            "Plan A",
            "--pmtiles-description",
            "Ground floor, as built",
            "--pmtiles-attribution",
            "Survey 2026",
        ],
    );
    ok(&out, "the archive run");
    let meta = metadata(&info(&archive));
    assert_eq!(meta["name"], "Plan A", "{meta}");
    assert_eq!(meta["description"], "Ground floor, as built", "{meta}");
    assert_eq!(meta["attribution"], "Survey 2026", "{meta}");
    assert!(
        meta.get("vnd.libviprs").is_some(),
        "the core's own namespace must survive: {meta}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_internal_compression_none_writes_a_readable_archive() {
    let dir = unique_dir("pm-none");
    let (out, archive) = archive_run(
        &dir,
        "none.pmtiles",
        &["--pmtiles-internal-compression", "none"],
    );
    ok(&out, "the archive run");
    let text = info(&archive);
    assert!(text.contains("Internal compression: none"), "{text}");
    let out = run(&["pmtiles", "verify", s(&archive)]);
    ok(&out, "pmtiles verify on an uncompressed index");
    let (out, gz) = archive_run(&dir, "gzip.pmtiles", &[]);
    ok(&out, "the default archive run");
    assert!(info(&gz).contains("Internal compression: gzip"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The CLI spells it `--centre` everywhere else, so the flag is
/// `--pmtiles-centre`; `--pmtiles-center` stays as a hidden alias.
#[test]
fn pmtiles_bounds_and_centre_land_in_the_header() {
    let dir = unique_dir("pm-bounds");
    let (out, archive) = archive_run(
        &dir,
        "b.pmtiles",
        &[
            "--pmtiles-bounds",
            "-10,-5,10,5",
            "--pmtiles-centre",
            "1.5,2.5,3",
        ],
    );
    ok(&out, "the archive run");
    let text = info(&archive);
    assert!(
        text.contains("Bounds: -10.000000,-5.000000,10.000000,5.000000"),
        "{text}"
    );
    assert!(text.contains("Center: 1.500000,2.500000 zoom 3"), "{text}");
    let (out, alias) = archive_run(&dir, "a.pmtiles", &["--pmtiles-center", "4,5"]);
    ok(&out, "the --pmtiles-center alias");
    assert!(info(&alias).contains("Center: 4.000000,5.000000"));
    let help = run(&["pyramid", "--help"]);
    assert!(stdout(&help).contains("--pmtiles-centre"));
    assert!(
        !stdout(&help).contains("--pmtiles-center "),
        "the alias stays out of the help"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pmtiles_flags_are_refused_where_they_mean_nothing() {
    let dir = unique_dir("pm-bad");
    let input = off_grid(&dir);
    for flags in [
        vec!["--pmtiles-name", "x"],
        vec!["--pmtiles-description", "x"],
        vec!["--pmtiles-attribution", "x"],
        vec!["--pmtiles-internal-compression", "none"],
        vec!["--pmtiles-bounds", "-10,-5,10,5"],
        vec!["--pmtiles-centre", "1,2"],
    ] {
        let (out, _) = tree_run(&dir, &input, "t", &flags);
        refused(&out, &format!("{flags:?} into a tree"));
    }
    for bad in [
        vec!["--pmtiles-bounds", "10,-5,-10,5"],
        vec!["--pmtiles-bounds", "-10,5,10,-5"],
        vec!["--pmtiles-bounds", "-190,-5,10,5"],
        vec!["--pmtiles-bounds", "1,2,3"],
        vec!["--pmtiles-centre", "200,0"],
        vec!["--pmtiles-centre", "0,95"],
        vec!["--pmtiles-centre", "0,0,40"],
        vec!["--pmtiles-internal-compression", "brotli"],
    ] {
        let (out, _) = archive_run(&dir, "bad.pmtiles", &bad);
        refused(&out, &format!("{bad:?}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// webpsave --keep
// ---------------------------------------------------------------------------

/// A JPEG with an EXIF APP1 segment carrying `payload`: a plain viprs JPEG with
/// the segment spliced in after SOI.
fn jpeg_with_exif(dir: &Path, payload: &[u8]) -> PathBuf {
    let ppm = dir.join("small.ppm");
    write_ppm(&ppm, 16, 16, |x, y| [x as u8 * 16, y as u8 * 16, 128]);
    let plain = dir.join("plain.jpg");
    let out = run(&["jpegsave", s(&ppm), s(&plain)]);
    ok(&out, "jpegsave");
    let jpeg = std::fs::read(&plain).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG starts with SOI");
    let mut body = b"Exif\0\0".to_vec();
    body.extend_from_slice(payload);
    let len = u16::try_from(body.len() + 2).unwrap();
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&jpeg[2..]);
    let path = dir.join("exif.jpg");
    std::fs::write(&path, out).unwrap();
    path
}

/// The RIFF chunk ids of a WebP file, in order.
fn webp_chunks(bytes: &[u8]) -> Vec<String> {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");
    let mut ids = Vec::new();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        ids.push(String::from_utf8_lossy(&bytes[at..at + 4]).into_owned());
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        at += 8 + size + (size & 1);
    }
    ids
}

#[test]
fn webpsave_keep_all_carries_the_exif_and_keep_none_drops_it() {
    let dir = unique_dir("webp-keep");
    let payload = b"MM\0*\0\0\0\x08\0\0viprs-152";
    let jpeg = jpeg_with_exif(&dir, payload);
    let kept = dir.join("kept.webp");
    let out = run(&[
        "webpsave",
        s(&jpeg),
        s(&kept),
        "--lossless",
        "--keep",
        "all",
    ]);
    ok(&out, "webpsave --keep all");
    let bytes = std::fs::read(&kept).unwrap();
    assert!(
        webp_chunks(&bytes).iter().any(|c| c == "EXIF"),
        "--keep all must write the EXIF chunk: {:?}",
        webp_chunks(&bytes)
    );
    assert!(
        bytes.windows(payload.len()).any(|w| w == payload),
        "the EXIF chunk must carry the source's payload"
    );
    for (name, extra) in [("none", vec!["--keep", "none"]), ("default", vec![])] {
        let path = dir.join(format!("{name}.webp"));
        let mut args = vec!["webpsave", s(&jpeg), s(&path), "--lossless"];
        args.extend_from_slice(&extra);
        let out = run(&args);
        ok(&out, name);
        let chunks = webp_chunks(&std::fs::read(&path).unwrap());
        assert!(
            !chunks.iter().any(|c| c == "EXIF"),
            "{name} must strip the metadata: {chunks:?}"
        );
    }
    let out = run(&[
        "webpsave",
        s(&jpeg),
        s(&dir.join("x.webp")),
        "--lossless",
        "--keep",
        "exif",
    ]);
    assert_eq!(
        code(&out),
        2,
        "the core keeps all or none, so a per-chunk keep is a usage error: {}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// PDFium strip streaming: --render-stream, --min-dpi
// ---------------------------------------------------------------------------

fn write_minimal_pdf(path: &Path, width_pts: u32, height_pts: u32) {
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

fn pdf_run(dir: &Path, name: &str, extra: &[&str]) -> (Output, PathBuf) {
    let pdf = dir.join("letter.pdf");
    write_minimal_pdf(&pdf, 612, 792);
    let out_dir = dir.join(name);
    let mut args = vec!["pyramid", s(&pdf), s(&out_dir)];
    args.extend_from_slice(extra);
    (run(&args), out_dir)
}

#[test]
fn pdfium_streaming_flags_refuse_what_they_cannot_mean() {
    let dir = unique_dir("pdf-refusals");
    let tree_flags = ["--storage", "directory", "--render"];
    let cases: Vec<Vec<&str>> = vec![
        // No --render: there is no PDFium source to stream.
        vec![
            "--storage",
            "directory",
            "--memory-budget",
            "8",
            "--min-dpi",
            "36",
        ],
        vec![
            "--storage",
            "directory",
            "--memory-budget",
            "8",
            "--render-stream",
        ],
        // No --memory-budget: nothing to fit the strips or the DPI to.
        [&tree_flags[..], &["--render-stream"]].concat(),
        [&tree_flags[..], &["--min-dpi", "36"]].concat(),
        // A zero budget means "pick one for me", which a DPI floor cannot be fitted to.
        [
            &tree_flags[..],
            &["--memory-budget", "0", "--render-stream"],
        ]
        .concat(),
        // A floor above the DPI asked for.
        [
            &tree_flags[..],
            &["--memory-budget", "8", "--dpi", "100", "--min-dpi", "200"],
        ]
        .concat(),
        // Into an archive, which the streaming drivers do not write.
        vec!["--render", "--memory-budget", "8", "--render-stream"],
    ];
    for extra in cases {
        let (out, out_dir) = pdf_run(&dir, "t", &extra);
        refused(&out, &format!("{extra:?}"));
        assert!(
            !out_dir.exists(),
            "{extra:?}: a refused run must write nothing"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The strip path never decodes the page into one raster, so the decoded-size
/// check behind --memory-limit has nothing to price; rather than ignore the
/// flag, the run refuses it, and --memory-budget is what bounds this path.
#[test]
fn memory_limit_is_refused_beside_the_pdfium_strip_flags() {
    let dir = unique_dir("pdf-memory-limit");
    for flag in [&["--render-stream"][..], &["--min-dpi", "36"][..]] {
        let mut args = vec![
            "--storage",
            "directory",
            "--render",
            "--memory-budget",
            "64",
            "--memory-limit",
            "1",
        ];
        args.extend_from_slice(flag);
        let (out, out_dir) = pdf_run(&dir, "t", &args);
        refused(&out, &format!("{flag:?} with --memory-limit"));
        assert!(stderr(&out).contains("--memory-limit"), "{}", stderr(&out));
        assert!(!out_dir.exists(), "a refused run must write nothing");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_stream_needs_a_pdf_and_a_fresh_run() {
    let dir = unique_dir("pdf-fresh");
    let input = off_grid(&dir);
    let out = run(&[
        "pyramid",
        s(&input),
        s(&dir.join("tree")),
        "--storage",
        "directory",
        "--render",
        "--memory-budget",
        "8",
        "--render-stream",
    ]);
    refused(&out, "--render-stream on a PPM");
    assert!(
        stderr(&out).contains("needs a PDF input"),
        "{}",
        stderr(&out)
    );
    let (out, out_dir) = pdf_run(
        &dir,
        "resumed",
        &[
            "--storage",
            "directory",
            "--render",
            "--memory-budget",
            "8",
            "--render-stream",
            "--resume",
        ],
    );
    refused(&out, "--render-stream with --resume");
    assert!(stderr(&out).contains("fresh"), "{}", stderr(&out));
    assert!(!out_dir.exists(), "a refused run must write nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(feature = "pdfium"))]
#[test]
fn render_stream_without_pdfium_exits_1_naming_it() {
    let dir = unique_dir("pdf-nofeature");
    let (out, _) = pdf_run(
        &dir,
        "t",
        &[
            "--storage",
            "directory",
            "--render",
            "--memory-budget",
            "8",
            "--render-stream",
        ],
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pdfium"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "pdfium")]
fn no_libpdfium(out: &Output) -> bool {
    code(out) != 0 && stderr(out).to_lowercase().contains("libpdfium")
}

#[cfg(feature = "pdfium")]
#[test]
fn render_stream_writes_the_page_through_strips() {
    let dir = unique_dir("pdf-stream");
    let (out, out_dir) = pdf_run(
        &dir,
        "t",
        &[
            "--storage",
            "directory",
            "--layout",
            "xyz",
            "--render",
            "--memory-budget",
            "64",
            "--render-stream",
        ],
    );
    if no_libpdfium(&out) {
        eprintln!("skipping: no libpdfium here");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    ok(&out, "the streamed render");
    assert!(stderr(&out).contains("streaming"), "{}", stderr(&out));
    assert!(!tiles(&out_dir, "png").is_empty(), "no tiles written");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Letter at 600 DPI is 5100 px wide, so one 2 x 64-row RGBA strip is 2.6 MB;
/// a 1 MB budget fits only at 200 DPI or so, and `AutoAdjustDpi` has to go there.
#[cfg(feature = "pdfium")]
#[test]
fn min_dpi_lowers_the_dpi_until_a_strip_fits() {
    let dir = unique_dir("pdf-mindpi");
    let (out, _) = pdf_run(
        &dir,
        "t",
        &[
            "--storage",
            "directory",
            "--layout",
            "xyz",
            "--tile-size",
            "64",
            "--render",
            "--render-stream",
            "--memory-budget",
            "1",
            "--dpi",
            "600",
            "--min-dpi",
            "36",
        ],
    );
    if no_libpdfium(&out) {
        eprintln!("skipping: no libpdfium here");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    ok(&out, "the auto-adjusted render");
    assert!(
        stderr(&out).contains("--dpi 600 lowered to"),
        "the run must say what DPI it settled on:\n{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
