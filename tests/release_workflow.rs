//! Guards the release workflow (`.github/workflows/release.yml`, issue #110).
//!
//! That workflow only ever runs on GitHub, on a push to the `release` branch,
//! and the push publishes for real. So nothing here can run it, and nothing
//! gets a second try if one of its promises has quietly drifted. What these
//! tests can do is read the file and refuse to be green when a claim that the
//! release depends on is no longer true: the trigger, the two targets, the
//! Rust pin agreeing with `rust-version`, the checksum file, no
//! `pull_request_target`, and every third-party action pinned by commit.
//!
//! There is no YAML parser in the dependency tree and a guard is not worth a
//! new one, so the file is read the way `tests/pdfium_render_lockstep.rs`
//! reads `Cargo.lock`: line by line, keyed on indentation. The workflow keeps
//! to a plain layout (block mappings, one inline list for `branches`) so that
//! stays honest. A construct this reader cannot see fails a test instead of
//! slipping past it.

use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = manifest_dir().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()))
}

fn workflow() -> String {
    read(".github/workflows/release.yml")
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A line with its trailing `# comment` and surrounding space removed. Only
/// cuts at ` #` so a `#` inside a value (a URL fragment) survives.
fn code(line: &str) -> &str {
    let line = match line.find(" #") {
        Some(i) => &line[..i],
        None => line,
    };
    line.trim()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

/// The lines nested under the column-0 key `key:` (exclusive of the key line).
fn top_block(text: &str, key: &str) -> Vec<String> {
    let header = format!("{key}:");
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if is_blank_or_comment(line) {
            continue;
        }
        if indent(line) == 0 {
            inside = code(line) == header || code(line).starts_with(&format!("{header} "));
            continue;
        }
        if inside {
            out.push(line.to_string());
        }
    }
    out
}

/// `(job id, its lines)` for every job under `jobs:`.
fn jobs(text: &str) -> Vec<(String, Vec<String>)> {
    let block = top_block(text, "jobs");
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in block {
        if indent(&line) == 2 && code(&line).ends_with(':') {
            let id = code(&line).trim_end_matches(':').to_string();
            out.push((id, Vec::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push(line);
        }
    }
    out
}

/// The `Cargo.toml` value of a top-level-table `key = "value"`.
fn cargo_value(key: &str) -> String {
    let manifest = read("Cargo.toml");
    for line in manifest.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                return rest.trim().trim_matches('"').to_string();
            }
        }
    }
    panic!("Cargo.toml has no `{key} = \"...\"` line");
}

fn bin_name() -> String {
    let manifest = read("Cargo.toml");
    let mut in_bin = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_bin = t == "[[bin]]";
            continue;
        }
        if in_bin && let Some(rest) = t.strip_prefix("name") {
            return rest
                .trim_start()
                .trim_start_matches('=')
                .trim()
                .trim_matches('"')
                .to_string();
        }
    }
    panic!("Cargo.toml has no [[bin]] name");
}

#[test]
fn workflow_file_exists() {
    let text = workflow();
    assert!(text.contains("jobs:"), "release.yml has no jobs");
}

#[test]
fn triggers_on_push_to_release_only() {
    let text = workflow();
    let on: Vec<String> = top_block(&text, "on")
        .iter()
        .map(|l| code(l).to_string())
        .collect();
    assert!(
        on.iter().any(|l| l == "push:"),
        "no `push:` trigger: {on:?}"
    );
    assert!(
        on.iter()
            .any(|l| l == "branches: [release]" || l == "branches: ['release']"),
        "push must be limited to `branches: [release]`: {on:?}"
    );
    assert!(
        !on.iter()
            .any(|l| l.contains("'**'") || l.contains("\"**\"")),
        "a wildcard branch would publish from any push: {on:?}"
    );
    assert!(
        !on.iter().any(|l| l == "tags:"),
        "the release is cut by the branch push, not a tag push: {on:?}"
    );
}

