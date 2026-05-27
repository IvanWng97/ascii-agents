#!/usr/bin/env bash
# Mirror of .github/workflows/ci.yml — run before pushing to avoid the
# round-trip of "push → wait for CI → red → fix → push again".
#
# If any check fails, exit non-zero so the pre-push hook blocks the push.
# Run manually with: ./scripts/preflight.sh
# Skip in an emergency with: SKIP_PREFLIGHT=1 git push  (not recommended)
set -euo pipefail

# Resolve repo root regardless of CWD when invoked from a hook.
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [[ "${SKIP_PREFLIGHT:-0}" == "1" ]]; then
    printf '\033[33m[preflight] SKIP_PREFLIGHT=1 — skipping checks\033[0m\n' >&2
    exit 0
fi

step() { printf '\033[36m[preflight] %s\033[0m\n' "$*" >&2; }
fail() { printf '\033[31m[preflight] FAILED: %s\033[0m\n' "$*" >&2; exit 1; }

# --- Phase 1: fast, independent lint checks (parallel) --------------------

step 'phase 1: fmt + machete + deny (parallel)'

TMPDIR_PF="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_PF"' EXIT

run_check() {
    local name="$1"; shift
    if "$@" > "$TMPDIR_PF/$name.log" 2>&1; then
        printf '\033[32m  ✓ %s\033[0m\n' "$name" >&2
    else
        printf '\033[31m  ✗ %s\033[0m\n' "$name" >&2
        cat "$TMPDIR_PF/$name.log" >&2
        return 1
    fi
}

pids=()
run_check fmt     cargo fmt --all --check & pids+=($!)
run_check machete cargo machete &           pids+=($!)
run_check deny    cargo deny check &        pids+=($!)

FAIL=0
for pid in "${pids[@]}"; do
    wait "$pid" || FAIL=1
done
[[ $FAIL -eq 0 ]] || fail 'phase 1 lint checks (see above)'

# --- Phase 2: clippy (needs compile, runs before tests) -------------------

step 'cargo clippy --workspace --all-targets --features ascii-agents-core/test-renderer -- -D warnings'
cargo clippy --workspace --all-targets \
    --features ascii-agents-core/test-renderer \
    -- -D warnings \
    || fail 'clippy: fix the warnings above and recommit'

# --- Phase 3: tests (parallel via nextest if available) -------------------

if command -v cargo-nextest &>/dev/null; then
    step 'cargo nextest run --workspace --features ascii-agents-core/test-renderer'
    cargo nextest run --workspace \
        --features ascii-agents-core/test-renderer \
        || fail 'tests: fix the failing tests above and recommit'
else
    step 'cargo test --workspace --features ascii-agents-core/test-renderer'
    cargo test --workspace \
        --features ascii-agents-core/test-renderer \
        || fail 'tests: fix the failing tests above and recommit'
fi

# Stamp so pre-push can skip redundant re-run (touch-based, not SHA-based,
# because during pre-commit the final commit SHA doesn't exist yet).
STAMP_DIR="${REPO_ROOT}/target/.preflight"
mkdir -p "$STAMP_DIR"
touch "$STAMP_DIR/passed"

printf '\033[32m[preflight] all checks passed\033[0m\n' >&2
