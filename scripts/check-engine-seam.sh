#!/usr/bin/env bash
#
# `src/engine.rs` is the only module allowed to name `diffpack_engine`.
#
# The rule is worth enforcing rather than documenting because the failure it
# prevents is silent: a second importer costs nothing today and turns the next
# engine bump into an archaeology exercise across the whole crate. A convention
# holds until someone is in a hurry.
#
# Run from the repository root. Exits non-zero, naming every offender, if any
# file under `src/` or `api/` other than `src/engine.rs` mentions the crate.

set -euo pipefail

readonly SEAM="src/engine.rs"

offenders=$(
  grep -rn --include='*.rs' -w 'diffpack_engine' src api |
    grep -v "^${SEAM}:" ||
    true
)

if [[ -n "$offenders" ]]; then
  echo "error: diffpack_engine is imported outside ${SEAM}:" >&2
  echo "$offenders" >&2
  cat >&2 <<EOF

Everything the server needs from the engine goes through ${SEAM}, so that a
bump to a new engine release is one file to change and one file to read.
Re-export what you need there and import it from \`crate::engine\`.
EOF
  exit 1
fi

echo "ok: diffpack_engine is imported only in ${SEAM}"
