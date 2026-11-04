//! acadrust is never resolved from a registry (libviprs-cli#142, the CLI half of libviprs#1486).
//!
//! The reader comes from libviprs's own fork, pinned by revision through the core. The core
//! commits no `Cargo.lock`, so only this crate can hold the real resolution: every `acadrust`
//! package in the committed lockfile must be the fork at a full revision.

const FORK: &str = "git+https://github.com/libviprs/acadrust?rev=";

/// What is wrong with the `acadrust` packages in a lockfile, one line each. Empty when sound.
fn problems(lock: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in lock.split("[[package]]") {
        if !block.lines().any(|l| l.trim() == "name = \"acadrust\"") {
            continue;
        }
        let source = block
            .lines()
            .find_map(|l| l.trim().strip_prefix("source = \""))
            .and_then(|s| s.strip_suffix('"'));
        match source {
            None => out.push("acadrust has no source".to_owned()),
            Some(s) => match s.strip_prefix(FORK) {
                Some(rest) => {
                    let rev = rest.split('#').next().unwrap_or("");
                    if rev.len() != 40 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
                        out.push(format!("acadrust is not pinned by a full revision: {s}"));
                    }
                }
                None => out.push(format!("acadrust is not the libviprs fork: {s}")),
            },
        }
    }
    out
}

#[test]
fn the_committed_lockfile_has_acadrust_from_the_fork_by_revision() {
    let lock = include_str!("../Cargo.lock");
    assert!(
        lock.contains("name = \"acadrust\""),
        "the lockfile has no acadrust; the default reader is gone"
    );
    assert_eq!(problems(lock), Vec::<String>::new());
}

#[test]
fn a_registry_copy_is_caught() {
    let lock = "[[package]]\nname = \"acadrust\"\nversion = \"0.5.5\"\n\
source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    assert_eq!(problems(lock).len(), 1);
}

#[test]
fn a_branch_a_short_rev_or_no_source_is_caught() {
    let branch = "[[package]]\nname = \"acadrust\"\nsource = \"git+https://github.com/libviprs/acadrust?branch=main#abc\"\n";
    let short = "[[package]]\nname = \"acadrust\"\nsource = \"git+https://github.com/libviprs/acadrust?rev=f1e00c11#f1e00c11\"\n";
    let none = "[[package]]\nname = \"acadrust\"\nversion = \"0.6.8\"\n";
    let other = "[[package]]\nname = \"acadrust\"\nsource = \"git+https://github.com/hakanaktt/acadrust?rev=f1e00c11f6371c8cfcff2c726620a51a45a2ad1b#f1e00c11\"\n";
    for lock in [branch, short, none, other] {
        assert_eq!(problems(lock).len(), 1, "{lock}");
    }
}

#[test]
fn a_second_registry_copy_beside_the_fork_is_caught() {
    let lock = "[[package]]\nname = \"acadrust\"\nsource = \"git+https://github.com/libviprs/acadrust?rev=f1e00c11f6371c8cfcff2c726620a51a45a2ad1b#f1e00c11\"\n\n\
[[package]]\nname = \"acadrust\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    assert_eq!(problems(lock).len(), 1);
}
