//! The hooks run this repo's gates on MARS and nowhere else (libviprs-cli#135).
//!
//! The hooks are shell, so these tests run them: in a throw-away git repo, with stand-ins for
//! `ssh`, `cargo`, `docker` and `rustup` first on PATH. The `ssh` stand-in records what it was
//! asked to run; the other three record that they were called at all, which they must never be.
//! That checks the property that matters (nothing builds on the machine that committed) without
//! reading the scripts for words.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// A scratch git repo with one commit whose subject is `subject`, plus a bin dir of stand-ins.
struct Sandbox {
    dir: PathBuf,
    bin: PathBuf,
    ssh_log: PathBuf,
    local_log: PathBuf,
    sha: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sandbox(tag: &str, subject: &str, ssh_exit: i32) -> Sandbox {
    let dir = std::env::temp_dir().join(format!("viprs-hooks-135-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bin = dir.join("bin");
    let repo = dir.join("repo");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("Cargo.toml"), "[package]\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", subject]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);

    let ssh_log = dir.join("ssh.log");
    let local_log = dir.join("local.log");
    let script = |name: &str, body: String| {
        let p = bin.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        Command::new("chmod").arg("+x").arg(&p).status().unwrap();
    };
    script(
        "ssh",
        format!(
            "echo \"$*\" >> '{}'\ncat >/dev/null\nexit {ssh_exit}",
            ssh_log.display()
        ),
    );
    for tool in ["cargo", "docker", "rustup", "cargo-clippy"] {
        script(tool, format!("echo {tool} >> '{}'", local_log.display()));
    }
    Sandbox {
        dir: repo,
        bin,
        ssh_log,
        local_log,
        sha,
    }
}

impl Sandbox {
    fn run(&self, script: &str, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        let mut cmd = Command::new("bash");
        cmd.current_dir(&self.dir)
            .arg(repo_root().join(script))
            .args(args)
            .env("PATH", path)
            .env_remove("VIPRS_GATE_HOST")
            .env_remove("LIBVIPRS_HOOKS_SKIP_MARS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("bash runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn ssh_calls(&self) -> String {
        fs::read_to_string(&self.ssh_log).unwrap_or_default()
    }

    fn local_tool_calls(&self) -> String {
        fs::read_to_string(&self.local_log).unwrap_or_default()
    }

    fn push_line(&self) -> String {
        format!(
            "refs/heads/x {} refs/heads/x {}\n",
            self.sha,
            "0".repeat(40)
        )
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn the_hooks_are_tracked_and_executable() {
    use std::os::unix::fs::PermissionsExt;
    for f in [
        "tools/gate.sh",
        "tools/hooks/pre-commit",
        "tools/hooks/pre-push",
        "tools/install-hooks.sh",
    ] {
        let meta = fs::metadata(repo_root().join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        assert!(
            meta.permissions().mode() & 0o111 != 0,
            "{f} must be executable"
        );
    }
}

#[test]
fn pre_commit_runs_the_lint_lane_on_mars_and_builds_nothing_locally() {
    let sb = sandbox("lint", "a change", 0);
    let out = sb.run("tools/hooks/pre-commit", &[], "", &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let calls = sb.ssh_calls();
    assert!(
        calls.contains("devshell@192.168.0.100"),
        "MARS is the default host: {calls}"
    );
    assert!(
        calls.contains("docker run --rm"),
        "a throw-away container: {calls}"
    );
    assert!(
        calls.contains("CARGO_TARGET_DIR=/tmp/target"),
        "target dir outside the tree: {calls}"
    );
    assert!(
        calls.contains("cargo clippy --all-targets --features full"),
        "{calls}"
    );
    assert!(calls.contains("cargo fmt -- --check"), "{calls}");
    assert!(
        !calls.contains("cargo test"),
        "tests belong to pre-push: {calls}"
    );
    assert_eq!(
        sb.local_tool_calls(),
        "",
        "cargo, docker and rustup must never run locally"
    );
}

#[test]
fn pre_push_runs_the_test_lane_on_mars_and_builds_nothing_locally() {
    let sb = sandbox("test", "a change", 0);
    let out = sb.run(
        "tools/hooks/pre-push",
        &["origin", "url"],
        &sb.push_line(),
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let calls = sb.ssh_calls();
    assert!(calls.contains("cargo test --features full"), "{calls}");
    assert!(
        calls.contains("cargo test --no-default-features"),
        "{calls}"
    );
    assert_eq!(sb.local_tool_calls(), "");
}

#[test]
fn an_unreachable_mars_fails_clearly_and_names_the_override() {
    let sb = sandbox("down", "a change", 255);
    let out = sb.run("tools/hooks/pre-commit", &[], "", &[]);
    assert!(
        !out.status.success(),
        "a gate that cannot run must not pass"
    );
    let err = text(&out.stderr);
    assert!(err.contains("could not reach MARS"), "{err}");
    assert!(err.contains("VIPRS_GATE_HOST"), "{err}");
    assert!(err.contains("LIBVIPRS_HOOKS_SKIP_MARS"), "{err}");
    assert_eq!(
        sb.local_tool_calls(),
        "",
        "no fallback to the local machine"
    );
}

#[test]
fn the_host_can_be_overridden() {
    let sb = sandbox("host", "a change", 0);
    let out = sb.run(
        "tools/hooks/pre-commit",
        &[],
        "",
        &[("VIPRS_GATE_HOST", "ci@other.example")],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let calls = sb.ssh_calls();
    assert!(calls.contains("ci@other.example"), "{calls}");
    assert!(!calls.contains("192.168.0.100"), "{calls}");
}

#[test]
fn a_red_tests_trailer_on_the_tip_compiles_and_lints_instead_of_running_tests() {
    let sb = sandbox("red", "Red tests: viprs does a thing\n\nRed-tests: 135", 0);
    let out = sb.run(
        "tools/hooks/pre-push",
        &["origin", "url"],
        &sb.push_line(),
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("Red-tests"),
        "{}",
        text(&out.stderr)
    );
    let calls = sb.ssh_calls();
    assert!(
        calls.contains("cargo test --no-run"),
        "every test must still build: {calls}"
    );
    assert!(
        calls.contains("cargo clippy --all-targets"),
        "and lint: {calls}"
    );
    assert!(
        !calls.contains("cargo test --features full"),
        "but not run: {calls}"
    );
}

#[test]
fn a_subject_alone_is_not_a_red_marker() {
    let sb = sandbox("subject", "Red tests: but no trailer", 0);
    let out = sb.run(
        "tools/hooks/pre-push",
        &["origin", "url"],
        &sb.push_line(),
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        sb.ssh_calls().contains("cargo test --features full"),
        "{}",
        sb.ssh_calls()
    );
}

#[test]
fn skip_mars_skips_loudly_and_never_builds_locally() {
    let sb = sandbox("skip", "a change", 255);
    for (script, args, stdin) in [
        ("tools/hooks/pre-commit", vec![], String::new()),
        (
            "tools/hooks/pre-push",
            vec!["origin", "url"],
            sb.push_line(),
        ),
    ] {
        let out = sb.run(script, &args, &stdin, &[("LIBVIPRS_HOOKS_SKIP_MARS", "1")]);
        assert!(out.status.success(), "{script}: {}", text(&out.stderr));
        assert!(
            text(&out.stderr).contains("SKIPPED"),
            "{script}: {}",
            text(&out.stderr)
        );
    }
    assert_eq!(sb.ssh_calls(), "");
    assert_eq!(sb.local_tool_calls(), "");
}

#[test]
fn deleting_a_branch_runs_nothing() {
    let sb = sandbox("del", "a change", 0);
    let line = format!("(delete) {} refs/heads/x {}\n", "0".repeat(40), sb.sha);
    let out = sb.run("tools/hooks/pre-push", &["origin", "url"], &line, &[]);
    assert!(out.status.success());
    assert_eq!(sb.ssh_calls(), "");
}

/// The `cargo ...` commands in a file, one per `run:` step or script line.
fn cargo_cells(src: &str, ci: bool) -> std::collections::BTreeSet<String> {
    src.lines()
        .map(str::trim)
        .filter_map(|l| {
            let l = if ci {
                l.strip_prefix("- run: ")?
            } else {
                l.strip_prefix("RUSTFLAGS= ").unwrap_or(l)
            };
            l.starts_with("cargo ").then(|| l.to_string())
        })
        .collect()
}

#[test]
fn the_gate_runs_every_cargo_cell_ci_runs() {
    let ci = cargo_cells(&read(".github/workflows/ci.yml"), true);
    let gate = cargo_cells(&read("tools/gate.sh"), false);
    assert!(!ci.is_empty());
    let missing: Vec<_> = ci.difference(&gate).collect();
    let extra: Vec<_> = gate.difference(&ci).collect();
    assert!(
        missing.is_empty(),
        "CI runs these and the gate does not: {missing:#?}"
    );
    // The one cell CI has no counterpart for: building the tests of a red tip without running them.
    assert_eq!(
        extra,
        ["cargo test --no-run"],
        "the gate runs cells CI does not"
    );
}

#[test]
fn the_readme_documents_the_hooks() {
    let readme = read("README.md");
    for needle in [
        "tools/install-hooks.sh",
        "VIPRS_GATE_HOST",
        "LIBVIPRS_HOOKS_SKIP_MARS",
        "Red-tests:",
        "MARS",
    ] {
        assert!(readme.contains(needle), "README must mention `{needle}`");
    }
}
