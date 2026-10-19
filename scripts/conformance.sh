#!/usr/bin/env bash

set -uo pipefail

readonly TOOLS_DIR="conformance"
readonly INSPECTOR="${TOOLS_DIR}/node_modules/.bin/mcp-inspector"
readonly CALLS="${TOOLS_DIR}/calls.json"
readonly HANDLE_PLACEHOLDER="<handle>"
readonly FIXTURES="fixtures"
readonly STARTUP_SECONDS=30

readonly ERAS=(modern legacy)
readonly CURRENT="2026-07-28"
readonly PREVIOUS="2025-11-25"
readonly REGISTRIES_RESOURCE="diffpack://registries"
readonly DIFF_TEMPLATES='["diff", "file-diff"]'
readonly PROMPT="review_upgrade"
readonly INVALID_PARAMS=-32602
readonly FOREIGN_ORIGIN="https://not-an-allowed-origin.example"

server=""
server_pid=""
server_log=""
endpoint=""
failures=()

install_tools() {
  npm ci --prefix "$TOOLS_DIR" --engine-strict --ignore-scripts --no-audit --no-fund --loglevel=error
}

built_server() {
  cargo build --quiet --example serve --message-format=json-render-diagnostics |
    jq -er 'select(.reason == "compiler-artifact" and .target.name == "serve") | .executable'
}

start_server() {
  server_log=$(mktemp)
  "$server" "$FIXTURES" >"$server_log" 2>&1 &
  server_pid=$!

  local waited=0
  until endpoint=$(grep -m1 '^http://' "$server_log"); do
    if ! kill -0 "$server_pid" 2>/dev/null || ((waited >= STARTUP_SECONDS)); then
      echo "error: the server did not start" >&2
      cat "$server_log" >&2
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  endpoint="${endpoint}/mcp"
}

stop_server() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null
    wait "$server_pid" 2>/dev/null
  fi
  [[ -n "$server_log" ]] && rm -f "$server_log"
}

revision_of() {
  case "$1" in
    modern) echo "$CURRENT" ;;
    legacy) echo "$PREVIOUS" ;;
  esac
}

inspect() {
  local era=$1
  shift
  "$INSPECTOR" --cli "$endpoint" --transport http \
    --protocol-era "$era" --format json "$@"
}

check() {
  local name=$1
  shift
  local output
  if output=$("$@" 2>&1); then
    echo "  ok    ${name}"
  else
    echo "  FAIL  ${name}"
    sed 's/^/        /' <<<"$output"
    failures+=("$name")
  fi
}

is_present() {
  [[ -n "$1" ]]
}

negotiates_its_revision() {
  local era=$1
  local expected negotiated
  expected=$(revision_of "$era")
  negotiated=$(inspect "$era" --method initialize | jq -r '.result.protocolVersion') || return 1
  if [[ "$negotiated" != "$expected" ]]; then
    echo "expected ${expected}, negotiated ${negotiated}"
    return 1
  fi
}

lists_tools_with_portable_schemas() {
  local era=$1
  inspect "$era" --method tools/list --strict >/dev/null
}

lists_tools_whose_schemas_are_json_schema_2020_12() {
  local era=$1
  inspect "$era" --method tools/list | node "${TOOLS_DIR}/schemas.mjs"
}

listed_tools() {
  local era=$1
  inspect "$era" --method tools/list 2>/dev/null | jq -er '.result.tools[].name'
}

example_arguments() {
  local tool=$1 handle=$2
  jq -ec --arg tool "$tool" --arg handle "$handle" --arg placeholder "$HANDLE_PLACEHOLDER" \
    '.[$tool] // empty | walk(if . == $placeholder then $handle else . end)' "$CALLS"
}

call_tool() {
  local era=$1 tool=$2 arguments=$3
  inspect "$era" --method tools/call --tool-name "$tool" --tool-args-json "$arguments"
}

minted_handle() {
  local era=$1
  call_tool "$era" diff_package_versions "$(example_arguments diff_package_versions "")" |
    jq -er '.result.structuredContent.handle'
}

answers_its_example_call() {
  local era=$1 tool=$2 handle=$3
  local arguments answer
  if ! arguments=$(example_arguments "$tool" "$handle"); then
    echo "${CALLS} has no example call for ${tool}, so its answers go unchecked"
    return 1
  fi
  answer=$(call_tool "$era" "$tool" "$arguments") || {
    echo "$answer"
    return 1
  }
  jq -e '.result.isError != true' <<<"$answer" >/dev/null || {
    echo "$answer"
    return 1
  }
}

lists_the_registries_resource() {
  local era=$1
  inspect "$era" --method resources/list |
    jq -e --arg uri "$REGISTRIES_RESOURCE" 'any(.result.resources[]; .uri == $uri)' >/dev/null
}

lists_both_diff_templates() {
  local era=$1
  inspect "$era" --method resources/templates/list |
    jq -e --argjson names "$DIFF_TEMPLATES" '[.result.resourceTemplates[].name] | sort == $names' >/dev/null
}

lists_the_prompt() {
  local era=$1
  inspect "$era" --method prompts/list |
    jq -e --arg name "$PROMPT" 'any(.result.prompts[]; .name == $name)' >/dev/null
}

request_body() {
  local revision=$1 method=$2 params=$3
  jq -nc --arg revision "$revision" --arg current "$CURRENT" --arg method "$method" \
    --argjson params "$params" '
    {jsonrpc: "2.0", id: 1, method: $method, params: $params}
    | if $revision == $current then
        .params._meta = {
          "io.modelcontextprotocol/protocolVersion": $revision,
          "io.modelcontextprotocol/clientCapabilities": {}
        }
      else . end'
}

