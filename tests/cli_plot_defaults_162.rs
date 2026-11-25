//! libviprs-cli#162, `viprs plot-defaults` and the precedence walk of `viprs plot` (the CLI half of step 7 of the plan on
//! libviprs#1670, on the core's `cad::plot_defaults`, `default_plot_style()` and `default_pc3()`, libviprs#1712).
//!
//! These run the real binary and read what it wrote: the files under a config directory, the colours a
//! plotted PDF sets, what `show` says. Every plot file is built in the test and every config directory is a
//! scratch one named by `LIBVIPRS_CONFIG_DIR`, so nothing private is committed and nothing touches the real
//! home directory. No network.

#![allow(dead_code)]

#[cfg(feature = "pvec-convert")]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vector")
        .join(name)
}

/// Runs viprs with none of the variables the config directory or the plot walk read, and a HOME that
/// does not exist, then the `env` given. A test says what it needs and the real environment never leaks in.
fn go(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_viprs"));
    cmd.args(args);
    for var in [
        "LIBVIPRS_CONFIG_DIR",
        "LIBVIPRS_PLOT_CTB",
        "LIBVIPRS_PLOT_PC3",
        "XDG_CONFIG_HOME",
        "APPDATA",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("HOME", "/nonexistent-viprs-162-home");
    cmd.envs(env.iter().copied());
    let out = cmd.output().expect("viprs must spawn");
    // A binary with no such command exits 2 too, which would pass every usage cell for the wrong reason.
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("unrecognized subcommand") && !err.contains("no family owns the command"),
        "viprs has no plot-defaults command:\n{err}"
    );
    out
}

/// `go` with `cfg` as the config directory.
fn run(cfg: &Path, args: &[&str]) -> Output {
    go(args, &[("LIBVIPRS_CONFIG_DIR", s(cfg))])
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

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "viprs-plot-defaults-162-{}-{tag}",
        std::process::id()
    ));
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

/// A drawing with three paper layouts, none of them empty.
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

/// Writes `bytes` to `dir/name` and returns the path.
fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The stored default of `cfg`, as the core lays it out: `plot/default.ctb` or `plot/default.pc3`.
fn stored(cfg: &Path, name: &str) -> PathBuf {
    cfg.join("plot").join(name)
}

/// Every colour operator of the PDF's pages, as text such as `RG 0.000 0.000 0.000`.
#[cfg(feature = "pvec-convert")]
fn colour_ops(pdf: &Path) -> Vec<String> {
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
        .filter(|op| matches!(op.operator.as_str(), "RG" | "rg" | "G" | "g" | "K" | "k"))
        .map(|op| {
            let v: Vec<String> = op
                .operands
                .iter()
                .map(|o| format!("{:.3}", num(o)))
                .collect();
            format!("{} {}", op.operator, v.join(" "))
        })
        .collect()
}

/// How many stroke or fill colour operators in the PDF's pages set pure red.
#[cfg(feature = "pvec-convert")]
fn reds(pdf: &Path) -> usize {
    colour_ops(pdf)
        .iter()
        .filter(|c| matches!(c.as_str(), "RG 1.000 0.000 0.000" | "rg 1.000 0.000 0.000"))
        .count()
}

#[cfg(feature = "pvec-convert")]
fn colour_set(pdf: &Path) -> BTreeSet<String> {
    colour_ops(pdf).into_iter().collect()
}

#[cfg(feature = "pvec-convert")]
fn pages(pdf: &Path) -> Vec<(f64, f64)> {
    libviprs::pdf_info(pdf)
        .unwrap_or_else(|e| panic!("{}: {e}", pdf.display()))
        .pages
        .iter()
        .map(|p| (p.width_pts, p.height_pts))
        .collect()
}

// ---- usage: exit 2, nothing written -----------------------------------------------------------------

