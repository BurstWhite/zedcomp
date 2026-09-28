#!/usr/bin/env bash
#
# e2e.sh — end-to-end smoke test for zedcomp-helper.
#
# What it does:
#   1. builds helper/ (cargo build --manifest-path helper/Cargo.toml)
#   2. starts the helper as a long-lived background job with
#      ZEDCOMP_WORKSPACE=<tempdir> ZEDCOMP_PORT=27121, keeping its stdin open so
#      the stdio LSP loop never sees EOF (a FIFO is held open read/write)
#   3. sends a minimal LSP "initialize" frame (rootUri + initializationOptions)
#      so a helper that only starts its listener after initialize also works
#   4. POSTs fixtures/cf.json, fixtures/ac.json, fixtures/luogu.json through
#      scripts/post_problem.sh
#   5. asserts each problem directory contains main.cpp, in1, ans1, problem.json
#   6. re-POSTs cf.json and asserts existing files were not clobbered
#   7. overwrites each main.cpp with a solution that matches the samples, runs
#      `zedcomp-helper judge <dir>` and requires "Test #1: AC" in its output
#   8. kills the helper and cleans up
#
# Usage:
#   scripts/e2e.sh
#
# Environment:
#   ZEDCOMP_PORT            port used for the helper + POSTs. Default: 27121
#   ZEDCOMP_HELPER_BIN      use this helper binary instead of building/locating one
#   ZEDCOMP_E2E_SKIP_BUILD  if 1, do not run cargo build (requires a built binary)
#   ZEDCOMP_E2E_KEEP        if 1, keep the temp workspace and logs on success
#   ZEDCOMP_E2E_STRICT_PORT if 1, fail instead of falling back to a free port
#   ZEDCOMP_E2E_NO_ZED_OPEN if 1, put a no-op `zed` shim first in PATH so the
#                           helper's auto-open does not pop up temp files
#   ZEDCOMP_CONFIG_DIR      config dir for template.cpp/cxxflags; defaults to an
#                           empty dir so the run does not depend on ~/.config
#
# Port conflicts: the default 27121 is also the port of the real CPH VS Code
# extension, so on a developer machine it is frequently already taken. When
# ZEDCOMP_PORT was not set explicitly the script then warns and falls back to
# the first free port in 27122..27126 instead of failing.
#
# Exit code: 0 when every step passed, non-zero otherwise.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${ZEDCOMP_PORT:-27121}"
PORT_WAS_SET=0
[ -n "${ZEDCOMP_PORT:-}" ] && PORT_WAS_SET=1
HOST="localhost"
SKIP_BUILD="${ZEDCOMP_E2E_SKIP_BUILD:-0}"

TMPBASE="${TMPDIR:-/tmp}"
TMPBASE="${TMPBASE%/}"

WS="$(mktemp -d "${TMPBASE}/zedcomp-e2e-ws.XXXXXX")" || exit 1
LOGDIR="$(mktemp -d "${TMPBASE}/zedcomp-e2e-logs.XXXXXX")" || exit 1
FIFO="${LOGDIR}/helper.stdin.fifo"
HELPER_PID=""
HELPER_BIN=""
PROBLEM_DIR=""

# Pin the config directory to an empty one unless the caller set it: without a
# template.cpp the generated main.cpp must be a 0-byte file (asserted below),
# and without a cxxflags file `judge` must use the built-in `-std=c++2a -O2 -Wall -Wextra`.
# Either assertion would otherwise depend on the developer's ~/.config/zedcomp.
if [ -z "${ZEDCOMP_CONFIG_DIR:-}" ]; then
  ZEDCOMP_CONFIG_DIR="${LOGDIR}/config"
  mkdir -p "$ZEDCOMP_CONFIG_DIR" || { echo "could not create $ZEDCOMP_CONFIG_DIR" >&2; exit 1; }
  export ZEDCOMP_CONFIG_DIR
fi

