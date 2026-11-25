//! libviprs-cli#157, `viprs plot`: a DWG's print layouts, or its model space, to a PDF, PMTiles or PVEC,
//! with a plot style table and a plotter config. The design is the consensus comment on libviprs#1670
//! (D2, D3, D6). These run the real binary and read what it wrote: the PDF's pages and strokes, the
//! sheet index, the entity count of the model-space file. Exit codes alone are not evidence.
//!
//! Every plot file here is built in the test (a CTB from the core's own table type, a PC3 from a few
//! lines of JSON, a model-space drawing from acadrust), so nothing private is committed. The one cell
//! that runs on a real drawing is gated on `VIPRS_OWNER_PLOT_DIR` and skips, saying so, without it.

#![allow(dead_code)]

#[cfg(all(unix, feature = "pvec-convert"))]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
#[cfg(all(unix, feature = "pvec-convert"))]
use std::process::Stdio;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vector")
        .join(name)
}

fn run(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("viprs must spawn");
    // A binary with no `plot` command exits 2 too, which would pass every usage cell for the wrong reason.
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("unrecognized subcommand") && !err.contains("no family owns the command"),
        "viprs has no plot command:\n{err}"
    );
    out
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
    let dir = std::env::temp_dir().join(format!("viprs-plot-157-{}-{tag}", std::process::id()));
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

/// A drawing with three paper layouts (Layout1, Layout2, MyLayout), none of them empty.
fn sample() -> PathBuf {
    fixture("sample_AC1018.dwg")
}

/// A CTB: the identity table, or one that draws every colour in pure red.
#[cfg(feature = "pvec-convert")]
fn ctb(red: bool) -> Vec<u8> {
    use libviprs::cad::plot_files::PlotStyleTable;
    let mut table = PlotStyleTable::identity("synthetic.ctb");
    if red {
        for entry in table.aci_entries.iter_mut().skip(1) {
            entry.color = Some([255, 0, 0]);
        }
    }
    table.to_bytes().expect("the table writes")
}

/// A JSON PC3 of `paper` millimetres, laid out the way the PDF driver writes it.
fn pc3(paper: [f64; 2], printable: [f64; 4]) -> Vec<u8> {
    format!(
        "PIAFILEVERSION_3.0,json\n{{\n\"data\" : {{\n\"custom\" : {{\n\
         \"0\" : {{ \"name\" : \"Raster_Limit\", \"value\" : 400 }},\n\
         \"1\" : {{ \"name\" : \"Font_Capture\", \"value\" : true }}\n}},\n\"media\" : {{\n\
         \"actual_printable_bounds_llx\" : {},\n\"actual_printable_bounds_lly\" : {},\n\
         \"actual_printable_bounds_urx\" : {},\n\"actual_printable_bounds_ury\" : {},\n\
         \"dm_orientation\" : 1,\n\"size\" : {{\n\"landscape_mode\" : false,\n\
         \"media_description\" : {{ \"media_bounds\" : {{ \"urx\" : {}, \"ury\" : {} }} }},\n\
         \"name\" : \"SYNTHETIC_SHEET\"\n}}\n}}\n}}\n}}\n",
        printable[0], printable[1], printable[2], printable[3], paper[0], paper[1]
    )
    .into_bytes()
}

fn files(dir: &Path) -> (PathBuf, PathBuf) {
    let ctb_path = dir.join("mono.ctb");
    let pc3_path = dir.join("device.pc3");
    #[cfg(feature = "pvec-convert")]
    std::fs::write(&ctb_path, ctb(false)).unwrap();
    #[cfg(not(feature = "pvec-convert"))]
    std::fs::write(&ctb_path, b"not read, the cell is a usage one").unwrap();
    std::fs::write(&pc3_path, pc3([200.0, 100.0], [5.0, 10.0, 195.0, 90.0])).unwrap();
    (ctb_path, pc3_path)
}

/// The page sizes of a PDF, in points, in page order.
#[cfg(feature = "pvec-convert")]
fn pages(pdf: &Path) -> Vec<(f64, f64)> {
    libviprs::pdf_info(pdf)
        .unwrap_or_else(|e| panic!("{}: {e}", pdf.display()))
        .pages
        .iter()
        .map(|p| (p.width_pts, p.height_pts))
        .collect()
}

