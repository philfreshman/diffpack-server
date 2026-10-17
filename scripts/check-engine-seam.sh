#!/usr/bin/env bash

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
