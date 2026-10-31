#!/usr/bin/env bash
# The gates for this repo, run on MARS and never on this machine.
#
#   tools/gate.sh lint    the lint half of CI (the `check` job): fmt, and clippy over every feature cell
#   tools/gate.sh test    the test half of CI (the `msrv` and `test` jobs)
#   tools/gate.sh compile lint, then build every test without running one (a "Red-tests:" tip)
#   tools/gate.sh clean   drop the cargo and target volumes on MARS
#
# The hooks in tools/hooks call this: pre-commit runs `lint`, pre-push runs `test`. Nothing
# here runs cargo, clippy or docker locally. The tree (tracked files plus untracked ones that
# are not ignored, so a new file is gated too) is streamed over ssh into a throw-away rust
# container on MARS, which is removed when the job ends. No scratch directory is left on the
# host. Two named docker volumes keep the cargo downloads and the target dir between runs
# (CARGO_TARGET_DIR=/tmp/target in the container); `tools/gate.sh clean` removes them.
#
# Environment:
#   VIPRS_GATE_HOST    ssh target, default devshell@192.168.0.100 (MARS). Point it at another
#                      docker host if MARS is down.
#   LIBVIPRS_HOOKS_SKIP_MARS=1
#                      skip the gate, loudly, when MARS is down. The same switch the other
#                      libviprs repos' hooks use. Nothing ever falls back to running locally.
#   VIPRS_GATE_IMAGE   the rust image, default rust:1.99.1 (the MSRV floor, MAJOR.MINOR must
#                      match `rust-version`; the msrv step below checks it)
#   VIPRS_GATE_CPUS    cpus for the container, default 8
#   VIPRS_GATE_MEMORY  memory for the container, default 12g
#
# The cells below are the `run:` steps of the CI jobs. tests/hooks_gate_135.rs reads both files
# and goes red if they stop agreeing, so a new CI cell cannot be missed here.
set -euo pipefail

HOST=${VIPRS_GATE_HOST:-devshell@192.168.0.100}
IMAGE=${VIPRS_GATE_IMAGE:-rust:1.99.1}
CPUS=${VIPRS_GATE_CPUS:-8}
MEMORY=${VIPRS_GATE_MEMORY:-12g}
CARGO_VOL=viprs-cli-cargo
TARGET_VOL=viprs-cli-target
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=10 -o LogLevel=ERROR "$HOST")

lane=${1:-}

if [ -n "${LIBVIPRS_HOOKS_SKIP_MARS:-}" ] && [ "$lane" != clean ]; then
  echo "gate: LIBVIPRS_HOOKS_SKIP_MARS is set, so the $lane gate is SKIPPED. Nothing was checked; CI-equivalent gates still have to run on MARS before merge." >&2
  exit 0
fi

unreachable() {
  cat >&2 <<MSG

The gate could not reach MARS ($HOST), so it did not run.
The hooks run the gates there on purpose: cargo, clippy and docker never run on this machine.
  - Check the network and 'ssh $HOST true'.
  - To use another docker host: VIPRS_GATE_HOST=user@host git commit ...
  - To skip the gate for this one command, loudly: LIBVIPRS_HOOKS_SKIP_MARS=1 git commit ...
  - Emergency only: git commit --no-verify (say so in the PR).
MSG
  exit 1
}

# The lint half. Mirrors the `check` job in .github/workflows/ci.yml.
lint_script() {
  cat <<'SCRIPT'
# --- container:lint
export RUSTFLAGS=-Dwarnings
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features tracing -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features packfile -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features s3 -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features avif -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features svg -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features jxl -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features jp2k -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features dwg-acadsharp -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features object-store-sink -- -D warnings -W clippy::incompatible_msrv -W deprecated
RUSTFLAGS= cargo clippy --all-targets --features pdfium-static -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --features full -- -D warnings -W clippy::incompatible_msrv -W deprecated
cargo clippy --all-targets --no-default-features -- -D warnings -W clippy::incompatible_msrv -W deprecated
# --- container:end
SCRIPT
}

