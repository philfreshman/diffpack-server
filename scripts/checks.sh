#!/usr/bin/env bash

set -uo pipefail

readonly ALL_CHECKS=(fmt seams clippy deny audit test conformance)

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

require_commands() {
  local missing=()
  local command
  for command in "$@"; do
    command -v "$command" >/dev/null 2>&1 || missing+=("$command")
  done
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "error: the conformance check needs ${missing[*]} on PATH (Node 22.19 or later)." >&2
    return 1
  fi
}

run_fmt() {
  cargo fmt --all --check
}

run_seams() {
  local failed=0
  ./scripts/check-engine-seam.sh || failed=1
  ./scripts/check-tool-seams.sh || failed=1
  return "$failed"
}

run_clippy() {
  cargo clippy --all-targets -- -D warnings
}

run_deny() {
  require deny cargo-deny || return 1
  cargo deny --all-features check
}

run_audit() {
  require audit cargo-audit || return 1
  cargo audit --deny warnings
}

run_test() {
  cargo test
}

run_conformance() {
  require_commands node npm jq curl || return 1
  ./scripts/conformance.sh
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
      fmt | seams | clippy | deny | audit | test | conformance) ;;
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