/// How many stroke or fill colour operators in the PDF's pages set pure red.
#[cfg(feature = "pvec-convert")]
fn reds(pdf: &Path) -> usize {
    use lopdf::{Document, Object};
    let num = |o: &Object| match o {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => f64::from(*r),
        _ => f64::NAN,
    };
    let doc = Document::load(pdf).unwrap();
    doc.get_pages()
        .values()
        .flat_map(|id| doc.get_and_decode_page_content(*id).unwrap().operations)
        .filter(|op| op.operator == "RG" || op.operator == "rg")
        .filter(|op| {
            let v: Vec<f64> = op.operands.iter().map(num).collect();
            v == [1.0, 0.0, 0.0]
        })
        .count()
}

/// The files the sheet index of `<stem>.sheets.json` lists.
fn indexed(dir: &Path, stem: &str) -> Vec<String> {
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(format!("{stem}.sheets.json"))).unwrap())
            .unwrap();
    index["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["file"].as_str().unwrap().to_owned())
        .collect()
}

// ---- a model-space drawing, written here with acadrust -------------------------------------------

/// Seven layers with one LINE each in model space and two stored layer states: `Saved` (the table as
/// the file has it) and `Review`, which restores only on/off and frozen (WALLS off, OFF-L on, FROZEN-L
/// thawed). Under the default profile the table keeps five lines, and Review keeps six.
#[cfg(feature = "pvec-convert")]
fn model_drawing(dir: &Path) -> PathBuf {
    use acadrust::entities::Line;
    use acadrust::tables::Layer;
    use acadrust::{CadDocument, DxfWriter, EntityType, LayerStateMask};
    let layers: [(&str, bool, bool, bool, bool); 7] = [
        ("WALLS", false, false, false, true),
        ("DOORS", false, false, false, true),
        ("OFF-L", true, false, false, true),
        ("FROZEN-L", false, true, false, true),
        ("LOCKED-L", false, false, true, true),
        ("NOPLOT-L", false, false, false, false),
        ("ANNOT", false, false, false, true),
    ];
    let mut doc = CadDocument::new();
    for (name, off, frozen, locked, plots) in layers {
        doc.layers
            .add(Layer::new(name))
            .expect("the layer is added");
        let layer = doc.layers.get_mut(name).expect("the layer exists");
        layer.flags.off = off;
        layer.flags.frozen = frozen;
        layer.flags.locked = locked;
        layer.is_plottable = plots;
    }
    doc.assign_table_entry_handles();
    for (n, (name, ..)) in layers.iter().enumerate() {
        let y = n as f64 * 10.0;
        let mut line = Line::from_coords(0.0, y, 0.0, 10.0, y, 0.0);
        line.common.layer = (*name).into();
        doc.add_entity(EntityType::Line(line))
            .expect("the line is added");
    }
    doc.capture_layer_state("Saved", "the table as the file has it");
    let mut review = doc.layer_state("Saved").expect("Saved is stored");
    review.name = "Review".into();
    review.mask = LayerStateMask::ON | LayerStateMask::FROZEN;
    for l in &mut review.layers {
        match l.layer_name.as_str() {
            "WALLS" => l.off = true,
            "OFF-L" => l.off = false,
            "FROZEN-L" => l.frozen = false,
            _ => {}
        }
    }
    review.layers.retain(|l| l.layer_name != "ANNOT");
    doc.store_layer_state(review);
    let path = dir.join("model.dxf");
    std::fs::write(
        &path,
        DxfWriter::new(&doc).write_to_vec().expect("the DXF writes"),
    )
    .unwrap();
    path
}

/// The N of the "(N entities)" the model line of a plot prints.
fn entities(err: &str) -> u64 {
    let line = err
        .lines()
        .find(|l| l.starts_with("model") && l.contains(" entities)"))
        .unwrap_or_else(|| panic!("no model line in:\n{err}"));
    let open = line.rfind('(').unwrap();
    line[open + 1..].split(' ').next().unwrap().parse().unwrap()
}

// ---- usage: exit 2, nothing written ---------------------------------------------------------------

