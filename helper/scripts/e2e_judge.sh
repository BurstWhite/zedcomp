#!/usr/bin/env bash
# End-to-end test for `zedcomp-helper judge` (uses clang++ since this machine has
# no GCC / libstdc++ bits/stdc++.h).
set -u
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
export ZEDCOMP_CXX="${ZEDCOMP_CXX:-clang++}"
# The flag cases below set these explicitly; clear any ambient value from the
# developer's shell so the earlier cases really exercise the built-in default.
unset ZEDCOMP_CXXFLAGS ZEDCOMP_CONFIG_DIR

BIN="${1:?usage: e2e_judge.sh <helper-binary> <scratch-dir>}"
SCRATCH="${2:?usage: e2e_judge.sh <helper-binary> <scratch-dir>}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FAILED=0
pass() { echo "  ok   $1"; }
fail() { echo "  FAIL $1"; FAILED=$((FAILED + 1)); }
check() { if [ "$1" = "$2" ]; then pass "$3 (=$1)"; else fail "$3 (want $2, got $1)"; fi; }

rm -rf "$SCRATCH"; mkdir -p "$SCRATCH"

# ---------------------------------------------------------------- AC / WA case
DIR="$SCRATCH/mixed"
mkdir -p "$DIR"
cat > "$DIR/main.cpp" <<'CPP'
#include <iostream>
int main() {
    long long a, b;
    if (!(std::cin >> a >> b)) { std::cerr << "no input\n"; return 2; }
    std::cout << a + b << std::endl;
    return 0;
}
CPP
printf '1 2\n'      > "$DIR/in1"
printf '3   \n'     > "$DIR/ans1"     # trailing whitespace: still AC
printf '100 200\n'  > "$DIR/in2"
printf '300'        > "$DIR/ans2"     # no trailing newline: still AC
printf '1 1\n'      > "$DIR/in3"
printf '3\n'        > "$DIR/ans3"     # wrong on purpose -> WA
printf '{"timeLimit":2000,"memoryLimit":256,"name":"x"}\n' > "$DIR/problem.json"

echo "judge: AC + WA"
OUT="$("$BIN" judge "$DIR" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "exit code is 1 when a test fails"
echo "$OUT" | grep -q "^Test #1: AC ([0-9]*ms)$" && pass "test 1 AC with ms" || fail "test 1 AC with ms"
echo "$OUT" | grep -q "^Test #2: AC ([0-9]*ms)$" && pass "test 2 AC (no trailing newline)" || fail "test 2 AC"
echo "$OUT" | grep -q "^Test #3: WA$" && pass "test 3 WA" || fail "test 3 WA"
echo "$OUT" | grep -q "expected: 3" && pass "WA shows expected line" || fail "WA expected detail"
[ -f "$DIR/.main" ] && pass ".main binary kept" || fail ".main binary kept"

# ---------------------------------------------------------------- all AC case
DIR2="$SCRATCH/allac"
mkdir -p "$DIR2"
cp "$DIR/main.cpp" "$DIR2/main.cpp"
printf '2 3\n' > "$DIR2/in1"; printf '5\n' > "$DIR2/ans1"
printf '{"timeLimit":5000}' > "$DIR2/problem.json"
echo "judge: all AC (default time limit 2000 when problem.json is minimal)"
OUT="$("$BIN" judge "$DIR2" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "0" "exit code is 0 when all tests pass"
echo "$OUT" | grep -q "1/1 test(s) passed" && pass "summary line" || fail "summary line"

# ---------------------------------------------------------------- TLE
DIR3="$SCRATCH/tle"
mkdir -p "$DIR3"
cat > "$DIR3/main.cpp" <<'CPP'
int main() { volatile long long x = 0; while (true) { x++; } }
CPP
printf 'x\n' > "$DIR3/in1"; printf 'y\n' > "$DIR3/ans1"
printf '{"timeLimit":400}' > "$DIR3/problem.json"
echo "judge: TLE (timeLimit 400 ms)"
START=$(date +%s%N 2>/dev/null || date +%s)
OUT="$("$BIN" judge "$DIR3" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "exit code is 1 on TLE"
echo "$OUT" | grep -q "^Test #1: TLE$" && pass "TLE verdict" || fail "TLE verdict"

# ---------------------------------------------------------------- RE
DIR4="$SCRATCH/re"
mkdir -p "$DIR4"
cat > "$DIR4/main.cpp" <<'CPP'
#include <cstdlib>
#include <iostream>
int main() { std::cerr << "boom\n"; std::abort(); }
CPP
printf 'x\n' > "$DIR4/in1"; printf 'y\n' > "$DIR4/ans1"
printf '{"timeLimit":2000}' > "$DIR4/problem.json"
echo "judge: RE"
OUT="$("$BIN" judge "$DIR4" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "exit code is 1 on RE"
echo "$OUT" | grep -q "^Test #1: RE$" && pass "RE verdict" || fail "RE verdict"