#[test]
fn workflow_dispatch_has_a_dry_run_that_defaults_on() {
    let text = workflow();
    let on: Vec<String> = top_block(&text, "on")
        .iter()
        .map(|l| code(l).to_string())
        .collect();
    assert!(
        on.iter().any(|l| l == "workflow_dispatch:"),
        "no workflow_dispatch trigger: {on:?}"
    );
    let at = on
        .iter()
        .position(|l| l == "dry_run:")
        .unwrap_or_else(|| panic!("workflow_dispatch has no `dry_run` input: {on:?}"));
    let input: Vec<&String> = on.iter().skip(at + 1).take(4).collect();
    assert!(
        input.iter().any(|l| l.as_str() == "type: boolean"),
        "dry_run must be a boolean: {input:?}"
    );
    assert!(
        input.iter().any(|l| l.as_str() == "default: true"),
        "dry_run must default to true, so a manual run publishes nothing unless asked: {input:?}"
    );
}

#[test]
fn never_uses_pull_request_target() {
    let text = workflow();
    for line in text.lines().filter(|l| !is_blank_or_comment(l)) {
        assert!(
            !code(line).contains("pull_request_target"),
            "pull_request_target runs with write tokens on untrusted code: {line}"
        );
    }
}

#[test]
fn builds_both_targets_on_the_right_runners() {
    let text = workflow();
    let lines: Vec<&str> = text.lines().map(code).collect();

    let linux = lines
        .iter()
        .position(|l| *l == "target: x86_64-unknown-linux-gnu")
        .expect("matrix lacks `target: x86_64-unknown-linux-gnu`");
    let mac = lines
        .iter()
        .position(|l| *l == "target: aarch64-apple-darwin")
        .expect("matrix lacks `target: aarch64-apple-darwin`");

    // Each matrix entry names its runner on the line next to its target.
    let near = |at: usize| lines[at.saturating_sub(2)..(at + 3).min(lines.len())].to_vec();
    let linux_os = near(linux)
        .into_iter()
        .find_map(|l| l.strip_prefix("os: "))
        .expect("the linux entry names no `os:`");
    assert!(
        linux_os.starts_with("ubuntu-") && !linux_os.contains("arm"),
        "x86_64-unknown-linux-gnu needs an x64 Linux runner, got {linux_os}"
    );
    let mac_os = near(mac)
        .into_iter()
        .find_map(|l| l.strip_prefix("os: "))
        .expect("the macOS entry names no `os:`");
    let n: u32 = mac_os
        .strip_prefix("macos-")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            panic!("aarch64-apple-darwin must name a numbered macos runner, got {mac_os}")
        });
    assert!(
        n >= 14 && !mac_os.contains("13"),
        "macos-13 and older are Intel, aarch64-apple-darwin needs macos-14 or newer, got {mac_os}"
    );
}

#[test]
fn rust_pin_matches_cargo_toml_rust_version() {
    let text = workflow();
    let declared = cargo_value("rust-version");
    assert_eq!(
        declared.split('.').count(),
        2,
        "this guard expects rust-version as MAJOR.MINOR, got {declared}"
    );
    let pin = text
        .lines()
        .map(code)
        .find_map(|l| l.strip_prefix("RUST_VERSION:"))
        .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
        .expect("release.yml must pin the toolchain in an `RUST_VERSION:` env entry");
    let parts: Vec<&str> = pin.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "pin the exact patch release (X.Y.Z), not a moving channel: {pin}"
    );
    assert_eq!(
        format!("{}.{}", parts[0], parts[1]),
        declared,
        "release.yml builds with Rust {pin} but Cargo.toml declares rust-version {declared}"
    );
    assert_eq!(pin, "1.99.0", "the release is pinned to Rust 1.99.0");
    assert!(
        text.lines()
            .map(code)
            .any(|l| l.contains("rustup toolchain install") && l.contains("RUST_VERSION")),
        "the pin must be what actually gets installed"
    );
}

#[test]
fn build_is_release_locked_with_default_features() {
    let text = workflow();
    let build = text
        .lines()
        .map(code)
        .find(|l| l.starts_with("cargo build") || l.contains(" cargo build "))
        .expect("no `cargo build` step");
    for want in ["--release", "--locked", "--target"] {
        assert!(build.contains(want), "build line lacks {want}: {build}");
    }
    // The default features are what the CLI ships (`default = ["pdfium"]`).
    // Turning them off or adding some here would make the release differ from
    // `cargo install`.
    for banned in ["--no-default-features", "--features", "--all-features"] {
        assert!(
            !build.contains(banned),
            "the release must build the default feature set, found {banned}: {build}"
        );
    }
}

