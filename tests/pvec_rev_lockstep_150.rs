//! libviprs-cli#150: the pvec revision this crate names is the one the pinned core names.
//!
//! `pvec-facade`, `pvec-capabilities`, `pvec-container` and `pvec-components` here have to resolve to
//! the same PVec commit as the core's own, or the core's `PvecWriter` and the one the CLI reads back
//! with are two different types and a conversion is checked by a reader that is not the writer's.
//! Moving the core pin moves that revision with it, and nothing but this file says so when it is
//! forgotten: cargo happily resolves two PVec commits side by side.
//!
//! The core's manifest is read from cargo's own checkout of the pinned revision, not from
//! `../libviprs` (which the gate clones at main, and main moves on), so the comparison is always
//! against the core this crate is built with.

use std::path::{Path, PathBuf};

const PVEC_REPO: &str = "https://github.com/libviprs/pvec";

/// Every `rev` a dependency on the PVec repository names in a manifest, as (dependency, rev).
fn pvec_revs(manifest: &str) -> Vec<(String, String)> {
    manifest
        .lines()
        .filter(|l| l.contains(&format!("git = \"{PVEC_REPO}\"")))
        .filter_map(|l| {
            let name = l.split('=').next()?.trim().to_owned();
            let rev = l.split("rev = \"").nth(1)?.split('"').next()?.to_owned();
            Some((name, rev))
        })
        .collect()
}

/// The rev of the `libviprs` dependency, as this crate's manifest names it.
fn core_rev(manifest: &str) -> String {
    let line = manifest
        .lines()
        .find(|l| l.starts_with("libviprs = {"))
        .expect("Cargo.toml has a libviprs dependency");
    line.split("rev = \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("the libviprs dependency is pinned by rev")
        .to_owned()
}

/// The pinned core's `Cargo.toml`, from cargo's checkout of that revision.
fn pinned_core_manifest(rev: &str) -> PathBuf {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo")))
        .expect("CARGO_HOME or HOME is set");
    let checkouts = home.join("git/checkouts");
    for repo in std::fs::read_dir(&checkouts)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", checkouts.display()))
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("libviprs-"))
    {
        for short in std::fs::read_dir(repo.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = short.file_name().to_string_lossy().into_owned();
            let manifest = short.path().join("Cargo.toml");
            let is_core = std::fs::read_to_string(&manifest)
                .is_ok_and(|t| t.contains("name = \"libviprs\"\n"));
            if name.len() >= 7 && rev.starts_with(&name) && is_core {
                return manifest;
            }
        }
    }
    panic!(
        "no cargo checkout of libviprs {rev} under {}",
        checkouts.display()
    );
}

#[test]
fn every_pvec_rev_here_is_the_one_the_pinned_core_names() {
    let mine = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("Cargo.toml is readable");
    let rev = core_rev(&mine);
    let core_path = pinned_core_manifest(&rev);
    let core = std::fs::read_to_string(&core_path).expect("the pinned core's manifest is readable");

    let theirs = pvec_revs(&core);
    let mut distinct: Vec<&str> = theirs.iter().map(|(_, r)| r.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        1,
        "the core at {rev} names more than one PVec rev: {theirs:?}"
    );

    let mine = pvec_revs(&mine);
    assert!(!mine.is_empty(), "this crate names no PVec dependency");
    for (name, have) in &mine {
        assert_eq!(
            have,
            distinct[0],
            "{name} is at {have} here, and the core at {rev} ({}) is at {}",
            core_path.display(),
            distinct[0]
        );
    }
}

#[test]
fn the_reader_finds_a_pvec_rev_and_ignores_other_git_dependencies() {
    let text = "\
pvec-geometry = { git = \"https://github.com/libviprs/pvec\", rev = \"abc123\", version = \"0.0.0\" }
acadrust = { git = \"https://github.com/libviprs/acadrust\", rev = \"def456\" }
pvec-facade = { package = \"pvec\", git = \"https://github.com/libviprs/pvec\", rev = \"abc123\" }
";
    assert_eq!(
        pvec_revs(text),
        [
            ("pvec-geometry".to_owned(), "abc123".to_owned()),
            ("pvec-facade".to_owned(), "abc123".to_owned()),
        ]
    );
    assert_eq!(
        core_rev(
            "libviprs = { git = \"https://github.com/libviprs/libviprs\", rev = \"4b44\", default-features = false }"
        ),
        "4b44"
    );
}
