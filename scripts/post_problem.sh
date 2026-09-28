#!/usr/bin/env bash
#
# post_problem.sh — POST a Competitive Companion problem fixture to a running
# zedcomp-helper, exactly the way the Competitive Companion browser extension
# would.
#
# Usage:
#   scripts/post_problem.sh [fixture.json]
#
# Arguments:
#   fixture.json   Path to a CC "Problem" JSON body. Default: fixtures/cf.json
#                  (resolved relative to this repo when the path is relative).
#
# Environment:
#   ZEDCOMP_PORT   Port of the helper's HTTP listener. Default: 27121
#   ZEDCOMP_HOST   Host of the helper's HTTP listener. Default: localhost
#
# Exit code:
#   0  the helper answered with HTTP 200
#   non-zero otherwise (curl failure, or a non-200 status)
#
# Notes:
#   * The fixtures in fixtures/ are already unwrapped: they contain the Problem
#     object itself (the `.result` field of the raw Competitive Companion test
#     data), so the file is sent verbatim as the POST body.
#   * Competitive Companion POSTs to the bare path "/", and so do we.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="${1:-fixtures/cf.json}"
HOST="${ZEDCOMP_HOST:-localhost}"
PORT="${ZEDCOMP_PORT:-27121}"
URL="http://${HOST}:${PORT}/"

# Allow repo-relative paths regardless of the caller's cwd.
if [ ! -f "$FIXTURE" ] && [ -f "${REPO_ROOT}/${FIXTURE}" ]; then
  FIXTURE="${REPO_ROOT}/${FIXTURE}"
fi

if [ ! -f "$FIXTURE" ]; then
  echo "post_problem.sh: fixture not found: $FIXTURE" >&2
  exit 2
fi
case "$FIXTURE" in
  /*) ;;
  *) FIXTURE="${PWD}/${FIXTURE}" ;;
esac

# --fail would hide the status code we want to report, so capture stdout instead.
# curl's own diagnostics (-sS) go straight to this script's stderr.
set +e
BODY="$(curl -sS --max-time 30 -X POST \
  -H 'Content-Type: application/json' \
  --data-binary "@${FIXTURE}" \
  -w '\n%{http_code}' \
  "${URL}")"
CURL_RC=$?
set -e

if [ "$CURL_RC" -ne 0 ]; then
  echo "post_problem.sh: curl failed (exit ${CURL_RC}) posting ${FIXTURE} to ${URL}" >&2
  exit "$CURL_RC"
fi

STATUS="$(printf '%s' "$BODY" | tail -n1)"
PAYLOAD="$(printf '%s' "$BODY" | sed '$d')"

echo "POST ${FIXTURE} -> ${URL} [${STATUS}]"
if [ -n "$PAYLOAD" ]; then
  echo "  body: ${PAYLOAD}"
fi

if [ "$STATUS" != "200" ]; then
  echo "post_problem.sh: expected HTTP 200, got ${STATUS}" >&2
  exit 1
fi
exit 0
