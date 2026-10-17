#!/usr/bin/env bash

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

git config core.hooksPath .githooks
echo "ok: core.hooksPath = .githooks"

for tool in deny audit; do
  if cargo "$tool" --version >/dev/null 2>&1; then
    echo "ok: cargo-${tool} already installed"
  else
    echo "installing cargo-${tool} (this takes a few minutes)"
    cargo install "cargo-${tool}" --locked
  fi
done

cat <<'EOF'

Done. `git commit` now runs the checks in scripts/checks.sh that match what is
staged. `git commit --no-verify` skips them; CI does not.

To undo: git config --unset core.hooksPath
EOF