#[test]
fn plot_defaults_without_a_verb_exits_2() {
    let out = go(&["plot-defaults"], &[]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

#[test]
fn write_without_a_directory_exits_2() {
    let out = go(&["plot-defaults", "write"], &[]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

/// `set` with neither file has nothing to store, and it is the command line that is wrong.
#[test]
fn set_without_a_file_exits_2_and_stores_nothing() {
    let dir = scratch("set-none");
    let cfg = dir.join("cfg");
    let out = run(&cfg, &["plot-defaults", "set"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--ctb"), "{}", stderr(&out));
    assert!(!cfg.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unknown_verb_or_flag_exits_2() {
    let dir = scratch("unknown");
    let cfg = dir.join("cfg");
    assert_eq!(code(&run(&cfg, &["plot-defaults", "bake"])), 2);
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "clear", "--everything"])),
        2
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- write: the built-in files, as real files -------------------------------------------------------

/// `write DIR` puts the built-in `.ctb` and `.pc3` in DIR, byte for byte what the core builds, and they load
/// back through the core's loaders: 255 black entries at lineweight 9.
#[cfg(feature = "pvec-convert")]
#[test]
fn write_puts_the_two_built_in_files_in_dir() {
    use libviprs::cad::plot_files::{
        Pc3File, PlotDevice, default_pc3, default_plot_style, load_pc3, load_plot_style,
    };
    let dir = scratch("write");
    let out_dir = dir.join("out");
    let out = run(&dir.join("cfg"), &["plot-defaults", "write", s(&out_dir)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        listing(&out_dir),
        ["libviprs-monochrome.ctb", "libviprs-pdf.pc3"],
        "nothing but the two files"
    );
    let ctb_bytes = std::fs::read(out_dir.join("libviprs-monochrome.ctb")).unwrap();
    let pc3_bytes = std::fs::read(out_dir.join("libviprs-pdf.pc3")).unwrap();
    assert_eq!(ctb_bytes, default_plot_style().to_bytes().unwrap());
    assert_eq!(pc3_bytes, default_pc3().to_bytes());
    let table = load_plot_style("libviprs-monochrome.ctb", &ctb_bytes).unwrap();
    assert_eq!(
        table
            .aci_entries
            .iter()
            .skip(1)
            .filter(|e| e.color == Some([0, 0, 0]) && e.lineweight == 9)
            .count(),
        255
    );
    let Pc3File::Json(pc3) = load_pc3(&pc3_bytes).unwrap() else {
        panic!("the generated PC3 is the JSON kind");
    };
    PlotDevice::from_pc3(&pc3).expect("the generated PC3 names a device");
    // Writing the defaults is not storing them.
    assert!(!dir.join("cfg").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// An existing file is not replaced without `--force`, and with it the two files are rewritten.
#[cfg(feature = "pvec-convert")]
#[test]
fn write_will_not_replace_a_file_without_force() {
    let dir = scratch("write-force");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mine = put(&out_dir, "libviprs-pdf.pc3", b"my own file");
    let cfg = dir.join("cfg");
    let out = run(&cfg, &["plot-defaults", "write", s(&out_dir)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("--force"), "{}", stderr(&out));
    assert_eq!(std::fs::read(&mine).unwrap(), b"my own file");
    assert_eq!(
        listing(&out_dir),
        ["libviprs-pdf.pc3"],
        "the refusal comes before either file is written"
    );
    let out = run(&cfg, &["plot-defaults", "write", s(&out_dir), "--force"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_ne!(std::fs::read(&mine).unwrap(), b"my own file");
    assert_eq!(
        listing(&out_dir),
        ["libviprs-monochrome.ctb", "libviprs-pdf.pc3"]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A symlink where a default file would go is never written through, with or without `--force`.
#[cfg(all(unix, feature = "pvec-convert"))]
#[test]
fn write_refuses_a_symlink_at_a_target_even_with_force() {
    let dir = scratch("write-link");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let victim = put(&dir, "victim.txt", b"keep me");
    std::os::unix::fs::symlink(&victim, out_dir.join("libviprs-monochrome.ctb")).unwrap();
    for extra in [&[][..], &["--force"][..]] {
        let mut args = vec!["plot-defaults", "write", s(&out_dir)];
        args.extend_from_slice(extra);
        let out = run(&dir.join("cfg"), &args);
        assert_eq!(code(&out), 1, "{}", stderr(&out));
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
    }
    let _ = std::fs::remove_dir_all(dir);
}

// ---- set: a copy of the user's files under the config directory --------------------------------------

/// `set` stores copies of both files under `plot/`, under fixed names, byte for byte, and on unix the
/// directory is 0700 and the files 0600. The user's own file names are never used.
#[cfg(feature = "pvec-convert")]
#[test]
fn set_stores_copies_of_both_files_privately() {
    let dir = scratch("set");
    let cfg = dir.join("cfg");
    let mine_ctb = put(&dir, "my-office.ctb", &ctb(true));
    let device = pc3([420.0, 297.0], [5.0, 5.0, 415.0, 292.0]);
    let mine_pc3 = put(&dir, "my-plotter.pc3", &device);
    let out = run(
        &cfg,
        &[
            "plot-defaults",
            "set",
            "--ctb",
            s(&mine_ctb),
            "--pc3",
            s(&mine_pc3),
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        std::fs::read(stored(&cfg, "default.ctb")).unwrap(),
        ctb(true)
    );
    assert_eq!(std::fs::read(stored(&cfg, "default.pc3")).unwrap(), device);
    assert_eq!(listing(&cfg.join("plot")), ["default.ctb", "default.pc3"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&cfg.join("plot")), 0o700);
        assert_eq!(mode(&stored(&cfg, "default.ctb")), 0o600);
        assert_eq!(mode(&stored(&cfg, "default.pc3")), 0o600);
    }
    // The copy is a copy: changing the user's file later changes nothing stored.
    std::fs::write(&mine_ctb, ctb(false)).unwrap();
    assert_eq!(
        std::fs::read(stored(&cfg, "default.ctb")).unwrap(),
        ctb(true)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Storing one file leaves the other stored one as it was.
#[cfg(feature = "pvec-convert")]
#[test]
fn set_one_file_leaves_the_other_alone() {
    let dir = scratch("set-one");
    let cfg = dir.join("cfg");
    let a = put(&dir, "a.ctb", &ctb(true));
    let device = pc3([200.0, 100.0], [5.0, 10.0, 195.0, 90.0]);
    let b = put(&dir, "b.pc3", &device);
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "set", "--ctb", s(&a)])),
        0
    );
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "set", "--pc3", s(&b)])),
        0
    );
    assert_eq!(
        std::fs::read(stored(&cfg, "default.ctb")).unwrap(),
        ctb(true)
    );
    assert!(stored(&cfg, "default.pc3").is_file());
    // And a second `set` of the same kind replaces its own file only.
    let c = put(&dir, "c.ctb", &ctb(false));
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "set", "--ctb", s(&c)])),
        0
    );
    assert_eq!(
        std::fs::read(stored(&cfg, "default.ctb")).unwrap(),
        ctb(false)
    );
    assert_eq!(std::fs::read(stored(&cfg, "default.pc3")).unwrap(), device);
    let _ = std::fs::remove_dir_all(dir);
}

/// A file no plot could use is refused with the core's code, and nothing is stored, not even the good
/// half of a pair.
#[cfg(feature = "pvec-convert")]
#[test]
fn set_refuses_what_a_plot_could_not_use_and_stores_nothing() {
    let dir = scratch("set-bad");
    let cfg = dir.join("cfg");
    let good = put(&dir, "good.ctb", &ctb(true));
    let junk = put(&dir, "junk.ctb", b"this is not a plot style table");
    let big = put(&dir, "big.pc3", &vec![b'x'; 1024 * 1024 + 1]);
    let no_device = put(
        &dir,
        "empty.pc3",
        b"PIAFILEVERSION_3.0,json\n{\n\"data\" : {\n}\n}\n",
    );
    let cases: [(&[&str], &str); 4] = [
        (&["--ctb", s(&junk)], "plot-file-unrecognised"),
        (&["--pc3", s(&big)], "plot-file-too-large"),
        (&["--pc3", s(&no_device)], "plot-default-no-device"),
        (
            &["--ctb", s(&good), "--pc3", s(&big)],
            "plot-file-too-large",
        ),
    ];
    for (flags, why) in cases {
        let mut args = vec!["plot-defaults", "set"];
        args.extend_from_slice(flags);
        let out = run(&cfg, &args);
        assert_eq!(code(&out), 1, "{flags:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(why), "{flags:?}: {}", stderr(&out));
        assert!(!cfg.join("plot").exists(), "{flags:?} stored something");
    }
    let missing = dir.join("missing.ctb");
    let out = run(&cfg, &["plot-defaults", "set", "--ctb", s(&missing)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(!cfg.join("plot").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// Where the directory is: `LIBVIPRS_CONFIG_DIR` when absolute, else XDG or HOME (the OS's own place), and a
/// relative value is ignored. With none of them usable `set` exits 1 and names the variable that fixes it.
#[cfg(all(unix, feature = "pvec-convert"))]
#[test]
fn the_config_directory_follows_the_documented_order_and_ignores_relative_values() {
    let dir = scratch("dirs");
    let file = put(&dir, "a.ctb", &ctb(true));
    let set = |env: &[(&str, &str)]| go(&["plot-defaults", "set", "--ctb", s(&file)], env);
    let xdg = dir.join("xdg");
    let home = dir.join("home");
    let explicit = dir.join("explicit");
    std::fs::create_dir_all(&home).unwrap();
    let macos = std::env::consts::OS == "macos";

    // An absolute LIBVIPRS_CONFIG_DIR is the directory itself, and beats XDG and HOME.
    let out = set(&[
        ("LIBVIPRS_CONFIG_DIR", s(&explicit)),
        ("XDG_CONFIG_HOME", s(&xdg)),
        ("HOME", s(&home)),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(explicit.join("plot/default.ctb").is_file());
    assert!(!xdg.exists());

    // A relative one is ignored, so the next level answers.
    let out = set(&[
        ("LIBVIPRS_CONFIG_DIR", "relative/dir"),
        ("XDG_CONFIG_HOME", s(&xdg)),
        ("HOME", s(&home)),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    if macos {
        let under = home.join("Library/Application Support/libviprs/plot/default.ctb");
        assert!(under.is_file());
    } else {
        assert!(xdg.join("libviprs/plot/default.ctb").is_file());
    }
    assert!(!Path::new("relative/dir").exists());

    // HOME alone: ~/.config/libviprs (anything but macOS) or ~/Library/Application Support/libviprs.
    let home2 = dir.join("home2");
    std::fs::create_dir_all(&home2).unwrap();
    let out = set(&[("HOME", s(&home2)), ("XDG_CONFIG_HOME", "also/relative")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let under_home = if macos {
        "Library/Application Support/libviprs"
    } else {
        ".config/libviprs"
    };
    assert!(home2.join(under_home).join("plot/default.ctb").is_file());

    // Nothing absolute anywhere.
    let out = set(&[
        ("LIBVIPRS_CONFIG_DIR", "relative"),
        ("HOME", "relative-home"),
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("LIBVIPRS_CONFIG_DIR"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- show: what a plot would use, and where each file comes from -------------------------------------

/// With nothing set both files are the built-in ones, and the config directory is named.
#[cfg(feature = "pvec-convert")]
#[test]
fn show_with_nothing_set_says_built_in_for_both() {
    let dir = scratch("show-none");
    let cfg = dir.join("cfg");
    let out = run(&cfg, &["plot-defaults", "show"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(&format!("config directory: {}", cfg.display())),
        "{text}"
    );
    assert!(
        text.contains("plot style table: built-in monochrome"),
        "{text}"
    );
    assert!(text.contains("plotter config: built-in PDF"), "{text}");
    assert!(!cfg.exists(), "show creates nothing");
    let _ = std::fs::remove_dir_all(dir);
}

/// `show --json` is one object: the directory, and per file the level and the path.
#[cfg(feature = "pvec-convert")]
#[test]
fn show_json_names_the_level_and_path_of_each_file() {
    let dir = scratch("show-json");
    let cfg = dir.join("cfg");
    let mine = put(&dir, "mine.ctb", &ctb(true));
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "set", "--ctb", s(&mine)])),
        0
    );
    let out = run(&cfg, &["plot-defaults", "show", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    assert_eq!(v["config_dir"], s(&cfg));
    assert_eq!(v["ctb"]["source"], "stored");
    assert_eq!(v["ctb"]["path"], s(&stored(&cfg, "default.ctb")));
    assert_eq!(v["pc3"]["source"], "built-in");
    assert!(v["pc3"]["path"].is_null(), "{v}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The walk, per file and independently: the flag, then the variable, then the stored default, then the
/// built-in. `--no-defaults` drops the variable and the stored one and never the flag, and `--ctb none` is
/// its own answer for the table.
#[cfg(feature = "pvec-convert")]
#[test]
fn show_walks_flag_then_env_then_stored_then_built_in_per_file() {
    let dir = scratch("walk");
    let cfg = dir.join("cfg");
    let cfg_s = s(&cfg);
    let flag_ctb = put(&dir, "flag.ctb", &ctb(false));
    let env_ctb = put(&dir, "env.ctb", &ctb(true));
    let store_ctb = put(&dir, "store.ctb", &ctb(true));
    let flag_pc3 = put(
        &dir,
        "flag.pc3",
        &pc3([200.0, 100.0], [5.0, 10.0, 195.0, 90.0]),
    );
    let env_pc3 = put(
        &dir,
        "env.pc3",
        &pc3([210.0, 297.0], [5.0, 5.0, 205.0, 292.0]),
    );
    let store_pc3 = put(
        &dir,
        "store.pc3",
        &pc3([420.0, 297.0], [5.0, 5.0, 415.0, 292.0]),
    );
    let source = |env: &[(&str, &str)], args: &[&str]| -> serde_json::Value {
        let mut all = vec!["plot-defaults", "show", "--json"];
        all.extend_from_slice(args);
        let mut e = vec![("LIBVIPRS_CONFIG_DIR", cfg_s)];
        e.extend_from_slice(env);
        let out = go(&all, &e);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let pair = |v: &serde_json::Value| {
        (
            v["ctb"]["source"].as_str().unwrap().to_owned(),
            v["pc3"]["source"].as_str().unwrap().to_owned(),
        )
    };
    let want = |a: &str, b: &str| (a.to_owned(), b.to_owned());

    assert_eq!(pair(&source(&[], &[])), want("built-in", "built-in"));

    let stored_both = [
        "plot-defaults",
        "set",
        "--ctb",
        s(&store_ctb),
        "--pc3",
        s(&store_pc3),
    ];
    assert_eq!(code(&run(&cfg, &stored_both)), 0);
    assert_eq!(pair(&source(&[], &[])), want("stored", "stored"));

    let env = [("LIBVIPRS_PLOT_CTB", s(&env_ctb))];
    let v = source(&env, &[]);
    assert_eq!(
        pair(&v),
        want("env", "stored"),
        "each file walks on its own"
    );
    assert_eq!(v["ctb"]["path"], s(&env_ctb));

    let env = [
        ("LIBVIPRS_PLOT_CTB", s(&env_ctb)),
        ("LIBVIPRS_PLOT_PC3", s(&env_pc3)),
    ];
    assert_eq!(pair(&source(&env, &[])), want("env", "env"));

    let v = source(&env, &["--ctb", s(&flag_ctb)]);
    assert_eq!(pair(&v), want("flag", "env"));
    assert_eq!(v["ctb"]["path"], s(&flag_ctb));
    let v = source(&env, &["--pc3", s(&flag_pc3)]);
    assert_eq!(pair(&v), want("env", "flag"));
    assert_eq!(v["pc3"]["path"], s(&flag_pc3));
    assert_eq!(
        pair(&source(
            &env,
            &["--ctb", s(&flag_ctb), "--pc3", s(&flag_pc3)]
        )),
        want("flag", "flag")
    );

    // --no-defaults: not the variable, not the stored pair, and still the flag.
    assert_eq!(
        pair(&source(&env, &["--no-defaults"])),
        want("built-in", "built-in")
    );
    assert_eq!(
        pair(&source(&env, &["--no-defaults", "--pc3", s(&flag_pc3)])),
        want("built-in", "flag")
    );

    // --ctb none: the objects' own colours, whatever else is set, and the PC3 walks on.
    let v = source(&env, &["--ctb", "none"]);
    assert_eq!(pair(&v), want("none", "env"));
    assert!(v["ctb"]["path"].is_null(), "{v}");
    assert_eq!(
        pair(&source(&[], &["--ctb", "none", "--no-defaults"])),
        want("none", "built-in")
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A variable that names a file that is not there is an error that names the variable, not a quiet fall
/// back to the next level. An empty one is not set.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_variable_naming_a_missing_file_exits_1_and_names_it() {
    let dir = scratch("env-missing");
    let cfg = dir.join("cfg");
    let missing = dir.join("missing.ctb");
    let out = go(
        &["plot-defaults", "show"],
        &[
            ("LIBVIPRS_CONFIG_DIR", s(&cfg)),
            ("LIBVIPRS_PLOT_CTB", s(&missing)),
        ],
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("LIBVIPRS_PLOT_CTB"),
        "{}",
        stderr(&out)
    );
    let out = go(
        &["plot-defaults", "show"],
        &[("LIBVIPRS_CONFIG_DIR", s(&cfg)), ("LIBVIPRS_PLOT_CTB", "")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("plot style table: built-in"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A stored default that no longer loads is an error with the core's code, never a quiet fall back to the
/// built-in; `--no-defaults` is the way past it.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_stored_default_that_no_longer_loads_is_an_error_and_no_defaults_skips_it() {
    let dir = scratch("corrupt");
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("plot")).unwrap();
    std::fs::write(stored(&cfg, "default.ctb"), b"hand corrupted").unwrap();
    let out = run(&cfg, &["plot-defaults", "show"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-default-unreadable"),
        "{}",
        stderr(&out)
    );
    let out = run(&cfg, &["plot-defaults", "show", "--no-defaults"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("plot style table: built-in"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- clear -----------------------------------------------------------------------------------------

/// `clear` removes both stored files, or only the one named, and says what it removed.
#[cfg(feature = "pvec-convert")]
#[test]
fn clear_removes_both_files_or_the_one_named() {
    let dir = scratch("clear");
    let cfg = dir.join("cfg");
    let a = put(&dir, "a.ctb", &ctb(true));
    let b = put(
        &dir,
        "b.pc3",
        &pc3([200.0, 100.0], [5.0, 10.0, 195.0, 90.0]),
    );
    let store = || {
        let out = run(
            &cfg,
            &["plot-defaults", "set", "--ctb", s(&a), "--pc3", s(&b)],
        );
        assert_eq!(code(&out), 0, "{}", stderr(&out));
    };
    store();
    let out = run(&cfg, &["plot-defaults", "clear", "--ctb"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!stored(&cfg, "default.ctb").exists());
    assert!(stored(&cfg, "default.pc3").is_file());
    assert_eq!(code(&run(&cfg, &["plot-defaults", "clear", "--pc3"])), 0);
    assert!(!stored(&cfg, "default.pc3").exists());
    store();
    let out = run(&cfg, &["plot-defaults", "clear"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let said = stderr(&out);
    assert!(
        said.contains("default.ctb") && said.contains("default.pc3"),
        "{said}"
    );
    assert!(!stored(&cfg, "default.ctb").exists() && !stored(&cfg, "default.pc3").exists());
    // The files the user gave are theirs and stay.
    assert!(a.is_file() && b.is_file());
    let _ = std::fs::remove_dir_all(dir);
}

/// Clearing what is not there is fine, and creates nothing.
#[cfg(feature = "pvec-convert")]
#[test]
fn clear_with_nothing_stored_is_fine_and_creates_nothing() {
    let dir = scratch("clear-none");
    let cfg = dir.join("cfg");
    let out = run(&cfg, &["plot-defaults", "clear"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stderr(&out).contains("nothing stored"), "{}", stderr(&out));
    assert!(!cfg.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// A `plot` directory that is a symlink is not gone through, in either direction: `clear` and `set` both
/// refuse and what it points at is untouched.
#[cfg(all(unix, feature = "pvec-convert"))]
#[test]
fn a_symlinked_plot_directory_is_refused() {
    let dir = scratch("plot-link");
    let cfg = dir.join("cfg");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let kept = put(&elsewhere, "default.ctb", b"not yours");
    std::fs::create_dir_all(&cfg).unwrap();
    std::os::unix::fs::symlink(&elsewhere, cfg.join("plot")).unwrap();
    let out = run(&cfg, &["plot-defaults", "clear"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-default-unsafe"),
        "{}",
        stderr(&out)
    );
    let mine = put(&dir, "a.ctb", &ctb(true));
    let out = run(&cfg, &["plot-defaults", "set", "--ctb", s(&mine)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(std::fs::read(&kept).unwrap(), b"not yours");
    assert_eq!(listing(&elsewhere), ["default.ctb"]);
    let _ = std::fs::remove_dir_all(dir);
}

// ---- plot: the walk, end to end -------------------------------------------------------------------

/// With no file given a plot is monochrome with the built-in style, says so, and creates nothing in the
/// config directory.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_plot_with_no_files_uses_the_built_in_style_and_device_and_says_so() {
    let dir = scratch("plot-builtin");
    let cfg = dir.join("cfg");
    let pdf = dir.join("out.pdf");
    let out = run(&cfg, &["plot", s(&sample()), s(&pdf)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let said = stderr(&out);
    assert!(
        said.contains("plot style table: built-in monochrome"),
        "{said}"
    );
    assert!(said.contains("plotter config: built-in PDF"), "{said}");
    assert_eq!(pages(&pdf).len(), 3);
    assert_eq!(reds(&pdf), 0);
    assert!(!cfg.exists(), "a plot stores nothing it was not asked to");
    let _ = std::fs::remove_dir_all(dir);
}

/// `--pc3` alone is a plot now: the table is the built-in one and the PC3 still fills only what a
/// layout leaves empty.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_pc3_alone_plots_with_the_built_in_table() {
    let dir = scratch("pc3-alone");
    let cfg = dir.join("cfg");
    let pc3_path = put(
        &dir,
        "device.pc3",
        &pc3([200.0, 100.0], [5.0, 10.0, 195.0, 90.0]),
    );
    let alone = dir.join("alone.pdf");
    let plain = dir.join("plain.pdf");
    let out = run(
        &cfg,
        &["plot", s(&sample()), s(&alone), "--pc3", s(&pc3_path)],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let said = stderr(&out);
    assert!(
        said.contains("plot style table: built-in monochrome"),
        "{said}"
    );
    assert!(
        said.contains(&format!("plotter config: --pc3 {}", pc3_path.display())),
        "{said}"
    );
    assert_eq!(code(&run(&cfg, &["plot", s(&sample()), s(&plain)])), 0);
    assert_eq!(
        pages(&alone),
        pages(&plain),
        "a layout's own page setup still wins"
    );
    assert_eq!(colour_set(&alone), colour_set(&plain));
    let _ = std::fs::remove_dir_all(dir);
}

/// The walk on a real plot, the table's half: red from the flag, the variable or the stored default
/// shows on the page, and the nearer level wins over the farther.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_plot_walks_flag_then_env_then_stored_then_built_in() {
    let dir = scratch("plot-walk");
    let cfg = dir.join("cfg");
    let red = put(&dir, "red.ctb", &ctb(true));
    let mono = put(&dir, "mono.ctb", &ctb(false));
    let pdf = dir.join("out.pdf");
    let drawing = sample();
    let plot = |env: &[(&str, &str)], extra: &[&str]| -> (String, usize) {
        let _ = std::fs::remove_file(&pdf);
        let mut args = vec!["plot", s(&drawing), s(&pdf)];
        args.extend_from_slice(extra);
        let mut e = vec![("LIBVIPRS_CONFIG_DIR", s(&cfg))];
        e.extend_from_slice(env);
        let out = go(&args, &e);
        assert_eq!(code(&out), 0, "{extra:?}: {}", stderr(&out));
        (stderr(&out), reds(&pdf))
    };
    assert_eq!(plot(&[], &[]).1, 0, "built-in: black");
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "set", "--ctb", s(&red)])),
        0
    );
    let (said, n) = plot(&[], &[]);
    assert!(n > 0, "the stored red table is not applied");
    assert!(said.contains("plot style table: stored default"), "{said}");
    let (said, n) = plot(&[("LIBVIPRS_PLOT_CTB", s(&mono))], &[]);
    assert_eq!(n, 0, "the variable beats the stored default");
    assert!(
        said.contains("plot style table: LIBVIPRS_PLOT_CTB"),
        "{said}"
    );
    let (said, n) = plot(&[("LIBVIPRS_PLOT_CTB", s(&mono))], &["--ctb", s(&red)]);
    assert!(n > 0, "the flag beats the variable");
    assert!(
        said.contains(&format!("plot style table: --ctb {}", red.display())),
        "{said}"
    );
    let (said, n) = plot(&[("LIBVIPRS_PLOT_CTB", s(&red))], &["--no-defaults"]);
    assert_eq!(n, 0, "--no-defaults drops the variable");
    assert!(
        said.contains("plot style table: built-in monochrome"),
        "{said}"
    );
    let (_, n) = plot(&[], &["--no-defaults"]);
    assert_eq!(n, 0, "--no-defaults drops the stored default");
    let (_, n) = plot(&[], &["--no-defaults", "--ctb", s(&red)]);
    assert!(n > 0, "--no-defaults never drops a flag");
    let _ = std::fs::remove_dir_all(dir);
}

/// `--ctb none` draws the objects' own colours and widths, which is what the identity table gives, and
/// beats a stored default; the generated `.ctb` and `.pc3` passed by hand match the built-in.
#[cfg(feature = "pvec-convert")]
#[test]
fn ctb_none_gives_the_objects_own_colours_and_the_generated_files_match_the_built_in() {
    let dir = scratch("ctb-none");
    let cfg = dir.join("cfg");
    let red = put(&dir, "red.ctb", &ctb(true));
    let identity = put(&dir, "identity.ctb", &ctb(false));
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "set", "--ctb", s(&red)])),
        0
    );
    let none = dir.join("none.pdf");
    let by_identity = dir.join("identity.pdf");
    let out = run(&cfg, &["plot", s(&sample()), s(&none), "--ctb", "none"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot style table: none"),
        "{}",
        stderr(&out)
    );
    let out = run(
        &cfg,
        &["plot", s(&sample()), s(&by_identity), "--ctb", s(&identity)],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(colour_set(&none), colour_set(&by_identity));
    assert_eq!(
        reds(&none),
        0,
        "the stored red table is not applied under --ctb none"
    );

    let written = dir.join("written");
    assert_eq!(
        code(&run(&cfg, &["plot-defaults", "write", s(&written)])),
        0
    );
    let built_in = dir.join("built-in.pdf");
    let generated = dir.join("generated.pdf");
    let out = run(&cfg, &["plot", s(&sample()), s(&built_in), "--no-defaults"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = run(
        &cfg,
        &[
            "plot",
            s(&sample()),
            s(&generated),
            "--ctb",
            s(&written.join("libviprs-monochrome.ctb")),
            "--pc3",
            s(&written.join("libviprs-pdf.pc3")),
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(colour_set(&built_in), colour_set(&generated));
    assert_eq!(pages(&built_in), pages(&generated));
    let _ = std::fs::remove_dir_all(dir);
}

/// A stored default that does not load stops the plot before anything is written, with the code;
/// `--no-defaults` plots anyway. A variable naming a missing file does the same and names itself.
#[cfg(feature = "pvec-convert")]
#[test]
fn a_bad_default_stops_the_plot_before_anything_is_written() {
    let dir = scratch("plot-bad-default");
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("plot")).unwrap();
    std::fs::write(stored(&cfg, "default.pc3"), b"hand corrupted").unwrap();
    let pdf = dir.join("out.pdf");
    let out = run(&cfg, &["plot", s(&sample()), s(&pdf)]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("plot-default-unreadable"),
        "{}",
        stderr(&out)
    );
    assert!(!pdf.exists());
    let out = run(&cfg, &["plot", s(&sample()), s(&pdf), "--no-defaults"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let _ = std::fs::remove_file(&pdf);
    let gone = dir.join("gone.pc3");
    let out = go(
        &["plot", s(&sample()), s(&pdf)],
        &[
            ("LIBVIPRS_CONFIG_DIR", s(&cfg)),
            ("LIBVIPRS_PLOT_PC3", s(&gone)),
        ],
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("LIBVIPRS_PLOT_PC3"),
        "{}",
        stderr(&out)
    );
    assert!(!pdf.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// `--ctb none` is a table choice and needs no `--pc3`, and the two do not conflict.
#[cfg(feature = "pvec-convert")]
#[test]
fn ctb_none_and_pc3_go_together() {
    let dir = scratch("none-pc3");
    let pc3_path = put(
        &dir,
        "device.pc3",
        &pc3([200.0, 100.0], [5.0, 10.0, 195.0, 90.0]),
    );
    let pdf = dir.join("out.pdf");
    let out = run(
        &dir.join("cfg"),
        &[
            "plot",
            s(&sample()),
            s(&pdf),
            "--ctb",
            "none",
            "--pc3",
            s(&pc3_path),
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(pdf.is_file());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- builds and docs --------------------------------------------------------------------------------

/// A build without `pvec-convert` has no defaults to keep: `show` exits 1 and names the feature, and a usage
/// mistake is still exit 2.
#[cfg(not(feature = "pvec-convert"))]
#[test]
fn a_build_without_pvec_convert_exits_1_and_names_it() {
    let dir = scratch("nofeature");
    let out = run(&dir.join("cfg"), &["plot-defaults", "show"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("pvec-convert"), "{}", stderr(&out));
    let out = run(&dir.join("cfg"), &["plot-defaults", "set"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

/// README, COVERAGE.md and CHANGELOG say the command exists, and COVERAGE.md says the core's defaults API
/// is covered by it.
#[test]
fn readme_coverage_and_changelog_name_plot_defaults() {
    let read =
        |f: &str| std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(f)).unwrap();
    let readme = read("README.md");
    for needle in [
        "plot-defaults",
        "LIBVIPRS_CONFIG_DIR",
        "LIBVIPRS_PLOT_CTB",
        "LIBVIPRS_PLOT_PC3",
        "--no-defaults",
        "--ctb none",
    ] {
        assert!(
            readme.contains(needle),
            "README.md does not mention {needle}"
        );
    }
    assert!(
        !readme.contains("--pc3` without `--ctb` is a usage error"),
        "README.md still says --pc3 without --ctb is a usage error"
    );
    let coverage = read("COVERAGE.md");
    let row = coverage
        .lines()
        .find(|l| l.starts_with("| `cad::plot_defaults`"))
        .expect("a COVERAGE.md row for cad::plot_defaults");
    assert!(
        row.contains("viprs plot-defaults") && row.contains("covered"),
        "{row}"
    );
    assert!(!row.contains("missing"), "{row}");
    assert!(
        read("CHANGELOG.md").contains("plot-defaults"),
        "CHANGELOG.md does not mention plot-defaults"
    );
}
