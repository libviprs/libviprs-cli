//! `pvec-raster` is a default feature (libviprs-cli#134): PVec is public, so the `.pvec` raster
//! commands ship in a plain `cargo install` and in the released tarballs.

use std::process::{Command, Output};

fn read(rel: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs")).args(args).output().unwrap()
}

/// The names inside `default = [...]` in Cargo.toml's `[features]` table.
fn default_features() -> Vec<String> {
    let manifest = read("Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.trim_start().starts_with("default = ["))
        .expect("Cargo.toml has a `default = [...]` line");
    line.split(['[', ']'])
        .nth(1)
        .unwrap()
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn the_manifest_lists_pvec_raster_as_default() {
    let d = default_features();
    assert!(d.iter().any(|f| f == "pvec-raster"), "default = {d:?}");
    assert!(d.iter().any(|f| f == "pdfium"), "pdfium stays default: {d:?}");
}

/// A default build (pdfium on means the default set) has the commands, not the refusal.
#[cfg(feature = "pdfium")]
#[test]
fn a_default_build_reports_the_feature_and_has_the_commands() {
    let out = run(&["features"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.lines().any(|l| l == "pvec-raster"), "features:\n{stdout}");
    for args in [["pmtiles", "to-pvec", "--help"], ["pvec", "rasters", "--help"]] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn the_release_smoke_test_requires_the_feature() {
    let wf = read(".github/workflows/release.yml");
    assert!(
        wf.lines().any(|l| l.trim() == "grep -qx pvec-raster features.txt"),
        "the smoke test must fail a tarball whose binary lacks pvec-raster"
    );
    assert!(!wf.contains("--features") && !wf.contains("--no-default-features"),
        "the release builds the default set, which is what ships");
}

#[test]
fn nothing_still_says_the_commands_are_off_by_default() {
    for (file, stale) in [
        ("README.md", "Only `pdfium` is on by default"),
        ("README.md", "All three commands need a build with the `pvec-raster` feature"),
        ("Cargo.toml", "private"),
        ("Cargo.toml", "release workflow does not build them"),
        ("CHANGELOG.md", "off by default"),
        ("RELEASING.md", "Default means `pdfium`"),
    ] {
        assert!(!read(file).contains(stale), "{file} still says: {stale}");
    }
    let readme = read("README.md");
    assert!(readme.contains("| `pvec-raster` (default) |"), "the feature table marks it default");
}
