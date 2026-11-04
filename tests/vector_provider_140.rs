//! `viprs vector --provider` (libviprs-cli#140): acadrust is the default DWG reader and
//! ACadSharp is the experimental one you ask for.
//!
//! The cells that read a drawing use the committed `sample_AC1018.dwg` and need no native
//! library, because the default reader is pure Rust.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("the viprs binary must be spawnable")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("a normal exit")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn sample() -> PathBuf {
    root("tests/fixtures/vector/sample_AC1018.dwg")
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The lines of a `[features]` entry in Cargo.toml, `name = [ ... ]`, as one string.
fn feature_entry(name: &str) -> String {
    let manifest = read("Cargo.toml");
    let start = manifest
        .find(&format!("\n{name} = ["))
        .unwrap_or_else(|| panic!("Cargo.toml has no `{name}` feature"));
    let rest = &manifest[start + 1..];
    rest[..rest.find(']').unwrap() + 1].to_owned()
}

#[test]
fn every_drawing_command_documents_the_provider_flag() {
    for command in ["views", "build", "report"] {
        let out = run(&["vector", command, "--help"]);
        assert_eq!(code(&out), 0, "{command}: {}", stderr(&out));
        let help = stdout(&out);
        assert!(help.contains("--provider"), "{command}:\n{help}");
        assert!(help.contains("acadrust"), "{command}:\n{help}");
        assert!(help.contains("acadsharp"), "{command}:\n{help}");
        assert!(help.contains("experimental"), "{command}:\n{help}");
    }
}

#[test]
fn an_unknown_provider_is_a_usage_error_that_writes_nothing() {
    let out = run(&["vector", "views", s(&sample()), "--provider", "nope"]);
    assert_eq!(code(&out), 2, "stderr:\n{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("acadrust") && err.contains("acadsharp"),
        "{err}"
    );
}

#[test]
fn acadrust_is_the_default_provider() {
    let out = run(&["vector", "views", s(&sample()), "--json"]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["views"][0]["kind"], "model");
}

#[test]
fn asking_for_acadrust_by_name_reads_the_same_drawing() {
    let plain = run(&["vector", "views", s(&sample()), "--json"]);
    let named = run(&[
        "vector",
        "views",
        s(&sample()),
        "--json",
        "--provider",
        "acadrust",
    ]);
    assert_eq!(code(&named), 0, "stderr:\n{}", stderr(&named));
    assert_eq!(stdout(&plain), stdout(&named));
}

#[test]
fn a_default_build_writes_an_archive_that_validates() {
    let dir = std::env::temp_dir().join(format!("viprs-provider-140-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let archive = dir.join("sample.pmtiles");
    let out = run(&[
        "vector",
        "build",
        s(&sample()),
        s(&archive),
        "--max-zoom",
        "2",
    ]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let out = run(&["vector", "validate", s(&archive)]);
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn features_lists_the_acadrust_reader() {
    let out = run(&["features"]);
    assert!(
        stdout(&out).lines().any(|l| l == "dwg-acadrust"),
        "{}",
        stdout(&out)
    );
}

/// A build that left ACadSharp out says which feature brings it back, and that it is the
/// experimental one, instead of silently reading the drawing with the other reader.
#[cfg(not(feature = "dwg-acadsharp"))]
#[test]
fn asking_for_acadsharp_without_it_names_the_feature_and_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("viprs-provider-140-s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let archive = dir.join("out.pmtiles");
    for args in [
        vec!["vector", "views", s(&sample()), "--provider", "acadsharp"],
        vec![
            "vector",
            "build",
            s(&sample()),
            s(&archive),
            "--provider",
            "acadsharp",
        ],
        vec!["vector", "report", s(&sample()), "--provider", "acadsharp"],
    ] {
        let out = run(&args);
        assert_eq!(code(&out), 1, "{args:?}\nstderr:\n{}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains("--features dwg-acadsharp"), "{args:?}\n{err}");
        assert!(err.contains("experimental"), "{args:?}\n{err}");
    }
    assert!(!archive.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_vector_command_help_calls_acadsharp_experimental() {
    let out = run(&["vector", "--help"]);
    assert_eq!(code(&out), 0);
    let help = stdout(&out);
    assert!(help.contains("acadrust"), "{help}");
    assert!(help.contains("experimental"), "{help}");
}

#[test]
fn the_manifest_makes_acadrust_the_default_and_keeps_acadsharp_in_full() {
    assert!(feature_entry("default").contains("\"dwg-acadrust\""));
    let full = feature_entry("full");
    assert!(full.contains("\"dwg-acadrust\""), "{full}");
    assert!(full.contains("\"dwg-acadsharp\""), "{full}");
    let manifest = read("Cargo.toml");
    let comment_end = manifest.find("\ndwg-acadsharp = [").unwrap();
    let comment = &manifest[manifest[..comment_end].rfind("\n\n").unwrap_or(0)..comment_end];
    assert!(
        comment.to_ascii_lowercase().contains("experimental"),
        "{comment}"
    );
}

#[test]
fn the_core_pin_is_past_the_acadrust_decoder() {
    let manifest = read("Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("libviprs = {"))
        .expect("the libviprs dependency line");
    assert!(
        !line.contains("09cca562"),
        "still on the old core pin: {line}"
    );
    assert!(
        line.contains("git = \"https://github.com/libviprs/libviprs\""),
        "{line}"
    );
    assert!(line.contains("rev = \""), "pinned by rev: {line}");
    assert!(
        line.contains("default-features = false"),
        "the CLI's own features decide the DWG reader: {line}"
    );
}

#[test]
fn the_readme_documents_the_flag_and_no_longer_says_acadsharp_only() {
    let readme = read("README.md");
    assert!(
        readme.contains("--provider"),
        "the README never mentions --provider"
    );
    assert!(readme.contains("dwg-acadrust"));
    assert!(readme.to_ascii_lowercase().contains("experimental"));
    assert!(
        !readme.contains("`vector` reads a drawing through ACadSharp only"),
        "the stale paragraph is still there"
    );
}