C_RESET=""; C_RED=""; C_GRN=""; C_YEL=""
if [ -t 1 ]; then
  C_RESET="$(printf '\033[0m')"; C_RED="$(printf '\033[31m')"
  C_GRN="$(printf '\033[32m')"; C_YEL="$(printf '\033[33m')"
fi
FAILURES=0

say()  { printf '%s\n' "$*"; }
ok()   { printf '%s  PASS%s %s\n' "$C_GRN" "$C_RESET" "$*"; }
warn() { printf '%s  WARN%s %s\n' "$C_YEL" "$C_RESET" "$*"; }
bad()  { printf '%s  FAIL%s %s\n' "$C_RED" "$C_RESET" "$*"; FAILURES=$((FAILURES + 1)); }
step() { printf '\n=== %s\n' "$*"; }
indent() { sed 's/^/      | /'; }

cleanup() {
  if [ -n "$HELPER_PID" ] && kill -0 "$HELPER_PID" 2>/dev/null; then
    kill "$HELPER_PID" 2>/dev/null
    for _ in 1 2 3 4 5; do
      kill -0 "$HELPER_PID" 2>/dev/null || break
      sleep 0.2
    done
    kill -9 "$HELPER_PID" 2>/dev/null
    wait "$HELPER_PID" 2>/dev/null
  fi
  # Release the FIFO write end so nothing keeps the helper's stdin alive.
  exec 9>&- 2>/dev/null
  if [ "$FAILURES" -eq 0 ] && [ "${ZEDCOMP_E2E_KEEP:-0}" != "1" ]; then
    rm -rf "$WS" "$LOGDIR"
  else
    say ""
    say "artifacts kept:"
    say "  workspace: $WS"
    say "  logs:      $LOGDIR (helper.out / helper.err)"
  fi
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------- helpers ----

# TCP-level readiness probe (no HTTP request, so the helper sees no traffic).
port_open() {
  (exec 3<>"/dev/tcp/127.0.0.1/${1:-$PORT}") 2>/dev/null
}

# First free port among the fallback candidates (used only when 27121 is taken).
find_free_port() {
  local p
  for p in 27122 27123 27124 27125 27126; do
    if ! port_open "$p"; then printf '%s\n' "$p"; return 0; fi
  done
  return 1
}

# 0 = listening, 1 = timed out, 2 = helper process died.
wait_for_port() {
  local tries="${1:-100}" i=0
  while [ "$i" -lt "$tries" ]; do
    if port_open; then
      return 0
    fi
    if [ -n "$HELPER_PID" ] && ! kill -0 "$HELPER_PID" 2>/dev/null; then
      return 2
    fi
    sleep 0.15
    i=$((i + 1))
  done
  return 1
}

# print the deepest directory under $1 containing a problem.json (empty if none)
find_problem_dir() {
  local root="$1" best="" f dir
  [ -d "$root" ] || return 1
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    dir="${f%/problem.json}"
    if [ -z "$best" ] || [ "${#dir}" -gt "${#best}" ]; then best="$dir"; fi
  done < <(find "$root" -type f -name problem.json 2>/dev/null)
  [ -n "$best" ] || return 1
  printf '%s\n' "$best"
}

# assert_problem <oj-root-name> <label> <accepted-relative-dir>...
# Accepts a list of documented candidate layouts; falls back to searching the
# OJ directory, warning if the result is not one of the candidates.
assert_problem() {
  local oj="$1" label="$2"; shift 2
  local candidates="$*"
  local dir="" cand="" in_list=0

  for cand in $candidates; do
    # A candidate only counts when it really is the leaf problem directory.
    if [ -f "${WS}/${cand}/problem.json" ]; then dir="${WS}/${cand}"; break; fi
  done
  if [ -z "$dir" ]; then
    dir="$(find_problem_dir "${WS}/${oj}")"
    if [ -n "$dir" ]; then
      for cand in $candidates; do
        if [ "$dir" = "${WS}/${cand}" ]; then in_list=1; break; fi
      done
      if [ "$in_list" != "1" ]; then
        warn "${label}: none of [${candidates}] matched; found ${dir#"${WS}/"} instead"
      fi
    fi
  fi

  if [ -z "$dir" ] || [ ! -d "$dir" ]; then
    bad "${label}: no problem directory found under ${WS}/${oj}/"
    say "      tree of ${WS}:"; (cd "$WS" && find . -maxdepth 5) 2>/dev/null | sort | indent
    return 1
  fi

  local missing=""
  for f in main.cpp in1 ans1 problem.json; do
    [ -e "${dir}/${f}" ] || missing="${missing} ${f}"
  done
  if [ -n "$missing" ]; then
    bad "${label}: ${dir#"${WS}/"} is missing:${missing}"
    say "      contents:"; ls -la "$dir" 2>/dev/null | indent
    return 1
  fi

  ok "${label}: ${dir#"${WS}/"}/{main.cpp,in1,ans1,problem.json}"
  PROBLEM_DIR="$dir"
  return 0
}

# judge_problem <dir> <label>
judge_problem() {
  local dir="$1" label="$2" out rc
  out="$("$HELPER_BIN" judge "$dir" 2>&1)"
  rc=$?
  printf '%s\n' "$out" | indent
  if printf '%s' "$out" | grep -q 'Test #1: AC'; then
    if [ "$rc" -eq 0 ]; then
      ok "${label}: judge reported Test #1: AC (exit 0)"
    else
      bad "${label}: judge reported Test #1: AC but exited ${rc} (expected 0 when all tests pass)"
    fi
  else
    bad "${label}: judge output did not contain \"Test #1: AC\""
    return 1
  fi
  return 0
}

# ------------------------------------------------------------ 1. build -------

step "build helper"
if [ -n "${ZEDCOMP_HELPER_BIN:-}" ]; then
  HELPER_BIN="$ZEDCOMP_HELPER_BIN"
  say "using ZEDCOMP_HELPER_BIN=${HELPER_BIN}"
elif [ "$SKIP_BUILD" = "1" ]; then
  say "ZEDCOMP_E2E_SKIP_BUILD=1, skipping cargo build"
else
  if [ ! -f "${REPO_ROOT}/helper/Cargo.toml" ]; then
    bad "helper/Cargo.toml not found — helper crate is missing at ${REPO_ROOT}/helper"
    exit 1
  fi
  if ! (cd "$REPO_ROOT" && cargo build --manifest-path helper/Cargo.toml); then
    bad "cargo build --manifest-path helper/Cargo.toml failed"
    exit 1
  fi
fi

if [ -z "$HELPER_BIN" ]; then
  for cand in "${REPO_ROOT}/helper/target/debug/zedcomp-helper" \
              "${REPO_ROOT}/helper/target/release/zedcomp-helper" \
              "${REPO_ROOT}/target/debug/zedcomp-helper" \
              "${REPO_ROOT}/target/release/zedcomp-helper"; do
    if [ -x "$cand" ]; then HELPER_BIN="$cand"; break; fi
  done
fi
if [ -z "$HELPER_BIN" ] || [ ! -x "$HELPER_BIN" ]; then
  bad "no zedcomp-helper binary found (build it, or set ZEDCOMP_HELPER_BIN)"
  exit 1
fi
ok "helper binary: ${HELPER_BIN}"

# --------------------------------------------------------- 2. start helper ---

step "start helper (ZEDCOMP_WORKSPACE=${WS}, ZEDCOMP_PORT=${PORT})"

if port_open "$PORT"; then
  if [ "$PORT_WAS_SET" = "1" ] || [ "${ZEDCOMP_E2E_STRICT_PORT:-0}" = "1" ]; then
    bad "port ${PORT} already accepts connections before we start; free it or set ZEDCOMP_PORT"
    exit 1
  fi
  FALLBACK="$(find_free_port)"
  if [ -z "$FALLBACK" ]; then
    bad "port ${PORT} is in use and no free fallback port was found in 27122..27126"
    exit 1
  fi
  warn "port ${PORT} is already in use (e.g. the real CPH VS Code extension on the same machine); using ${FALLBACK} instead."
  warn "set ZEDCOMP_PORT to pin a port, or ZEDCOMP_E2E_STRICT_PORT=1 to fail on conflicts."
  PORT="$FALLBACK"
else
  say "port ${PORT} is free"
fi

if ! mkfifo "$FIFO"; then
  bad "could not create FIFO at ${FIFO}"
  exit 1
fi
# A read-write open never blocks and never delivers EOF to the child, so the
# helper's stdio LSP loop stays alive for the whole test.
if ! exec 9<>"$FIFO"; then
  bad "could not open FIFO ${FIFO}"
  exit 1
fi

# The helper opens the generated main.cpp with the `zed` CLI. That is part of
# the contract, but on a machine with the CLI installed it pops up editor tabs
# for files in a temp workspace that is deleted moments later, so allow opting out.
if [ "${ZEDCOMP_E2E_NO_ZED_OPEN:-0}" = "1" ]; then
  SHIMDIR="${LOGDIR}/shim"
  mkdir -p "$SHIMDIR" || { bad "could not create PATH shim dir"; exit 1; }
  printf '#!/bin/sh\n# e2e shim: swallow `zed <path>` so no editor tabs open\nexit 0\n' > "${SHIMDIR}/zed"
  chmod +x "${SHIMDIR}/zed"
  PATH="${SHIMDIR}:${PATH}"
  export PATH
  say "PATH shim installed: zed auto-open suppressed (ZEDCOMP_E2E_NO_ZED_OPEN=1)"
fi

ZEDCOMP_WORKSPACE="$WS" ZEDCOMP_PORT="$PORT" \
  "$HELPER_BIN" <"$FIFO" >"${LOGDIR}/helper.out" 2>"${LOGDIR}/helper.err" &
HELPER_PID=$!
say "helper pid ${HELPER_PID} (logs: ${LOGDIR}/helper.{out,err})"

# Minimal LSP handshake: initialize with rootUri + initializationOptions.port.
# Best effort — the environment variables above are the primary contract.
INIT_JSON="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"processId\":$$,\"rootUri\":\"file://${WS}\",\"rootPath\":\"${WS}\",\"capabilities\":{},\"initializationOptions\":{\"port\":${PORT}}}}"
if printf 'Content-Length: %s\r\n\r\n%s' "${#INIT_JSON}" "$INIT_JSON" >&9 2>/dev/null; then
  say "sent LSP initialize frame"
else
  warn "could not write LSP initialize frame to helper stdin"
fi

wait_for_port 100
PORT_RC=$?
if [ "$PORT_RC" -eq 0 ]; then
  ok "helper is listening on 127.0.0.1:${PORT}"
elif [ "$PORT_RC" -eq 2 ]; then
  bad "helper exited before it started listening"
  say "      helper stderr:"; tail -20 "${LOGDIR}/helper.err" 2>/dev/null | indent
  exit 1
else
  bad "helper never accepted a connection on 127.0.0.1:${PORT} within ~15s"
  say "      helper stderr:"; tail -20 "${LOGDIR}/helper.err" 2>/dev/null | indent
  exit 1
fi

# ------------------------------------------------------------- 3. POSTs ------

step "POST fixtures"
for fx in cf ac luogu; do
  if ZEDCOMP_PORT="$PORT" ZEDCOMP_HOST="$HOST" \
     bash "${REPO_ROOT}/scripts/post_problem.sh" "${REPO_ROOT}/fixtures/${fx}.json"; then
    ok "POST fixtures/${fx}.json -> HTTP 200"
  else
    bad "POST fixtures/${fx}.json failed"
    say "      helper stderr:"; tail -20 "${LOGDIR}/helper.err" 2>/dev/null | indent
  fi
done

if [ "$FAILURES" -gt 0 ]; then
  say ""
  say "aborting before assertions: POST stage failed"
  exit 1
fi

# -------------------------------------------------------- 4. structure ------

step "assert generated structure"
CF_DIR=""; AC_DIR=""; LUOGU_DIR=""

if assert_problem cf cf "cf/118/A"; then CF_DIR="$PROBLEM_DIR"; fi
if assert_problem ac ac "ac/abc096/abc096_a"; then AC_DIR="$PROBLEM_DIR"; fi
if assert_problem luogu luogu "luogu/P1001" "luogu/P1001/P1001" "luogu/problem/P1001" "luogu/luogu/P1001"; then
  LUOGU_DIR="$PROBLEM_DIR"
fi

if [ "$FAILURES" -gt 0 ]; then
  say ""
  say "aborting before judge: structure assertions failed"
  say "full tree under ${WS}:"
  (cd "$WS" && find . -maxdepth 5) 2>/dev/null | sort | indent
  exit 1
fi

# ------------------------------------------------- 5. no-clobber check ------

step "assert the generated main.cpp is empty (no template configured)"
if [ -f "${CF_DIR}/main.cpp" ] && [ ! -s "${CF_DIR}/main.cpp" ]; then
  ok "generated main.cpp is a 0-byte file (empty default, no template configured)"
else
  bad "generated main.cpp should be empty without a template (${CF_DIR}/main.cpp, $(wc -c < "${CF_DIR}/main.cpp" 2>/dev/null || echo '?') bytes)"
fi

step "re-POST cf.json (existing main.cpp must not be overwritten)"
CF_MAIN_SUM="$(shasum -a 256 "${CF_DIR}/main.cpp" 2>/dev/null | awk '{print $1}')"

if ZEDCOMP_PORT="$PORT" ZEDCOMP_HOST="$HOST" \
   bash "${REPO_ROOT}/scripts/post_problem.sh" "${REPO_ROOT}/fixtures/cf.json" >/dev/null; then
  CF_MAIN_SUM2="$(shasum -a 256 "${CF_DIR}/main.cpp" 2>/dev/null | awk '{print $1}')"
  if [ -n "$CF_MAIN_SUM" ] && [ "$CF_MAIN_SUM" = "$CF_MAIN_SUM2" ]; then
    ok "main.cpp untouched by the second POST (same sha256)"
  else
    bad "main.cpp changed after re-POST (expected the template to be written only when absent)"
  fi
else
  bad "second POST of fixtures/cf.json failed"
fi

# ------------------------------------------------------------ 6. judge ------

step "write sample-passing solutions and run judge"

cat > "${CF_DIR}/main.cpp" <<'CPP'
// Codeforces 118A "String Task": lowercase, drop vowels, prefix consonants with '.'.
#include <cctype>
#include <iostream>
#include <string>

int main() {
    std::string s;
    std::cin >> s;
    const std::string vowels = "aoyeui";
    for (char c : s) {
        char l = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        if (vowels.find(l) != std::string::npos) continue;
        std::cout << '.' << l;
    }
    std::cout << '\n';
    return 0;
}
CPP
judge_problem "$CF_DIR" "cf judge"

cat > "${AC_DIR}/main.cpp" <<'CPP'
// AtCoder ABC096 A "Day of Takahashi": answer is a when a <= b, else a - 1.
#include <iostream>

int main() {
    int a = 0, b = 0;
    std::cin >> a >> b;
    std::cout << (a <= b ? a : a - 1) << '\n';
    return 0;
}
CPP
judge_problem "$AC_DIR" "ac judge"

cat > "${LUOGU_DIR}/main.cpp" <<'CPP'
// Luogu P1001 "A+B Problem".
#include <iostream>

int main() {
    long long a = 0, b = 0;
    if (std::cin >> a >> b) std::cout << a + b << '\n';
    return 0;
}
CPP
judge_problem "$LUOGU_DIR" "luogu judge"

# ------------------------------------------------------------ 7. result -----

step "result"
if [ "$FAILURES" -eq 0 ]; then
  say "${C_GRN}ALL E2E CHECKS PASSED${C_RESET}"
  exit 0
fi
say "${C_RED}${FAILURES} E2E CHECK(S) FAILED${C_RESET}"
say "helper stderr (tail):"
tail -30 "${LOGDIR}/helper.err" 2>/dev/null | indent
exit 1
