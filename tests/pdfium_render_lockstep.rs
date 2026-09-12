//! Guards that this crate and the core `libviprs` crate resolve the *same*
//! `pdfium-render`, and that no `[patch]` entry here has gone inert.
//!
//! The fork existed because `ThreadSafePdfiumBindings` segfaulted under
//! concurrent access to a shared `Pdfium`, so "the CLI and the core link the
//! same copy" is a soundness property rather than tidiness. Issue #45 is what
//! happens without a check: the core moved off the fork, the `[patch]` table
//! here kept naming it, and cargo reported that on every single build as
//! `warning: patch ... was not used in the crate graph` — where it scrolled
//! past unread for weeks.
//!
//! These read `Cargo.lock` rather than that warning, because the lockfile is a
//! checkable artefact and the warning is not. Two properties of it matter, and
//! the second one is the reason this file exists rather than a grep for the
//! word "patch" in `Cargo.toml`:
//!
//! * Cargo records an ignored patch **in the lockfile**, as a `[[patch.unused]]`
//!   table. That survives re-resolution, so it is visible to a test even though
//!   cargo rewrites `Cargo.lock` before the test binary runs.
//! * A `[patch]` entry naming a crate that *is* in the graph still leaves one
//!   `[[package]]` stanza, so counting stanzas cannot tell an applied patch from
//!   an ignored one. `[[patch.unused]]` can, and is what cargo itself uses.
//!
//! `../libviprs` is always present when these run: it is this crate's path
//! dependency, so `cargo test` cannot have built the binary without it, and CI
//! clones it to that exact location before every job.

use std::path::{Path, PathBuf};

/// One `[[package]]` stanza from `Cargo.lock`.
#[derive(Debug)]
struct LockedPackage {
    version: String,
    /// `None` for a path dependency or workspace member, which `Cargo.lock`
    /// writes with no `source` key at all.
    source: Option<String>,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()))
}

fn lock_text() -> String {
    read(&manifest_dir().join("Cargo.lock"))
}

fn core_manifest_text() -> String {
    read(&manifest_dir().join("../libviprs/Cargo.toml"))
}

