#!/usr/bin/env bash
#
# Run Aria's end-to-end SIP tests against a real PBX.
#
# What it exercises
# -----------------
# Two in-process SipManagers register on two extensions of the PBX and call
# each other through it: an inbound INVITE is received and answered, RTP flows
# both ways, DTMF is sent, the callee holds and resumes, the caller hangs up
# and the callee sees the BYE. The tests live in src-tauri/src/sip/pbx_e2e.rs
# and are #[ignore]d, so a normal `cargo test` never touches the network.
#
# This runs on a developer machine with audio devices and a route to the PBX,
# on demand. It is not for CI.
#
# Usage
# -----
#   scripts/pbx-e2e.sh [test-name-filter]
#
# With no filter every pbx_e2e test runs; pass e.g. `inbound_call` to run one.
# RUST_LOG defaults to info so the SIP/RTP engine lines show next to the
# test's own [A]/[B] progress lines. Each test prints both endpoints' full SIP
# traces when it fails.
#
# Two ways to supply the extensions
# ---------------------------------
# 1. Bring your own. Set all six and the script just runs the tests:
#
#      ARIA_E2E_PBX_DOMAIN   SIP AOR domain       (test.ariaroute.com)
#      ARIA_E2E_PBX_SERVER   registrar host[:port] (sip.ariaroute.com)
#      ARIA_E2E_EXT_A / ARIA_E2E_PASS_A   the extension that is called
#      ARIA_E2E_EXT_B / ARIA_E2E_PASS_B   the extension that calls
#      ARIA_E2E_EXT_C / ARIA_E2E_PASS_C   optional: a third, the target of
#                       `blind_transfer_through_the_pbx` (the PBX only
#                       transfers to a registered extension; without a
#                       third the test says so and passes)
#
#    Create the two extensions however you manage the PBX — the aria-pbx MCP
#    server's `create_extension` tool is one way (its `list_commands` /
#    `call_api extensions delete <id>` removes them afterwards).
#
# 2. Let the script create throwaway extensions over the PBX's REST API, run
#    the tests, and delete them again (also on failure or Ctrl-C):
#
#      ARIA_API_KEY     bearer token of a tenant admin (same variable the
#                       aria CLI and MCP server use)
#      ARIA_TENANT      tenant UUID the extensions are created in
#      ARIA_API_URL     API base; default https://api.ariaroute.com
#      ARIA_E2E_PBX_DOMAIN / ARIA_E2E_PBX_SERVER  as above; default
#                       test.ariaroute.com / sip.ariaroute.com
#      ARIA_E2E_EXT_A / _B / _C  extension numbers to create;
#                       default 1901 / 1902 / 1903
#
#    The routes are the ones in aria-cloud-pbx's openapi.yaml:
#      POST   /api/v1/tenants/{tenant}/extensions   {extension_number,
#                                                    display_name, password}
#      DELETE /api/v1/extensions/{id}
#    The password is chosen here, so nothing has to read it back.
#
# Optional knobs
# --------------
#   ARIA_E2E_SRTP_A / ARIA_E2E_SRTP_B   `sdes` or `disabled` (default) — what
#                       each endpoint offers. `sdes_offer_is_answered_by_the_pbx`
#                       forces sdes on both regardless.
#   ARIA_E2E_VOICEMAIL  what `outbound_call_to_voicemail` dials; default 1001,
#                       an extension with a mailbox and no phone, so its
#                       greeting answers (`*97` plays nothing for an
#                       extension without voicemail)
#
set -euo pipefail

cd "$(dirname "$0")/.."

FILTER="${1:-pbx_e2e}"
export RUST_LOG="${RUST_LOG:-info}"
export ARIA_E2E_PBX_DOMAIN="${ARIA_E2E_PBX_DOMAIN:-test.ariaroute.com}"
export ARIA_E2E_PBX_SERVER="${ARIA_E2E_PBX_SERVER:-sip.ariaroute.com}"

