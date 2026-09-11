//! Guards on the `@doc-*` annotations libviprs.org's snippet extractor reads
//! out of `src/main.rs`.
//!
//! The site's `extract-snippets` gate requires the set of `@doc-flag` ids in
//! its frozen copy of this file to *equal* the flag ids in its hand-authored
//! `pyramid.command.json` embed, and `anchors.js` requires every
//! `https://libviprs.org/cli/#flag-<name>` link in a doc comment to resolve to
//! a flag the extractor emitted. Both of those fail in the *other* repository,
//! with a message that points at a file nobody editing this crate is looking
//! at, so the cheap half of each check lives here where the edit happens.
//!
//! These read the source rather than the binary on purpose: the annotations are
//! comments, so nothing else in this crate's build can notice when one goes
//! missing.

use std::collections::BTreeSet;

/// The `src/main.rs` this crate is built from.
fn main_rs() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("src/main.rs must be readable at {}: {e}", path.display()))
}

/// Every `<prefix><name>` occurrence, with `name` in `[a-z0-9-]`.
fn ids_after(source: &str, prefix: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = source;
    while let Some(at) = rest.find(prefix) {
        rest = &rest[at + prefix.len()..];
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

#[test]
fn every_flag_anchor_links_to_an_annotated_flag() {
    let src = main_rs();
    let linked = ids_after(&src, "#flag-");
    let annotated = ids_after(&src, "@doc-flag:");

    assert!(
        !linked.is_empty(),
        "the parse found no #flag- anchors at all, which means it is broken \
         rather than that the file is clean"
    );
    assert!(
        !annotated.is_empty(),
        "the parse found no @doc-flag annotations at all"
    );

    let orphans: Vec<&String> = linked.difference(&annotated).collect();
    assert!(
        orphans.is_empty(),
        "these doc comments link to https://libviprs.org/cli/#flag-<name> anchors \
         that no @doc-flag annotation produces, so libviprs.org's anchors gate \
         will fail on them: {orphans:?}"
    );
}

#[test]
fn the_storage_flag_is_annotated() {
    // `--storage` is the flag the PMTiles default flip added, and the site's
    // extract gate compares the *set* of ids in both directions: an annotation
    // with no embed entry and an embed entry with no annotation both fail. This
    // is the half of that contract this repository owns.
    let annotated = ids_after(&main_rs(), "@doc-flag:");
    assert!(
        annotated.contains("storage"),
        "@doc-flag: storage must exist for libviprs-org's pyramid.command.json \
         entry to have a counterpart, got {annotated:?}"
    );
}

#[test]
fn every_snippet_slot_is_opened_and_closed() {
    let src = main_rs();
    let opened = ids_after(&src, "@doc-snippet:begin slot=");
    let closed = ids_after(&src, "@doc-snippet:end slot=");

    assert!(
        !opened.is_empty(),
        "the parse found no snippet slots at all"
    );
    let unclosed: Vec<&String> = opened.difference(&closed).collect();
    assert!(
        unclosed.is_empty(),
        "these @doc-snippet slots open and never close: {unclosed:?}"
    );
    let unopened: Vec<&String> = closed.difference(&opened).collect();
    assert!(
        unopened.is_empty(),
        "these @doc-snippet slots close without opening: {unopened:?}"
    );
}