#[test]
fn artifacts_are_named_per_target_and_hold_the_binary() {
    let text = workflow();
    let bin = bin_name();
    // `$` or `${{ }}` stand in for the version and target.
    assert!(
        text.contains("libviprs-cli-") && text.contains(".tar.gz"),
        "no libviprs-cli-<tag>-<target>.tar.gz archive"
    );
    let stem = text
        .lines()
        .map(code)
        .find(|l| l.contains("libviprs-cli-") && !l.starts_with("name:") && !l.starts_with("echo"))
        .expect("no line builds the archive name");
    assert!(
        stem.contains("TARGET") || stem.contains("matrix.target"),
        "the archive name must carry the target: {stem}"
    );
    assert!(
        text.contains(&format!("target/${{TARGET}}/release/{bin}"))
            || text.contains(&format!("target/$TARGET/release/{bin}")),
        "the packaged binary must be the [[bin]] `{bin}` from the target dir"
    );
    for file in ["README.md", "LICENSE"] {
        assert!(
            text.lines().map(code).any(|l| l.contains(file)),
            "{file} is not packaged"
        );
    }
    assert!(
        text.lines()
            .map(code)
            .any(|l| l.starts_with("strip ") || l.contains(" strip ")),
        "the binary is not stripped"
    );
}

#[test]
fn checksums_are_produced_and_attached() {
    let text = workflow();
    let lines: Vec<&str> = text.lines().map(code).collect();
    assert!(
        lines.iter().any(|l| l.contains("SHA256SUMS")
            && (l.contains("sha256sum") || l.contains("shasum -a 256") || l.contains(">"))),
        "no step writes a SHA256SUMS file"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("sha256sum ") || l.contains("shasum -a 256")),
        "nothing computes a sha256"
    );
    // The release call has to name it, or it is built and then dropped.
    let release = lines
        .iter()
        .position(|l| l.contains("gh release create"))
        .expect("no `gh release create`");
    let tail = lines[release..(release + 12).min(lines.len())].join(" ");
    assert!(
        tail.contains("SHA256SUMS"),
        "gh release create does not attach SHA256SUMS: {tail}"
    );
}

#[test]
fn every_action_is_pinned_by_full_commit_sha() {
    let text = workflow();
    let mut seen = 0;
    for line in text.lines().map(code) {
        let Some(rest) = line
            .strip_prefix("- uses:")
            .or_else(|| line.strip_prefix("uses:"))
        else {
            continue;
        };
        let spec = rest.trim();
        seen += 1;
        let (name, rev) = spec
            .split_once('@')
            .unwrap_or_else(|| panic!("`uses: {spec}` has no @revision"));
        assert!(
            rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()),
            "`uses: {name}` must be pinned by a full 40-hex commit SHA, got `{rev}` \
             (a tag or branch can be moved under the release)"
        );
        assert!(
            !name.starts_with("./") && !name.contains("docker://"),
            "unexpected action source: {name}"
        );
    }
    assert!(
        seen > 0,
        "the workflow uses no actions, is the reader broken?"
    );
}

#[test]
fn permissions_are_read_only_except_on_the_publish_job() {
    let text = workflow();
    let top: Vec<String> = top_block(&text, "permissions")
        .iter()
        .map(|l| code(l).to_string())
        .collect();
    assert_eq!(
        top,
        vec!["contents: read".to_string()],
        "the workflow-level permissions must be exactly `contents: read`"
    );
    let writers: Vec<String> = jobs(&text)
        .into_iter()
        .filter(|(_, body)| body.iter().any(|l| code(l) == "contents: write"))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        writers,
        vec!["publish".to_string()],
        "only the final `publish` job may hold contents: write"
    );
    for (id, body) in jobs(&text) {
        for l in body.iter().map(|l| code(l)) {
            assert!(
                !l.ends_with(": write") || (id == "publish" && l == "contents: write"),
                "job {id} asks for `{l}`"
            );
        }
    }
    assert!(
        !text.contains("write-all"),
        "write-all is not minimal permissions"
    );
}