post() {
  local revision=$1 method=$2 params=$3
  shift 3
  local headers=(
    -H "accept: application/json, text/event-stream"
    -H "content-type: application/json"
    -H "mcp-protocol-version: ${revision}"
  )
  if [[ "$revision" == "$CURRENT" ]]; then
    headers+=(-H "mcp-method: ${method}")
    local name
    name=$(jq -r '.name // .uri // empty' <<<"$params")
    [[ -n "$name" ]] && headers+=(-H "mcp-name: ${name}")
  fi
  curl -sS -X POST "$endpoint" "${headers[@]}" \
    --data "$(request_body "$revision" "$method" "$params")" "$@"
}

discover_advertises_both_revisions_and_tools() {
  post "$CURRENT" server/discover '{}' |
    jq -e --arg current "$CURRENT" --arg previous "$PREVIOUS" '
      .result
      | (.supportedVersions | index($current) != null and index($previous) != null)
        and (.capabilities.tools | type == "object")
        and ._meta["io.modelcontextprotocol/serverInfo"].name == "diffpack"'
}

refuses_invalid_arguments_as_invalid_params() {
  local revision=$1
  local arguments
  for arguments in \
    '{"registry": "maven", "package": "junit", "version": "4.13.2"}' \
    '{"registry": "npm", "package": "zod"}' \
    '{"registry": "npm", "package": "zod", "version": 4}'; do
    post "$revision" tools/call "{\"name\": \"resolve_archive_url\", \"arguments\": ${arguments}}" |
      jq -e --argjson code "$INVALID_PARAMS" '.error.code == $code' || {
      echo "arguments ${arguments} were not refused with ${INVALID_PARAMS}"
      return 1
    }
  done
}

answers_get_and_delete_with_405_allowing_post() {
  local method headers
  for method in GET DELETE; do
    headers=$(curl -sS -o /dev/null -D - -X "$method" "$endpoint" \
      -H "accept: application/json, text/event-stream")
    grep -q '^HTTP/[0-9.]* 405' <<<"$headers" && grep -qi '^allow: POST' <<<"$headers" || {
      echo "${method}:"
      echo "$headers"
      return 1
    }
  done
}

forbids_a_foreign_origin() {
  local status
  status=$(post "$CURRENT" tools/list '{}' -o /dev/null -w '%{http_code}' -H "origin: ${FOREIGN_ORIGIN}")
  [[ "$status" == 403 ]] || {
    echo "Origin ${FOREIGN_ORIGIN} answered ${status}"
    return 1
  }
}

mints_no_session_id() {
  local initialize headers
  initialize=$(jq -nc --arg revision "$PREVIOUS" \
    '{protocolVersion: $revision, capabilities: {}, clientInfo: {name: "conformance", version: "0"}}')
  for headers in \
    "$(post "$PREVIOUS" initialize "$initialize" -o /dev/null -D -)" \
    "$(post "$PREVIOUS" tools/list '{}' -o /dev/null -D -)" \
    "$(post "$CURRENT" tools/list '{}' -o /dev/null -D -)"; do
    if ! grep -q '^HTTP/[0-9.]* 200' <<<"$headers" || grep -qi '^mcp-session-id:' <<<"$headers"; then
      echo "$headers"
      return 1
    fi
  done
}

check_transport() {
  check "server/discover advertises both revisions and the tools capability" \
    discover_advertises_both_revisions_and_tools
  check "a ${CURRENT} client's invalid arguments are refused as ${INVALID_PARAMS}" \
    refuses_invalid_arguments_as_invalid_params "$CURRENT"
  check "a ${PREVIOUS} client's invalid arguments are refused as ${INVALID_PARAMS}" \
    refuses_invalid_arguments_as_invalid_params "$PREVIOUS"
  check "GET and DELETE are 405 and say POST is allowed" answers_get_and_delete_with_405_allowing_post
  check "a foreign Origin is 403" forbids_a_foreign_origin
  check "no answer mints a session id" mints_no_session_id
}

check_era() {
  local era=$1
  local client
  client="a $(revision_of "$era") client"

  check "${client} negotiates its revision" negotiates_its_revision "$era"
  check "${client} lists tools whose schemas are portable" \
    lists_tools_with_portable_schemas "$era"
  check "${client} lists tools whose schemas are JSON Schema 2020-12" \
    lists_tools_whose_schemas_are_json_schema_2020_12 "$era"

  check "${client} lists the registries resource" lists_the_registries_resource "$era"
  check "${client} lists both diff templates" lists_both_diff_templates "$era"
  check "${client} lists the ${PROMPT} prompt" lists_the_prompt "$era"

  local handle tools tool
  handle=$(minted_handle "$era")
  tools=$(listed_tools "$era")
  check "${client} lists the tools it is about to call" is_present "$tools"
  for tool in $tools; do
    check "${client} calls ${tool} and gets the shape it declares" \
      answers_its_example_call "$era" "$tool" "$handle"
  done
}

main() {
  install_tools || exit 1
  server=$(built_server) || exit 1
  trap stop_server EXIT
  start_server || exit 1
  echo "conformance against ${endpoint}"

  check_transport
  local era
  for era in "${ERAS[@]}"; do
    check_era "$era"
  done

  if [[ ${#failures[@]} -gt 0 ]]; then
    echo "failed: ${#failures[@]}" >&2
    exit 1
  fi
  echo "ok: conformance"
}

main "$@"