# ---------------------------------------------------------------- compile error
DIR5="$SCRATCH/broken"
mkdir -p "$DIR5"
printf 'int main() { this is not c++ }\n' > "$DIR5/main.cpp"
printf '{"timeLimit":1000}' > "$DIR5/problem.json"
printf 'x\n' > "$DIR5/in1"; printf 'y\n' > "$DIR5/ans1"
echo "judge: compile error"
OUT="$("$BIN" judge "$DIR5" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "exit code is 1 on compile error"
echo "$OUT" | grep -q "^Compile error:" && pass "compile error reported" || fail "compile error reported"

# ------------------------------------------------------ custom compile flags
# Program whose behaviour depends on -DZEDCOMP_TEST_FLAG; ans1 expects FLAG, so
# the test is AC only when the define actually reached the compiler.
DIR7="$SCRATCH/flags"
mkdir -p "$DIR7"
cat > "$DIR7/main.cpp" <<'CPP'
#include <cstdio>
int main() {
#ifdef ZEDCOMP_TEST_FLAG
    std::puts("FLAG");
#else
    std::puts("NOFLAG");
#endif
    return 0;
}
CPP
printf '\n' > "$DIR7/in1"
printf 'FLAG\n' > "$DIR7/ans1"
printf '{"timeLimit":5000}' > "$DIR7/problem.json"

echo "judge: built-in flags when nothing is configured -> macro undefined -> WA"
OUT="$("$BIN" judge "$DIR7" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "default flags leave ZEDCOMP_TEST_FLAG undefined"
echo "$OUT" | grep -q "cxx: .* -std=c++17 -O2)" && pass "summary reports compiler + default flags" || fail "summary reports compiler + default flags"

echo "judge: ZEDCOMP_CXXFLAGS=-DZEDCOMP_TEST_FLAG -> AC"
OUT="$(ZEDCOMP_CXXFLAGS="-DZEDCOMP_TEST_FLAG" "$BIN" judge "$DIR7" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "0" "env compile flag changes the compiled program"
echo "$OUT" | grep -q "cxx: .*-DZEDCOMP_TEST_FLAG)" && pass "summary reports the effective flag" || fail "summary reports the effective flag"

echo "judge: \$ZEDCOMP_CONFIG_DIR/cxxflags is used when the env var is blank"
CFGDIR="$SCRATCH/config-flags"
mkdir -p "$CFGDIR"
printf -- '-DZEDCOMP_TEST_FLAG\n' > "$CFGDIR/cxxflags"
OUT="$(ZEDCOMP_CXXFLAGS="" ZEDCOMP_CONFIG_DIR="$CFGDIR" "$BIN" judge "$DIR7" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "0" "blank ZEDCOMP_CXXFLAGS falls back to the cxxflags file"

echo "judge: a real ZEDCOMP_CXXFLAGS beats the cxxflags file"
OUT="$(ZEDCOMP_CXXFLAGS="-std=c++17" ZEDCOMP_CONFIG_DIR="$CFGDIR" "$BIN" judge "$DIR7" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "env flags win over the config file"

echo "judge: unknown compile flag fails the build"
OUT="$(ZEDCOMP_CXXFLAGS="-fnonexistent-flag" "$BIN" judge "$DIR7" 2>&1)"; CODE=$?
echo "$OUT" | sed 's/^/    | /'
check "$CODE" "1" "unknown flag -> non-zero exit"
echo "$OUT" | grep -q "^Compile error:" && pass "unknown flag reported as a compile error" || fail "unknown flag reported as a compile error"

# ---------------------------------------------------------------- missing dir
echo "judge: bad arguments"
"$BIN" judge "$SCRATCH/does-not-exist" >/dev/null 2>&1
check "$?" "2" "exit code 2 for a non-directory"

# ---------------------------------------------------------------- fixtures via full pipeline
echo "judge: fixture-driven workspace (generated main.cpp, replaced with clang-compatible code)"
DIR6="$SCRATCH/cf118A"
mkdir -p "$DIR6"
if [ -d "$REPO/fixtures" ]; then
  cp "$REPO/fixtures/cf.json" "$DIR6/problem.json"
  cat > "$DIR6/main.cpp" <<'CPP'
#include <iostream>
#include <string>
int main() {
    std::string s, out;
    std::cin >> s;
    for (char c : s) {
        char lower = std::tolower(static_cast<unsigned char>(c));
        if (lower == 'a' || lower == 'o' || lower == 'y' || lower == 'e' ||
            lower == 'u' || lower == 'i') continue;
        out += '.'; out += lower;
    }
    std::cout << out << std::endl;
}
CPP
  printf 'tour\n' > "$DIR6/in1"; printf '.t.r\n' > "$DIR6/ans1"
  printf 'Codeforces\n' > "$DIR6/in2"; printf '.c.d.f.r.c.s\n' > "$DIR6/ans2"
  printf 'aBAcAba\n' > "$DIR6/in3"; printf '.b.c.b\n' > "$DIR6/ans3"
  OUT="$("$BIN" judge "$DIR6" 2>&1)"; CODE=$?
  echo "$OUT" | sed 's/^/    | /'
  check "$CODE" "0" "codeforces 118A style solution passes 3 tests"
else
  echo "  skip fixtures not found"
fi

echo
if [ "$FAILED" -gt 0 ]; then
  echo "$FAILED judge check(s) FAILED"
  exit 1
fi
echo "all judge checks passed"
