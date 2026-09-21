#!/usr/bin/env bash
#
# The checks that gate a commit, and the same ones CI runs on a pull request.
#
# They live here rather than being spelled out in both places so that the two
# cannot drift: `.githooks/pre-commit` and `.github/workflows/ci.yml` both
# invoke this script, so a flag added here is a flag added to both. A check
# that is stricter locally than in CI wastes people's time; a check that is
# looser lets a red pipeline be a surprise.
#
# Usage, from the repository root:
#
#   ./scripts/checks.sh                  # fmt, clippy, deny, audit, test
#   ./scripts/checks.sh clippy test      # just those, in the order given
#
# Each check is independent, and the script runs every one it was asked for
# before exiting non-zero: one commit attempt should tell you everything that
# is wrong, not the first thing.

set -uo pipefail

readonly ALL_CHECKS=(fmt clippy deny audit test)

# `cargo deny` and `cargo audit` are not part of a Rust toolchain, so a fresh
# clone does not have them. Missing tools are a hard failure rather than a
# skip: a check that quietly does nothing when its tool is absent is worse
# than no check, because it reports success.
require() {
  local tool=$1 install=$2
  if ! cargo "$tool" --version >/dev/null 2>&1; then
    cat >&2 <<EOF
error: cargo-${tool} is not installed, so the ${tool} check cannot run.

  cargo install ${install} --locked

EOF
    return 1
  fi
}

run_fmt() {
  # First because it is the only check that compiles nothing: a misplaced
  # brace is reported in a second rather than after the whole dependency
  # graph has been built. `--all` covers `api/` and `tests/` as well as
  # `src/`.
  #
  # When this fails: `cargo fmt --all`, and commit.
  cargo fmt --all --check
}

run_clippy() {
  # `--all-targets` so the binary, the tests and any example are linted, not
  # just the library, and `-D warnings` so a lint is a failure rather than
  # something that scrolls past.
  cargo clippy --all-targets -- -D warnings
}

run_deny() {
  require deny cargo-deny || return 1
  # Every section of `deny.toml`: advisories, licenses, bans and sources.
  # `--all-features` because a dependency pulled in only by a feature is still
  # a dependency that ships.
  cargo deny --all-features check
}

run_audit() {
  require audit cargo-audit || return 1
  # Overlaps `cargo deny check advisories` on purpose. They read the same
  # RustSec database but not the same way: deny is configured by `deny.toml`
  # and is where an advisory gets ignored with a reason, audit is unconfigured
  # and is the second opinion that notices when an ignore in `deny.toml` has
  # outlived its justification. If these two disagree, the disagreement is the
  # finding.
  cargo audit --deny warnings
}

run_test() {
  # The whole suite, not `--lib`: `tests/` holds the cache-key vectors, the
  # engine-version agreement and the /health body, which are the checks with
  # contracts behind them.
  cargo test
}

main() {
  local checks=("$@")
  if [[ ${#checks[@]} -eq 0 ]]; then
    checks=("${ALL_CHECKS[@]}")
  fi

  local failed=()
  local check
  for check in "${checks[@]}"; do
    case "$check" in
      fmt | clippy | deny | audit | test) ;;
      *)
        echo "error: unknown check '${check}' (expected one of: ${ALL_CHECKS[*]})" >&2
        exit 2
        ;;
    esac

    echo "==> ${check}"
    if ! "run_${check}"; then
      failed+=("$check")
    fi
    echo
  done

  if [[ ${#failed[@]} -gt 0 ]]; then
    echo "failed: ${failed[*]}" >&2
    exit 1
  fi

  echo "ok: ${checks[*]}"
}

main "$@"
