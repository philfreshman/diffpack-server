#!/usr/bin/env bash

set -uo pipefail

readonly TOOLS="src/tools"
readonly RESOURCES="src/resources"

readonly ALLOWED_ROOTS=(crate self super std core alloc futures rmcp serde serde_json schemars)

readonly ALLOWED_MODULES=(archive cache_key catalogue engine error handle page registry resources search store tools)

readonly FORBIDDEN='reqwest|hyper|ureq|isahc|std::net|tokio::net|vercel_blob|BlobStore|BLOB_READ_WRITE_TOKEN|BLOB_STORE_ID|VERCEL_OIDC_TOKEN|as_engine_map'

guarded=()
for directory in "$TOOLS" "$RESOURCES"; do
  [[ -d "$directory" ]] && guarded+=("$directory")
done

if [[ ${#guarded[@]} -eq 0 ]]; then
  echo "ok: nothing to guard yet (neither ${TOOLS}/ nor ${RESOURCES}/ exists)"
  exit 0
fi

offenders=()

contains() {
  local needle=$1 item
  shift
  for item in "$@"; do
    [[ "$item" == "$needle" ]] && return 0
  done
  return 1
}

while IFS= read -r hit; do
  [[ -z "$hit" ]] && continue

  where=${hit%%:*}
  rest=${hit#*:}
  line=${rest#*:}
  where="${where}:${rest%%:*}"

  [[ "${where%%:*}" == "${TOOLS}/mod.rs" ]] && continue

  path=${line#*use }
  path=${path%%;*}
  path=${path%%\{*}
  path=${path#"${path%%[![:space:]]*}"}
  path=${path#::}

  root=${path%%::*}
  root=${root%% *}

  if ! contains "$root" "${ALLOWED_ROOTS[@]}"; then
    offenders+=("${where}: imports \`${root}\`, which is not a tool's or a resource's to reach")
    continue
  fi

  [[ "$root" != "crate" ]] && continue

  modules=()
  after=${path#crate}
  after=${after#::}
  if [[ -n "$after" ]]; then
    modules+=("${after%%::*}")
  elif [[ "$line" == *'{'* ]]; then
    inner=${line#*\{}
    inner=${inner%%\}*}
    for item in ${inner//,/ }; do
      modules+=("${item%%::*}")
    done
  fi

  for module in "${modules[@]}"; do
    module=${module%% *}
    [[ -z "$module" ]] && continue
    if ! contains "$module" "${ALLOWED_MODULES[@]}"; then
      offenders+=("${where}: imports \`crate::${module}\`, which is not a seam a tool goes through")
    fi
  done
done < <(grep -rnE '^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]' --include='*.rs' "${guarded[@]}")

while IFS= read -r hit; do
  [[ -z "$hit" ]] && continue
  where=${hit%%:*}
  rest=${hit#*:}
  where="${where}:${rest%%:*}"
  name=$(grep -oE "$FORBIDDEN" <<<"${rest#*:}" | head -n 1)
  offenders+=("${where}: names \`${name}\`, which lives behind a seam")
done < <(grep -rnE "$FORBIDDEN" --include='*.rs' "${guarded[@]}")

if [[ ${#offenders[@]} -gt 0 ]]; then
  echo "error: a module reaches past its seams:" >&2
  printf '  %s\n' "${offenders[@]}" >&2
  cat >&2 <<EOF

A module under ${TOOLS}/ is one tool and a module under ${RESOURCES}/ is one
resource, and neither is anything else. Each gets a package's
files from \`crate::archive\`, asks \`crate::catalogue\` what a package has
released and \`crate::search\` which packages a registry has, asks
\`crate::registry\` what a registry is, caches through \`crate::store\`, names
a diff with \`crate::handle\`, stays inside the response ceiling with
\`crate::page\`, and fails through \`crate::error\`. The HTTP client and the
blob store are those modules' business, not a tool's: eight tools that each
know how to fetch is eight places to fix a timeout, a retry or a user agent.

docs/architecture.md has the map and docs/adr/ has the reasoning. If a tool
genuinely needs something this list does not have, the list is the thing to
change - deliberately, in this file, with the reason.
EOF
  exit 1
fi

echo "ok: no tool or resource module reaches past its seams"
