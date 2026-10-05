//! Guards on the `@doc-*` annotations libviprs.org's snippet extractor reads
//! out of `src/main.rs` and, since #66 moved the pyramid driver there,
//! `src/pipeline.rs`.
//!
//! The extractor (libviprs-org `cli/tools/extract-snippets`, fed by
//! `cli/tools/sync-cli-src.sh`) reads `main.rs` and `ops/**` today, so it has
//! to be taught `pipeline.rs` before its next re-sync of this crate, or its
//! flag-set cross-check comes up short by the flags that moved.
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

/// Every file carrying pyramid annotations, concatenated.
fn annotated_sources() -> String {
    main_rs() + "\n" + &pipeline_rs()
}

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
    let src = annotated_sources();
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
    let annotated = ids_after(&annotated_sources(), "@doc-flag:");
    assert!(
        annotated.contains("storage"),
        "@doc-flag: storage must exist for libviprs-org's pyramid.command.json \
         entry to have a counterpart, got {annotated:?}"
    );
}

#[test]
fn every_snippet_slot_is_opened_and_closed() {
    let src = annotated_sources();
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

/// `src/pipeline.rs`, where the pyramid driver lives since #66.
fn pipeline_rs() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "src/pipeline.rs must be readable at {}: {e}",
            path.display()
        )
    })
}

/// The snippet slots describe the code that runs. Since #66 every archive,
/// tree and object-store run goes through `src/pipeline.rs`, so the slots for
/// the plan, the memory check, the engine config and those three sinks sit
/// there, and `main.rs` keeps no copy of the driver they used to annotate.
#[test]
fn the_pyramid_driver_slots_sit_on_the_code_that_runs() {
    let pipeline = ids_after(&pipeline_rs(), "@doc-snippet:begin slot=");
    let main = ids_after(&main_rs(), "@doc-snippet:begin slot=");
    for slot in [
        "planner",
        "memory-limit",
        "engine-config",
        "sink-fs",
        "sink-pmtiles",
        "sink-s3",
    ] {
        assert!(
            pipeline.contains(slot),
            "slot {slot} must annotate the pipeline, got {pipeline:?}"
        );
        assert!(
            !main.contains(slot),
            "slot {slot} is still on a main.rs copy of the driver"
        );
    }
    assert!(
        !main_rs().contains("not yet fully wired"),
        "main.rs still carries the old s3 stub arm"
    );
}

/// The pyramid flag ids are a set libviprs.org compares against its embed in
/// both directions, so moving annotations between files must not add or drop
/// one. This is the set as it stood before #66 moved the driver.
#[test]
fn the_pyramid_flag_ids_are_the_set_the_site_knows() {
    let annotated = ids_after(&annotated_sources(), "@doc-flag:");
    let expected: BTreeSet<String> = [
        "blank-tolerance",
        "buffer-size",
        "centre",
        "checksum-algo",
        "concurrency",
        "dedupe-all",
        "dedupe-blanks",
        "dpi",
        "failure-policy",
        "format",
        "geo-origin",
        "geo-scale",
        "layout",
        "manifest-emit-checksums",
        "match-page-size",
        "memory-budget",
        "memory-limit",
        "overlap",
        "overwrite",
        "page",
        "parallel",
        "quality",
        "render",
        "resume",
        "retry-backoff",
        "retry-max",
        "sink",
        "skip-blank",
        "storage",
        "tile-size",
        "trace-level",
        "verify",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        annotated, expected,
        "the @doc-flag set changed; libviprs-org's pyramid.command.json has to change with it"
    );
}
