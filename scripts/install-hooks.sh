#!/usr/bin/env bash
#
# Points this clone's hooks at `.githooks/`, and installs the two cargo
# subcommands those hooks need.
#
# Run once per clone:
#
#   ./scripts/install-hooks.sh
#
# Git cannot ship hooks in a repository - `.git/hooks` is not tracked, and
# that is a security property, not an oversight: cloning a repository must not
# execute its code. `core.hooksPath` is the opt-in. This script is the opt-in
# being made once, deliberately, rather than a set of instructions in a README
# that half a team has followed.
#
# `core.hooksPath` is repository configuration, so it is shared by every
# worktree of this clone.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

git config core.hooksPath .githooks
echo "ok: core.hooksPath = .githooks"

# The hook fails rather than skips when these are missing, so installing them
# here is what keeps that from being the first thing a new clone sees.
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