/// The value of a `key = "value"` line, if this is one.
fn string_value(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim();
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

/// Split `Cargo.lock` into `(header, body)` pairs, one per array-of-tables
/// stanza.
///
/// `Cargo.lock` is generated, so its shape is stable in a way a hand-written
/// TOML file is not: one `[[table]]` header per stanza, bare top-level
/// `key = "value"` lines, and no nesting. Scanning it needs no TOML
/// dependency, and adding one would put a third-party crate into the very
/// dependency graph this file exists to police.
fn stanzas() -> Vec<(String, String)> {
    let lock = lock_text();
    let mut out = Vec::new();

    for index in lock.match_indices("\n[[").map(|(at, _)| at) {
        let after = &lock[index + 3..];
        let Some(close) = after.find("]]") else {
            continue;
        };
        let header = after[..close].to_string();
        let body_start = index + 3 + close + 2;
        let body = &lock[body_start..];
        // A stanza runs to the next top-level table header. Dependency arrays
        // open with `[` at end of line, never at the start of one, so `\n[`
        // cannot match inside a stanza body.
        let body = match body.find("\n[") {
            Some(at) => &body[..at],
            None => body,
        };
        out.push((header, body.to_string()));
    }

    out
}

/// Every `[[package]]` stanza naming `wanted`.
fn locked_packages(wanted: &str) -> Vec<LockedPackage> {
    let mut found = Vec::new();

    for (header, body) in stanzas() {
        if header != "package" {
            continue;
        }

        let mut name = None;
        let mut version = None;
        let mut source = None;
        for line in body.lines().map(str::trim) {
            if let Some(v) = string_value(line, "name") {
                name = Some(v);
            } else if let Some(v) = string_value(line, "version") {
                version = Some(v);
            } else if let Some(v) = string_value(line, "source") {
                source = Some(v);
            }
        }

        if name.as_deref() == Some(wanted) {
            found.push(LockedPackage {
                version: version.unwrap_or_else(|| {
                    panic!("the {wanted} stanza in Cargo.lock must carry a version")
                }),
                source,
            });
        }
    }

    found
}

/// Every patch cargo recorded as ignored, as `(name, source)`.
fn unused_patches() -> Vec<(String, String)> {
    let mut out = Vec::new();

    for (header, body) in stanzas() {
        if header != "patch.unused" {
            continue;
        }

        let mut name = None;
        let mut source = None;
        for line in body.lines().map(str::trim) {
            if let Some(v) = string_value(line, "name") {
                name = Some(v);
            } else if let Some(v) = string_value(line, "source") {
                source = Some(v);
            }
        }

        out.push((
            name.unwrap_or_else(|| "<unnamed>".to_string()),
            source.unwrap_or_else(|| "<no source>".to_string()),
        ));
    }

    out
}

/// The version requirement the core crate declares for `dep`.
fn core_requirement(dep: &str) -> String {
    let manifest = core_manifest_text();
    let needle = format!("\n{dep} = {{");
    let at = manifest.find(&needle).unwrap_or_else(|| {
        panic!("the core manifest must declare `{dep}` as a table; the shape of that line changed")
    });

    // The declaration spans several lines (a `features` array), so search
    // forward from its start rather than within one line.
    let tail = &manifest[at + 1..];
    let end = tail
        .find('}')
        .unwrap_or_else(|| panic!("the core's `{dep}` table must be closed"));
    let decl = &tail[..end];

    let version_at = decl
        .find("version")
        .unwrap_or_else(|| panic!("the core's `{dep}` table must pin a `version`"));
    string_value(decl[version_at..].trim(), "version")
        .unwrap_or_else(|| panic!("the core's `{dep}` version must be a quoted string"))
}

#[test]
fn cargo_recorded_no_ignored_patch() {
    let unused = unused_patches();

    assert!(
        unused.is_empty(),
        "Cargo.lock records {} ignored [patch] entr{}, so the `[patch]` table in \
         Cargo.toml is claiming something that is not happening:\n{}\n\
         This is issue #45's failure mode exactly. Either the patch is obsolete \
         and the table should go, or the patched crate left the graph and the \
         build is linking something nobody chose.",
        unused.len(),
        if unused.len() == 1 { "y" } else { "ies" },
        unused
            .iter()
            .map(|(name, source)| format!("  {name}  <-  {source}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn pdfium_render_resolves_from_the_registry_at_the_core_version() {
    let required = core_requirement("pdfium-render");
    let locked = locked_packages("pdfium-render");

    // The positive control. This crate builds `pdfium` by default, so an empty
    // result means resolution moved somewhere this guard can no longer see, and
    // the two assertions below would otherwise pass over an empty list. A
    // vacuous pass here is the specific way a guard in this org has gone wrong
    // before, so it is checked rather than assumed.
    assert_eq!(
        locked.len(),
        1,
        "Cargo.lock must record exactly one pdfium-render, found {}: {locked:#?}",
        locked.len(),
    );

    let pkg = &locked[0];
    let source = pkg
        .source
        .as_deref()
        .expect("pdfium-render is not a workspace member, so it must carry a source");

    assert!(
        source.starts_with("registry+"),
        "pdfium-render must resolve from the registry, not a fork; got {source}\n\
         The core crate retired the libviprs/pdfium-render fork when upstream \
         released 0.9.4 with the thread-safe bindings reinstated. A git source \
         here means this crate has gone back to resolving something the core \
         does not build against."
    );

    assert_eq!(
        pkg.version, required,
        "the core declares pdfium-render {required} but this crate's lockfile \
         resolves {}. The fork existed because ThreadSafePdfiumBindings \
         segfaulted under concurrent access to a shared Pdfium, so the two \
         crates linking the same copy is a soundness property.",
        pkg.version,
    );
}
