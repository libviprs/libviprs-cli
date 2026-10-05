//! Guards on how the cells under `tests/` are gated on the sink features.
//!
//! The `s3://` sink is built when either `s3` or `object-store-sink` is on
//! (`src/pipeline.rs`, the two `object_store` modules), so a cell that is only
//! meant for a build without it has to be gated on both. A cell gated on `s3`
//! alone runs in an object-store-sink-only build against the real sink and
//! fails there (libviprs-cli#95), and nothing else in the default, `full` or
//! no-default test runs would notice.
//!
//! This reads the test sources rather than building every feature combination,
//! because the combination that breaks is exactly the one nobody runs.

/// Every `tests/*.rs` file, as `(name, source)`.
fn test_sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("read tests/")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&p).expect("read a test file"))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn no_cell_is_gated_on_s3_without_object_store_sink() {
    // Spelled in two halves so this file does not match itself.
    let lone = ["cfg(not(feature = ", "\"s3\"))]"].concat();
    let mut bad = Vec::new();
    for (name, src) in test_sources() {
        for (i, line) in src.lines().enumerate() {
            if line.contains(&lone) {
                bad.push(format!("tests/{name}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "these cells are gated on `s3` alone, so an object-store-sink-only build \
         runs them against the real sink; gate them \
         `not(any(feature = \"s3\", feature = \"object-store-sink\"))`:\n{}",
        bad.join("\n")
    );
}
