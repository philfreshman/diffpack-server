#!/usr/bin/env bash
#
# A tool module goes through the seams, not around them.
#
# Every module under `src/tools/` is one MCP tool: its definition and its
# handler, together (ADR 0002). What it may reach for is deliberately small —
# `archive` to get a package's files, `registry` to know what a registry is,
# `store` to cache a result, `page` to stay inside the response ceiling,
# `error` to fail on the right channel. Everything else is somebody else's
# job.
#
# The rule is worth enforcing rather than documenting because there will be
# eight of these modules and they will be written months apart. The first one
# that reaches for an HTTP client directly is not a bug — it works — and it is
# also the moment the archive seam stops being a seam, because the next seven
# copy the file that already exists. A convention holds until someone is in a
# hurry; this is the thing that is still there when they are.
#
# Two rules, because they catch different mistakes:
#
#   1. An allow-list over `use`. A tool may import the standard library, the
#      MCP and serialisation crates, and the seam modules. Anything else
#      fails, including a crate nobody has added to this repository yet —
#      which is the point, since the client this is meant to keep out (#20's
#      Blob client) does not exist yet and will not be called what this script
#      guesses.
#   2. A deny-list over the whole file, for the names that mean a seam was
#      crossed even when there is no `use` to catch: `reqwest::get(..)` spelled
#      out in full, a blob token read from the environment.
#
# The deny-list is a backstop, not the defence. The defence is privacy: #20's
# Blob client is a private module inside `src/store/`, so a tool cannot name
# it and the compiler says so. This script is what notices when someone makes
# it `pub` to get at it in a hurry.
#
# Mentions in a comment count, because a grep cannot tell a comment from code
# and a name written in a comment is a name someone can move into one. Write
# around it: reference the issue or the ADR rather than the crate.
#
# Run from the repository root. Exits non-zero, naming every offender.

set -uo pipefail

readonly TOOLS="src/tools"

# What a tool module may import. Not a list of what is convenient: a list of
# what a tool's job needs. Adding to it is a decision about the shape of the
# crate, which is why it is one line in a checked-in file rather than an
# import someone adds on a Friday.
#
# `futures` is here for one thing: a tool comparing two versions waits on the
# network twice and the two waits do not depend on each other. Joining them is
# that tool's own business rather than a seam's, because `archive` fetches one
# version and cannot know it is half of a pair. It is on the list rather than
# written as a full path at a call site, which is the spelling this rule
# cannot see.
readonly ALLOWED_ROOTS=(crate self super std core alloc futures rmcp serde serde_json schemars)

# The crate's own modules a tool may reach. The seams, plus `error` because
# every handler ends in one, `handle` because minting one is how a diff-taking
# tool answers at all, and `cache_key` because a tool may still need the key a
# handle names.
readonly ALLOWED_MODULES=(archive cache_key engine error handle page registry store tools)

# Names that mean a seam was crossed, wherever they appear.
readonly FORBIDDEN='reqwest|hyper|ureq|isahc|std::net|tokio::net|vercel_blob|BlobStore|BLOB_READ_WRITE_TOKEN'

if [[ ! -d "$TOOLS" ]]; then
  echo "ok: no tool modules yet (${TOOLS}/ does not exist)"
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

# Rule 1: every `use` in a tool module, held to the allow-list.
while IFS= read -r hit; do
  [[ -z "$hit" ]] && continue

  where=${hit%%:*}
  rest=${hit#*:}
  line=${rest#*:}
  where="${where}:${rest%%:*}"

  path=${line#*use }
  path=${path%%;*}
  path=${path%%\{*}
  path=${path#"${path%%[![:space:]]*}"}
  path=${path#::}

  root=${path%%::*}
  root=${root%% *}

  if ! contains "$root" "${ALLOWED_ROOTS[@]}"; then
    offenders+=("${where}: imports \`${root}\`, which is not a tool's to reach")
    continue
  fi

  [[ "$root" != "crate" ]] && continue

  # `use crate::...`: which module, and is it one of ours to use? A braced
  # group (`use crate::{error, page};`) names several at once, so each is
  # checked rather than the group being waved through.
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
done < <(grep -rnE '^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]' --include='*.rs' "$TOOLS")

# Rule 2: the names, wherever they are written.
while IFS= read -r hit; do
  [[ -z "$hit" ]] && continue
  where=${hit%%:*}
  rest=${hit#*:}
  where="${where}:${rest%%:*}"
  name=$(grep -oE "$FORBIDDEN" <<<"${rest#*:}" | head -n 1)
  offenders+=("${where}: names \`${name}\`, which lives behind a seam")
done < <(grep -rnE "$FORBIDDEN" --include='*.rs' "$TOOLS")

if [[ ${#offenders[@]} -gt 0 ]]; then
  echo "error: a tool module reaches past its seams:" >&2
  printf '  %s\n' "${offenders[@]}" >&2
  cat >&2 <<EOF

A module under ${TOOLS}/ is one tool and nothing else. It gets a package's
files from \`crate::archive\`, asks \`crate::registry\` what a registry is,
caches through \`crate::store\`, names a diff with \`crate::handle\`, stays
inside the response ceiling with \`crate::page\`, and fails through
\`crate::error\`. The HTTP client and the
blob store are those modules' business, not a tool's: eight tools that each
know how to fetch is eight places to fix a timeout, a retry or a user agent.

docs/architecture.md has the map and docs/adr/ has the reasoning. If a tool
genuinely needs something this list does not have, the list is the thing to
change - deliberately, in this file, with the reason.
EOF
  exit 1
fi

echo "ok: no tool module reaches past its seams"
