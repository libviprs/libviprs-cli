//! libviprs-cli#154 (CV2.5): `pvec-interchange` is an optional direct dependency behind `pvec-export`,
//! which is in `full` and not in `default`. The PVec revision it names is held to the pinned core's
//! by `tests/pvec_rev_lockstep_150.rs`, which reads every PVec line of this manifest.

use std::path::Path;

fn manifest() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap()
}

/// The `[features]` entry `name`, as the quoted members of its array.
fn feature(text: &str, name: &str) -> Vec<String> {
    let start = text
        .lines()
        .position(|l| l.starts_with(&format!("{name} = [")))
        .unwrap_or_else(|| panic!("no feature {name}"));
    let mut body = String::new();
    for line in text.lines().skip(start) {
        body.push_str(line);
        if line.contains(']') {
            break;
        }
    }
    body.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn pvec_export_needs_the_facade_and_the_interchange_crate_and_nothing_else() {
    assert_eq!(
        feature(&manifest(), "pvec-export"),
        ["pvec-raster", "dep:pvec-interchange"]
    );
}

#[test]
fn pvec_export_is_in_full_and_not_in_default() {
    let text = manifest();
    assert!(feature(&text, "full").iter().any(|f| f == "pvec-export"));
    assert!(
        !feature(&text, "default")
            .iter()
            .any(|f| f.contains("export"))
    );
}

#[test]
fn the_interchange_crate_is_optional_and_comes_from_the_pvec_repository_by_rev() {
    let text = manifest();
    let line = text
        .lines()
        .find(|l| l.starts_with("pvec-interchange = {"))
        .expect("a pvec-interchange dependency");
    assert!(
        line.contains("git = \"https://github.com/libviprs/pvec\""),
        "{line}"
    );
    assert!(line.contains("rev = \""), "{line}");
    assert!(line.contains("optional = true"), "{line}");
}

#[test]
fn the_lock_holds_the_interchange_crate_once() {
    let lock =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).unwrap();
    let n = lock.matches("name = \"pvec-interchange\"").count();
    assert_eq!(n, 1, "two PVec revisions in the lock would be two crates");
}