#[test]
fn publish_needs_the_builds_and_uses_only_the_github_token() {
    let text = workflow();
    let all = jobs(&text);
    let (_, publish) = all
        .iter()
        .find(|(id, _)| id == "publish")
        .expect("no `publish` job");
    let body: Vec<&str> = publish.iter().map(|l| code(l)).collect();
    let needs = body
        .iter()
        .find(|l| l.starts_with("needs:"))
        .expect("publish has no `needs:`");
    assert!(
        needs.contains("build"),
        "publish must wait for the build matrix: {needs}"
    );
    // Secrets: only the built-in token may appear anywhere.
    for line in text.lines().map(code) {
        let mut rest = line;
        while let Some(at) = rest.find("secrets.") {
            let name: String = rest[at + 8..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            assert_eq!(name, "GITHUB_TOKEN", "only GITHUB_TOKEN is allowed: {line}");
            rest = &rest[at + 8..];
        }
    }
    // The tag has to be annotated, which the git data API does with a tag
    // object before the ref.
    assert!(
        text.contains("git/tags") && text.contains("git/refs"),
        "the tag must be an annotated tag object plus a ref"
    );
}

#[test]
fn checkouts_do_not_leave_a_token_on_disk() {
    let text = workflow();
    let lines: Vec<&str> = text.lines().collect();
    let mut checkouts = 0;
    for (i, line) in lines.iter().enumerate() {
        if !code(line).contains("actions/checkout@") {
            continue;
        }
        checkouts += 1;
        let step_indent = indent(line);
        let mut ok = false;
        for next in &lines[i + 1..] {
            if is_blank_or_comment(next) {
                continue;
            }
            // The next step starts with `- ` at (or left of) this one's dash.
            if indent(next) < step_indent + 2 && next.trim_start().starts_with("- ") {
                break;
            }
            if indent(next) < step_indent {
                break;
            }
            if code(next) == "persist-credentials: false" {
                ok = true;
            }
        }
        assert!(
            ok,
            "checkout at line {} keeps credentials in .git/config",
            i + 1
        );
    }
    assert!(checkouts > 0, "no checkout step found");
}

#[test]
fn the_version_gates_run_before_anything_is_built() {
    let text = workflow();
    // Version read from Cargo.toml, CHANGELOG section required, tag refused.
    for needle in ["Cargo.toml", "CHANGELOG.md", "## [", "already exists"] {
        assert!(
            text.contains(needle),
            "the version/changelog/tag gate is missing `{needle}`"
        );
    }
    let all = jobs(&text);
    let (_, build) = all
        .iter()
        .find(|(id, _)| id == "build")
        .expect("no `build` job");
    assert!(
        build.iter().any(|l| code(l).starts_with("needs:")),
        "build must wait on the gate job so a refused release builds nothing"
    );
}

/// 0.5.0 was never released: the CLI went from 0.4.0 to 0.6.0 to line up with
/// libviprs 0.6.0, so a `## [0.5.0]` heading would claim a version nobody got.
#[test]
fn changelog_has_no_phantom_0_5_release() {
    let changelog = read("CHANGELOG.md");
    assert!(
        !changelog.lines().any(|l| l.starts_with("## [0.5")),
        "CHANGELOG.md has a 0.5.x section, but 0.5 was skipped"
    );
}

/// The cut has to leave `--locked` buildable and the notes findable: the
/// manifest version, the lockfile entry and a sealed CHANGELOG heading agree,
/// and `[Unreleased]` is bare.
#[test]
fn cargo_version_lockfile_and_changelog_agree() {
    let version = cargo_value("version");
    let lock = read("Cargo.lock");
    let mut lines = lock.lines();
    let mut locked = None;
    while let Some(l) = lines.next() {
        if l == "name = \"libviprs-cli\"" {
            locked = lines
                .next()
                .and_then(|v| v.strip_prefix("version = "))
                .map(|v| v.trim_matches('"').to_string());
            break;
        }
    }
    assert_eq!(
        locked.as_deref(),
        Some(version.as_str()),
        "Cargo.lock's libviprs-cli entry is not {version}, `--locked` would fail"
    );

    let changelog = read("CHANGELOG.md");
    let heads: Vec<&str> = changelog
        .lines()
        .filter(|l| l.starts_with("## ["))
        .collect();
    assert_eq!(heads.first(), Some(&"## [Unreleased]"));
    assert!(
        heads
            .get(1)
            .is_some_and(|h| h.starts_with(&format!("## [{version}] "))),
        "the section right below [Unreleased] must be {version}: {heads:?}"
    );
    let after_unreleased = changelog
        .lines()
        .skip_while(|l| *l != "## [Unreleased]")
        .skip(1)
        .take_while(|l| !l.starts_with("## ["))
        .any(|l| !l.trim().is_empty());
    assert!(!after_unreleased, "[Unreleased] must be bare after a cut");
}