# The test half. Mirrors the `msrv` and `test` jobs.
test_script() {
  cat <<'SCRIPT'
# --- container:test
export RUSTFLAGS=-Dwarnings
core=$(grep -m1 -E '^rust-version *= *"' ../libviprs/Cargo.toml | sed -E 's/.*"([^"]*)".*/\1/' | cut -d. -f1,2)
declared=$(grep -m1 -E '^rust-version *= *"' Cargo.toml | sed -E 's/.*"([^"]*)".*/\1/' | cut -d. -f1,2)
pinned=$(rustc --version | sed -E 's/^rustc ([0-9]+\.[0-9]+).*/\1/')
advertised=$(grep -oE 'rust-[0-9]+\.[0-9]+%2B|Rust [0-9]+\.[0-9]+\+' README.md | grep -oE '[0-9]+\.[0-9]+' | sort -u | paste -sd/ -)
echo "declared (Cargo.toml): $declared  pinned (image): $pinned  advertised (README): $advertised"
[ "$declared" = "$pinned" ] || { echo "image rustc $pinned but Cargo.toml declares rust-version $declared"; exit 1; }
[ "$declared" = "$advertised" ] || { echo "README advertises Rust $advertised+ but Cargo.toml declares $declared"; exit 1; }
[ "$(printf '%s\n%s\n' "$core" "$declared" | sort -V | tail -1)" = "$declared" ] || { echo "libviprs needs $core, above the $declared this crate declares"; exit 1; }
cargo check --all-targets
cargo test
cargo test --no-default-features
cargo test --features full
# --- container:end
SCRIPT
}

# The lint lane plus a build of every test, for a tip that is red on purpose.
compile_script() {
  lint_script
  printf '%s\n' '# --- container:compile' 'cargo test --no-run' '# --- container:end'
}

# Run a script in a container on MARS with the tree streamed in on stdin.
run_remote() {
  local script=$1
  "${SSH[@]}" true 2>/dev/null || unreachable
  local top
  top=$(git rev-parse --show-toplevel)
  local inner
  # The tests read ../libviprs (tests/pdfium_render_lockstep.rs), and CI clones the core beside
  # this repo for every job, matching branch or main. Do the same, inside the container.
  local branch
  branch=$(git -C "$top" rev-parse --abbrev-ref HEAD 2>/dev/null || echo main)
  inner="set -eu; mkdir -p /w/libviprs-cli; tar -x -C /w/libviprs-cli; cd /w/libviprs-cli; \
(git clone -q --depth 1 --branch '$branch' https://github.com/libviprs/libviprs.git ../libviprs 2>/dev/null \
  || git clone -q --depth 1 https://github.com/libviprs/libviprs.git ../libviprs); \
(cargo fmt --version >/dev/null 2>&1 && cargo clippy --version >/dev/null 2>&1) || rustup component add clippy rustfmt; \
$script"
  echo "gate: $lane on $HOST in $IMAGE (tree streamed in, container removed afterwards)" >&2
  # Files git lists but the working tree no longer has (a staged delete) are dropped from the list.
  (cd "$top" && git ls-files -z --cached --others --exclude-standard \
    | while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done \
    | tar --null -T - -cf - 2>/dev/null) \
    | "${SSH[@]}" "docker run --rm -i --platform linux/amd64 --cpus=$CPUS --memory=$MEMORY \
        -v $CARGO_VOL:/tmp/cargo -v $TARGET_VOL:/tmp/target \
        -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/tmp/target -e CARGO_TERM_COLOR=never \
        $IMAGE sh -c $(printf '%q' "$inner")"
}

case $lane in
  lint) run_remote "$(lint_script)" ;;
  test) run_remote "$(test_script)" ;;
  compile) run_remote "$(compile_script)" ;;
  clean)
    "${SSH[@]}" true 2>/dev/null || unreachable
    "${SSH[@]}" "docker volume rm $CARGO_VOL $TARGET_VOL" || true
    ;;
  *)
    sed -n '2,8p' "$0" >&2
    exit 2
    ;;
esac