API_URL="${ARIA_API_URL:-https://api.ariaroute.com}"
CREATED_IDS=()

have_credentials() {
  [ -n "${ARIA_E2E_EXT_A:-}" ] && [ -n "${ARIA_E2E_PASS_A:-}" ] &&
    [ -n "${ARIA_E2E_EXT_B:-}" ] && [ -n "${ARIA_E2E_PASS_B:-}" ]
}

json_field() {
  # $1 = field; stdin = JSON object
  python3 -I -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"
}

# A password the PBX accepts (8+ chars, upper, lower, digit) that never
# touches the shell history or the process list of another user.
new_password() {
  printf 'E2e%sX9a' "$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 16)"
}

api() {
  # $1 = method, $2 = path, $3 = JSON body (optional)
  local method="$1" path="$2" body="${3:-}"
  local -a args=(-sS -f -X "$method" -H "Authorization: Bearer $ARIA_API_KEY")
  if [ -n "$body" ]; then
    args+=(-H 'Content-Type: application/json' --data "$body")
  fi
  curl "${args[@]}" "$API_URL$path"
}

create_extension() {
  # $1 = number, $2 = display name, $3 = password; prints the new id
  api POST "/api/v1/tenants/$ARIA_TENANT/extensions" \
    "{\"extension_number\":\"$1\",\"display_name\":\"$2\",\"password\":\"$3\"}" |
    json_field id
}

cleanup() {
  local id
  for id in "${CREATED_IDS[@]:-}"; do
    [ -n "$id" ] || continue
    if api DELETE "/api/v1/extensions/$id" >/dev/null; then
      echo "pbx-e2e: deleted extension $id"
    else
      echo "pbx-e2e: WARNING: could not delete extension $id; remove it by hand" >&2
    fi
  done
}

if ! have_credentials; then
  if [ -z "${ARIA_API_KEY:-}" ] || [ -z "${ARIA_TENANT:-}" ]; then
    cat >&2 <<EOF
pbx-e2e: set either the six ARIA_E2E_* variables (existing extensions) or
ARIA_API_KEY and ARIA_TENANT (the script creates and deletes two throwaway
extensions). See the comment at the top of $0.
EOF
    exit 2
  fi
  export ARIA_E2E_EXT_A="${ARIA_E2E_EXT_A:-1901}"
  export ARIA_E2E_EXT_B="${ARIA_E2E_EXT_B:-1902}"
  export ARIA_E2E_EXT_C="${ARIA_E2E_EXT_C:-1903}"
  ARIA_E2E_PASS_A="$(new_password)"
  ARIA_E2E_PASS_B="$(new_password)"
  ARIA_E2E_PASS_C="$(new_password)"
  export ARIA_E2E_PASS_A ARIA_E2E_PASS_B ARIA_E2E_PASS_C
  trap cleanup EXIT
  echo "pbx-e2e: creating extensions $ARIA_E2E_EXT_A, $ARIA_E2E_EXT_B and $ARIA_E2E_EXT_C in tenant $ARIA_TENANT"
  CREATED_IDS+=("$(create_extension "$ARIA_E2E_EXT_A" "E2E harness A" "$ARIA_E2E_PASS_A")")
  CREATED_IDS+=("$(create_extension "$ARIA_E2E_EXT_B" "E2E harness B" "$ARIA_E2E_PASS_B")")
  CREATED_IDS+=("$(create_extension "$ARIA_E2E_EXT_C" "E2E harness C" "$ARIA_E2E_PASS_C")")
  echo "pbx-e2e: created ${CREATED_IDS[*]}"
fi

echo "pbx-e2e: $ARIA_E2E_EXT_B calls $ARIA_E2E_EXT_A at $ARIA_E2E_PBX_DOMAIN via $ARIA_E2E_PBX_SERVER"
cd src-tauri
cargo test --lib -- --ignored --test-threads=1 --nocapture "$FILTER"