/// `--pc3` without `--ctb` is a usage error: the supported inputs are a CTB alone or both.
#[test]
fn a_pc3_without_a_ctb_exits_2_and_writes_nothing() {
    let dir = scratch("pc3-alone");
    let (_, pc3_path) = files(&dir);
    let out_pdf = dir.join("out.pdf");
    let out = run(&["plot", s(&sample()), s(&out_pdf), "--pc3", s(&pc3_path)]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--ctb"), "{}", stderr(&out));
    assert!(!out_pdf.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// The format comes from OUT when OUT is a `.pdf`; anything else has to say.
#[test]
fn an_out_that_is_not_a_pdf_needs_a_format() {
    let dir = scratch("no-format");
    let out = run(&["plot", s(&sample()), s(&dir.join("sheets"))]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--format"), "{}", stderr(&out));
    assert!(!dir.join("sheets").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unknown_format_exits_2() {
    let dir = scratch("bad-format");
    let out = run(&["plot", s(&sample()), s(&dir.join("o")), "--format", "svg"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

/// `--model` and `--layout` choose what to plot, so they cannot both be given.
#[test]
fn model_and_layout_together_exit_2() {
    let dir = scratch("model-layout");
    let out = run(&[
        "plot",
        s(&sample()),
        s(&dir.join("o.pdf")),
        "--model",
        "--layout",
        "Layout1",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

/// OUT is never the drawing, the CTB or the PC3: the plot would overwrite its own input.
#[test]
fn an_out_equal_to_an_input_is_refused_and_the_input_is_left_alone() {
    let dir = scratch("out-is-input");
    let (ctb_path, pc3_path) = files(&dir);
    let drawing = dir.join("copy.dwg");
    std::fs::copy(sample(), &drawing).unwrap();
    let before = [&drawing, &ctb_path, &pc3_path].map(|p| std::fs::read(p).unwrap());
    for (out_path, case) in [
        (&drawing, "drawing"),
        (&ctb_path, "ctb"),
        (&pc3_path, "pc3"),
    ] {
        let out = run(&[
            "plot",
            s(&drawing),
            s(out_path),
            "--format",
            "pdf",
            "--ctb",
            s(&ctb_path),
            "--pc3",
            s(&pc3_path),
        ]);
        assert_eq!(code(&out), 2, "{case}: {}", stderr(&out));
    }
    let after = [&drawing, &ctb_path, &pc3_path].map(|p| std::fs::read(p).unwrap());
    assert_eq!(before, after, "an input was written");
    // The same file reached through another spelling of its path is the same file.
    let sideways = dir.join(".").join("copy.dwg");
    let out = run(&["plot", s(&drawing), s(&sideways), "--format", "pdf"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert_eq!(std::fs::read(&drawing).unwrap(), before[0]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A hard link to an input is the input: OUT is refused when it is the drawing, the CTB or the PC3 under another name.
#[cfg(unix)]
#[test]
fn an_out_that_is_a_hard_link_to_an_input_is_refused_and_the_input_is_left_alone() {
    let dir = scratch("out-hardlink");
    let (ctb_path, pc3_path) = files(&dir);
    let drawing = dir.join("copy.dwg");
    std::fs::copy(sample(), &drawing).unwrap();
    let before = [&drawing, &ctb_path, &pc3_path].map(|p| std::fs::read(p).unwrap());
    for (input, case) in [
        (&drawing, "drawing"),
        (&ctb_path, "ctb"),
        (&pc3_path, "pc3"),
    ] {
        let link = dir.join(format!("{case}-link.pdf"));
        std::fs::hard_link(input, &link).unwrap();
        let out = run(&[
            "plot",
            s(&drawing),
            s(&link),
            "--format",
            "pdf",
            "--ctb",
            s(&ctb_path),
            "--pc3",
            s(&pc3_path),
        ]);
        assert_eq!(code(&out), 2, "{case}: {}", stderr(&out));
    }
    let after = [&drawing, &ctb_path, &pc3_path].map(|p| std::fs::read(p).unwrap());
    assert_eq!(before, after, "an input was written through a hard link");
    let _ = std::fs::remove_dir_all(dir);
}

/// A symlink to an input is refused too, and the link is not replaced by the plot.
#[cfg(unix)]
#[test]
fn an_out_that_is_a_symlink_to_an_input_is_refused_and_the_input_is_left_alone() {
    let dir = scratch("out-symlink");
    let (ctb_path, pc3_path) = files(&dir);
    let drawing = dir.join("copy.dwg");
    std::fs::copy(sample(), &drawing).unwrap();
    let before = [&drawing, &ctb_path, &pc3_path].map(|p| std::fs::read(p).unwrap());
    for (input, case) in [
        (&drawing, "drawing"),
        (&ctb_path, "ctb"),
        (&pc3_path, "pc3"),
    ] {
        let link = dir.join(format!("{case}-sym.pdf"));
        std::os::unix::fs::symlink(input, &link).unwrap();
        let out = run(&[
            "plot",
            s(&drawing),
            s(&link),
            "--format",
            "pdf",
            "--ctb",
            s(&ctb_path),
            "--pc3",
            s(&pc3_path),
        ]);
        assert_eq!(code(&out), 2, "{case}: {}", stderr(&out));
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    let after = [&drawing, &ctb_path, &pc3_path].map(|p| std::fs::read(p).unwrap());
    assert_eq!(before, after, "an input was written through a symlink");
    let _ = std::fs::remove_dir_all(dir);
}

/// A stem is a plain file name, so nothing is written outside OUT.
#[test]
fn a_stem_with_a_separator_is_refused() {
    let dir = scratch("stem");
    for stem in ["../x", "a/b", "a\\b", "..", ".", ""] {
        let out = run(&[
            "plot",
            s(&sample()),
            s(&dir.join("o")),
            "--format",
            "pvec",
            "--stem",
            stem,
        ]);
        assert_eq!(code(&out), 2, "{stem:?}: {}", stderr(&out));
    }
    assert_eq!(listing(&dir), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_out_of_the_wrong_kind_is_refused() {
    let dir = scratch("out-kind");
    // A directory where a PDF goes.
    let out = run(&["plot", s(&sample()), s(&dir), "--format", "pdf"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    // A file where a directory goes.
    let file = dir.join("file");
    std::fs::write(&file, b"x").unwrap();
    let out = run(&["plot", s(&sample()), s(&file), "--format", "pvec"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert_eq!(std::fs::read(&file).unwrap(), b"x");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_deadline_that_is_not_a_positive_number_exits_2() {
    let dir = scratch("deadline-usage");
    for bad in ["0", "-1", "nan", "soon"] {
        let out = run(&[
            "plot",
            s(&sample()),
            s(&dir.join("o.pdf")),
            "--deadline",
            bad,
        ]);
        assert_eq!(code(&out), 2, "{bad}: {}", stderr(&out));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_drawing_exits_1() {
    let dir = scratch("missing");
    let out = run(&["plot", s(&dir.join("nope.dwg")), s(&dir.join("o.pdf"))]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- PDF ------------------------------------------------------------------------------------------

/// With no `--layout` every print layout is a page, in tab order, and a plot with no CTB says so.
#[cfg(feature = "pvec-convert")]
#[test]
fn every_print_layout_is_a_page_by_default() {
    let dir = scratch("pdf-all");
    let out_pdf = dir.join("out.pdf");
    let out = run(&["plot", s(&sample()), s(&out_pdf)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(std::fs::read(&out_pdf).unwrap().starts_with(b"%PDF-"));
    assert_eq!(pages(&out_pdf).len(), 3);
    assert!(
        stderr(&out).contains("no --ctb"),
        "a plot with no style table says it draws object colours:\n{}",
        stderr(&out)
    );
    // Only the PDF is left in the directory: no scratch directory, no temporary file.
    assert_eq!(listing(&dir), ["out.pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn layout_picks_pages_and_matches_with_case_ignored() {
    let dir = scratch("pdf-some");
    let all = dir.join("all.pdf");
    let some = dir.join("some.pdf");
    assert_eq!(code(&run(&["plot", s(&sample()), s(&all)])), 0);
    let out = run(&[
        "plot",
        s(&sample()),
        s(&some),
        "--layout",
        "mylayout",
        "--layout",
        "Layout1",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // Tab order, whatever order they were asked for: Layout1 then MyLayout.
    let all = pages(&all);
    assert_eq!(pages(&some), [all[0], all[2]]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A layout the drawing does not have exits 1 and writes nothing, and a PDF that was already at OUT
/// is the same bytes afterwards (the write is atomic).
#[cfg(feature = "pvec-convert")]
#[test]
fn an_unknown_layout_exits_1_and_leaves_what_was_at_out() {
    let dir = scratch("pdf-unknown");
    let out_pdf = dir.join("out.pdf");
    std::fs::write(&out_pdf, b"%PDF-1.7 earlier plot").unwrap();
    let out = run(&["plot", s(&sample()), s(&out_pdf), "--layout", "Layout9"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("Layout9"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("MyLayout"),
        "the layouts there are: {}",
        stderr(&out)
    );
    assert_eq!(std::fs::read(&out_pdf).unwrap(), b"%PDF-1.7 earlier plot");
    assert_eq!(listing(&dir), ["out.pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The CTB is applied: a table that draws every colour red puts red strokes on the page, and the plot
/// without it has none.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_ctb_is_applied_to_the_pages() {
    let dir = scratch("pdf-ctb");
    let red = dir.join("red.ctb");
    std::fs::write(&red, ctb(true)).unwrap();
    let plain = dir.join("plain.pdf");
    let styled = dir.join("styled.pdf");
    assert_eq!(code(&run(&["plot", s(&sample()), s(&plain)])), 0);
    let out = run(&["plot", s(&sample()), s(&styled), "--ctb", s(&red)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!stderr(&out).contains("no --ctb"), "{}", stderr(&out));
    assert_eq!(reds(&plain), 0);
    assert!(
        reds(&styled) > 0,
        "no red stroke in the plot made with the red CTB"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A layout's own page setup wins over the PC3: the PC3's 200 x 100 mm sheet does not become the page.
#[cfg(feature = "pvec-convert")]
#[test]
fn the_layouts_page_setup_wins_over_the_pc3() {
    let dir = scratch("pdf-pc3");
    let (ctb_path, pc3_path) = files(&dir);
    let without = dir.join("without.pdf");
    let with = dir.join("with.pdf");
    let a = run(&["plot", s(&sample()), s(&without), "--ctb", s(&ctb_path)]);
    assert_eq!(code(&a), 0, "{}", stderr(&a));
    let b = run(&[
        "plot",
        s(&sample()),
        s(&with),
        "--ctb",
        s(&ctb_path),
        "--pc3",
        s(&pc3_path),
    ]);
    assert_eq!(code(&b), 0, "{}", stderr(&b));
    let sheet_200_by_100 = (200.0 * 72.0 / 25.4, 100.0 * 72.0 / 25.4);
    let with = pages(&with);
    assert_eq!(with, pages(&without));
    assert!(
        with.iter().all(|p| (p.0 - sheet_200_by_100.0).abs() > 1.0),
        "a page took the PC3's sheet: {with:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- the plot files are read through the core's loaders -------------------------------------------

/// A CTB over 1 MiB is refused with the core's code before anything is plotted.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_ctb_over_the_cap_exits_1_with_the_core_code() {
    let dir = scratch("ctb-big");
    let big = dir.join("big.ctb");
    let mut bytes = ctb(false);
    bytes.resize(1024 * 1024 + 1, 0);
    std::fs::write(&big, bytes).unwrap();
    let out_pdf = dir.join("out.pdf");
    let out = run(&["plot", s(&sample()), s(&out_pdf), "--ctb", s(&big)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-file-too-large"),
        "{}",
        stderr(&out)
    );
    assert!(!out_pdf.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// A small CTB that inflates past 8 MiB is refused, whatever its header announces.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_ctb_that_inflates_past_the_bound_exits_1_with_the_core_code() {
    // The header the CTB reader checks: 48 bytes of text, the checksum, the text length and the
    // compressed length, then the zlib payload. A stored deflate block of spaces would be as big as
    // the text, so this is a zlib stream of a long run, made with the encoder the core uses.
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&vec![b' '; 9 * 1024 * 1024]).unwrap();
    let payload = z.finish().unwrap();
    let mut file = b"PIAFILEVERSION_2.0,CTBVER1,compress\r\npmzlibcodec".to_vec();
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&100u32.to_le_bytes());
    file.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    file.extend_from_slice(&payload);
    assert!(file.len() < 64 * 1024);
    let dir = scratch("ctb-bomb");
    let bomb = dir.join("bomb.ctb");
    std::fs::write(&bomb, file).unwrap();
    let out = run(&[
        "plot",
        s(&sample()),
        s(&dir.join("o.pdf")),
        "--ctb",
        s(&bomb),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-file-inflated-too-large"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "pvec-convert")]
#[test]
fn a_file_that_is_no_plot_file_or_is_missing_exits_1() {
    let dir = scratch("ctb-junk");
    let junk = dir.join("junk.ctb");
    std::fs::write(&junk, b"this is not a plot style table").unwrap();
    let out = run(&[
        "plot",
        s(&sample()),
        s(&dir.join("o.pdf")),
        "--ctb",
        s(&junk),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-file-unrecognised"),
        "{}",
        stderr(&out)
    );
    let (ctb_path, _) = files(&dir);
    let junk_pc3 = dir.join("junk.pc3");
    std::fs::write(&junk_pc3, b"nor is this").unwrap();
    let out = run(&[
        "plot",
        s(&sample()),
        s(&dir.join("o.pdf")),
        "--ctb",
        s(&ctb_path),
        "--pc3",
        s(&junk_pc3),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-file-unrecognised"),
        "{}",
        stderr(&out)
    );
    let out = run(&[
        "plot",
        s(&sample()),
        s(&dir.join("o.pdf")),
        "--ctb",
        s(&dir.join("absent.ctb")),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(!dir.join("o.pdf").exists());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- PVEC -----------------------------------------------------------------------------------------

/// `--format pvec` writes one file per sheet and the index, as `pvec convert` does, and `--layout` picks.
#[cfg(feature = "pvec-convert")]
#[test]
fn pvec_writes_one_file_per_sheet_and_the_index_last() {
    let dir = scratch("pvec-all");
    let (ctb_path, pc3_path) = files(&dir);
    let sheets = dir.join("sheets");
    let out = run(&[
        "plot",
        s(&sample()),
        s(&sheets),
        "--format",
        "pvec",
        "--ctb",
        s(&ctb_path),
        "--pc3",
        s(&pc3_path),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let names = indexed(&sheets, "sample_AC1018");
    assert_eq!(names.len(), 3, "{names:?}");
    for n in &names {
        assert!(sheets.join(n).is_file(), "{n}");
    }
    let some = dir.join("some");
    let out = run(&[
        "plot",
        s(&sample()),
        s(&some),
        "--format",
        "pvec",
        "--layout",
        "Layout2",
        "--stem",
        "plan",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let names = indexed(&some, "plan");
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with("plan.02-Layout2"), "{names:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Model space plots to one PVEC. With no layer state the layer table as saved decides; `--layer-state`
/// restores a stored state under its mask.
#[cfg(feature = "pvec-convert")]
#[test]
fn model_space_follows_the_saved_table_or_a_named_layer_state() {
    let dir = scratch("model");
    let drawing = model_drawing(&dir);
    let plain = dir.join("plain");
    let out = run(&[
        "plot",
        s(&drawing),
        s(&plain),
        "--format",
        "pvec",
        "--model",
        "--unit",
        "m",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(plain.join("model.pvec").is_file(), "{:?}", listing(&plain));
    assert_eq!(entities(&stderr(&out)), 5);

    let review = dir.join("review");
    let out = run(&[
        "plot",
        s(&drawing),
        s(&review),
        "--format",
        "pvec",
        "--model",
        "--unit",
        "m",
        "--layer-state",
        "Review",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(entities(&stderr(&out)), 6);

    // The state's name matches with case ignored, like a layout's.
    let lower = dir.join("lower");
    let out = run(&[
        "plot",
        s(&drawing),
        s(&lower),
        "--format",
        "pvec",
        "--model",
        "--unit",
        "m",
        "--layer-state",
        "review",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(entities(&stderr(&out)), 6);
    let _ = std::fs::remove_dir_all(dir);
}

/// An unknown state exits 1, lists the drawing's states, and writes nothing.
#[cfg(feature = "pvec-convert")]
#[test]
fn an_unknown_layer_state_exits_1_and_lists_the_states() {
    let dir = scratch("state-unknown");
    let drawing = model_drawing(&dir);
    let out_dir = dir.join("o");
    let out = run(&[
        "plot",
        s(&drawing),
        s(&out_dir),
        "--format",
        "pvec",
        "--model",
        "--unit",
        "m",
        "--layer-state",
        "Nope",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("Nope") && err.contains("Saved") && err.contains("Review"),
        "{err}"
    );
    assert!(
        !out_dir.exists() || listing(&out_dir).is_empty(),
        "{:?}",
        listing(&out_dir)
    );
    // Plotting layouts checks the state the same way, before the first page.
    let out_pdf = dir.join("o.pdf");
    let out = run(&["plot", s(&sample()), s(&out_pdf), "--layer-state", "Nope"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(!out_pdf.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// The core's PDF and PMTiles plots take paper layouts only, so model space is a PVEC. Saying so is
/// better than writing a page the core cannot make.
#[cfg(feature = "pvec-convert")]
#[test]
fn model_space_to_pdf_exits_1_and_says_which_format_has_it() {
    let dir = scratch("model-pdf");
    let drawing = model_drawing(&dir);
    let out_pdf = dir.join("o.pdf");
    let out = run(&["plot", s(&drawing), s(&out_pdf), "--model", "--unit", "m"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("--format pvec"), "{}", stderr(&out));
    assert!(!out_pdf.exists());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- PMTiles --------------------------------------------------------------------------------------

/// Without the `pdfium` feature PMTiles exits 1 and names it, before anything is written.
#[cfg(all(feature = "pvec-convert", not(feature = "pdfium")))]
#[test]
fn pmtiles_without_pdfium_exits_1_and_names_the_feature() {
    let dir = scratch("pmtiles-nofeature");
    let out_dir = dir.join("o");
    let out = run(&["plot", s(&sample()), s(&out_dir), "--format", "pmtiles"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pdfium"), "{}", stderr(&out));
    assert!(!out_dir.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// A dpi outside what the core renders is a usage error.
#[cfg(feature = "pdfium")]
#[test]
fn a_dpi_outside_36_to_600_exits_2() {
    let dir = scratch("dpi");
    for dpi in ["0", "35", "601", "x"] {
        let out = run(&[
            "plot",
            s(&sample()),
            s(&dir.join("o")),
            "--format",
            "pmtiles",
            "--dpi",
            dpi,
        ]);
        assert_eq!(code(&out), 2, "{dpi}: {}", stderr(&out));
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// One archive per sheet and the index. Rendering needs a libpdfium the machine may not have, and a
/// missing library is the only reason this cell skips.
#[cfg(all(feature = "pvec-convert", feature = "pdfium"))]
#[test]
fn pmtiles_writes_one_archive_per_sheet() {
    let dir = scratch("pmtiles");
    let out_dir = dir.join("tiles");
    let out = run(&[
        "plot",
        s(&sample()),
        s(&out_dir),
        "--format",
        "pmtiles",
        "--layout",
        "Layout1",
        "--dpi",
        "72",
    ]);
    let err = stderr(&out);
    if code(&out) == 1 && err.contains("RENDER_FAILED") && err.to_lowercase().contains("libpdfium")
    {
        eprintln!("skipped: no libpdfium on this machine:\n{err}");
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    assert_eq!(code(&out), 0, "{err}");
    let names = indexed(&out_dir, "sample_AC1018");
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].ends_with(".pmtiles"), "{names:?}");
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_dir.join("sample_AC1018.sheets.json")).unwrap())
            .unwrap();
    assert_eq!(
        index["source"], "sample_AC1018.dwg",
        "the index names the file, not the typed path"
    );
    let info = run(&["pmtiles", "verify", s(&out_dir.join(&names[0]))]);
    assert_eq!(code(&info), 0, "{}", stderr(&info));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- progress, deadline, Ctrl-C --------------------------------------------------------------------

/// `--progress` puts one "started" and one "finished" line per sheet on stderr, numbered over the sheets
/// asked for, and nothing else of the kind is printed without it.
#[cfg(feature = "pvec-convert")]
#[test]
fn progress_names_each_sheet_of_the_total_on_stderr() {
    let dir = scratch("progress");
    let quiet = run(&["plot", s(&sample()), s(&dir.join("a.pdf"))]);
    assert_eq!(code(&quiet), 0, "{}", stderr(&quiet));
    assert!(!stderr(&quiet).contains("[1/3]"), "{}", stderr(&quiet));
    let out = run(&["plot", s(&sample()), s(&dir.join("b.pdf")), "--progress"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let err = stderr(&out);
    for line in [
        "[1/3] Layout1 started",
        "[1/3] Layout1 written",
        "[3/3] MyLayout started",
        "[3/3] MyLayout written",
    ] {
        assert!(err.contains(line), "{line:?} not in:\n{err}");
    }
    assert!(out.stdout.is_empty(), "stdout carries nothing");
    let _ = std::fs::remove_dir_all(dir);
}

/// A deadline that has passed by the first page exits 1 with the core's code, and writes no PDF.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_deadline_stops_the_plot_with_exit_1_and_no_file() {
    let dir = scratch("deadline");
    let out_pdf = dir.join("o.pdf");
    let out = run(&["plot", s(&sample()), s(&out_pdf), "--deadline", "0.001"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("DEADLINE"), "{}", stderr(&out));
    assert!(!out_pdf.exists());
    assert_eq!(listing(&dir), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(dir);
}

/// Ctrl-C after the first sheet starts cancels cleanly: exit 130, no PDF and no scratch left. The signal
/// goes out on the first "[1/3]" line, before the first page is done, so the plot has something to cancel;
/// a run that still finishes first is tried again, and the cell fails if none of three is cancelled.
#[cfg(all(unix, feature = "pvec-convert"))]
#[test]
fn ctrl_c_cancels_through_the_token_and_leaves_nothing() {
    let dir = scratch("sigint");
    let out_pdf = dir.join("o.pdf");
    let mut last = String::new();
    for _ in 0..3 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_viprs"))
            .args(["plot", s(&sample()), s(&out_pdf), "--progress"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id().to_string();
        let mut seen = String::new();
        let mut sent = false;
        for line in BufReader::new(child.stderr.take().unwrap()).lines() {
            let line = line.unwrap();
            seen.push_str(&line);
            seen.push('\n');
            if !sent && line.contains("[1/3]") {
                let kill = Command::new("kill").args(["-INT", &pid]).status().unwrap();
                assert!(kill.success());
                sent = true;
            }
        }
        let status = child.wait().unwrap();
        assert!(sent, "no progress line before the end:\n{seen}");
        if status.code() == Some(130) {
            assert!(!out_pdf.exists());
            assert_eq!(listing(&dir), Vec::<String>::new());
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        last = seen;
        let _ = std::fs::remove_file(&out_pdf);
    }
    panic!("three plots were signalled and none was cancelled (exit 130):\n{last}");
}

// ---- builds ---------------------------------------------------------------------------------------

/// A build without `pvec-convert` exits 1 and names it, and a usage mistake is still exit 2.
#[cfg(not(feature = "pvec-convert"))]
#[test]
fn a_build_without_pvec_convert_exits_1_and_names_it() {
    let dir = scratch("nofeature");
    let out = run(&["plot", s(&sample()), s(&dir.join("o.pdf"))]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-convert"), "{}", stderr(&out));
    let out = run(&[
        "plot",
        s(&sample()),
        s(&dir.join("o.pdf")),
        "--pc3",
        "x.pc3",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

/// COVERAGE.md says the plot is covered and names the command, and no row still says it is missing.
#[test]
fn coverage_has_the_plot_row_as_covered() {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("COVERAGE.md")).unwrap();
    let row = text
        .lines()
        .find(|l| l.starts_with("| `PdfPlot`"))
        .expect("a row for PdfPlot");
    assert!(
        row.contains("viprs plot") && row.contains("covered"),
        "{row}"
    );
    assert!(!row.contains("missing"), "{row}");
}

// ---- the owner's drawing ----------------------------------------------------------------------------

/// Plots the one .dwg with its .ctb and .pc3 from `VIPRS_OWNER_PLOT_DIR`, a directory that is never
/// committed, and prints page counts only. Skips, saying so, without the variable.
#[cfg(feature = "pvec-convert")]
#[test]
fn owner_drawing_plots_to_a_pdf_with_its_style_table_and_plotter() {
    let Some(dir) = std::env::var_os("VIPRS_OWNER_PLOT_DIR").map(PathBuf::from) else {
        eprintln!("skipped: VIPRS_OWNER_PLOT_DIR is not set");
        return;
    };
    let one = |ext: &str| -> PathBuf {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
            .collect();
        assert_eq!(found.len(), 1, "one .{ext} file in the directory");
        found.remove(0)
    };
    let (dwg, ctb_path, pc3_path) = (one("dwg"), one("ctb"), one("pc3"));
    let work = scratch("owner");
    let out_pdf = work.join("plot.pdf");
    let out = run(&[
        "plot",
        s(&dwg),
        s(&out_pdf),
        "--ctb",
        s(&ctb_path),
        "--pc3",
        s(&pc3_path),
    ]);
    assert_eq!(code(&out), 0, "exit {}", code(&out));
    eprintln!("owner plot: {} pages", pages(&out_pdf).len());
    assert!(!pages(&out_pdf).is_empty());
    let _ = std::fs::remove_dir_all(work);
}
