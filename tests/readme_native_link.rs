//! The README says how a `dwg-acadsharp` binary finds its native library, and
//! what a decode that reports errors exits (issue #121).
//!
//! `acadsharp-rs` links `libacadsharp_native` as a shared library and the
//! binary carries no rpath, so without `LD_LIBRARY_PATH` the loader refuses to
//! start it. That is a fact about how the binary is built, measured on Linux
//! x64 (`readelf -d` shows `NEEDED libacadsharp_native.so` and no `RUNPATH`).
//! A text guard cannot re-measure it, but it can keep the README from losing the
//! sentence, and from claiming an exit code the code does not have.

use std::path::Path;

fn readme() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap()
}

/// The README text from the `viprs vector` section to the next `##` heading.
fn vector_section() -> String {
    let text = readme();
    let start = text
        .find("### `viprs vector`")
        .expect("README has no viprs vector section");
    let rest = &text[start + 4..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].to_owned()
}

#[test]
fn the_vector_section_says_the_native_library_is_linked_shared() {
    let s = vector_section();
    for needle in [
        "linked shared",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "libacadsharp_native",
    ] {
        assert!(s.contains(needle), "the vector section lost `{needle}`");
    }
}

#[test]
fn the_vector_section_says_a_decode_with_errors_still_exits_zero() {
    let s = vector_section();
    assert!(s.contains("still exits 0"), "{s}");
    assert!(s.contains("decode.has_errors"), "{s}");
    assert!(s.contains("decode.complete"), "{s}");
}
