# fixtures/

Competitive Companion **Problem** JSON bodies (the exact POST payloads the
browser extension sends to `http://localhost:<port>/`), used to smoke-test
`zedcomp-helper` without a browser.

| File | OJ | URL | Problem | Tests |
|---|---|---|---|---|
| `cf.json` | Codeforces → `cf` | `https://codeforces.com/problemset/problem/118/A?locale=en` | A. String Task | 3 |
| `ac.json` | AtCoder → `ac` | `https://atcoder.jp/contests/abc096/tasks/abc096_a` | A - Day of Takahashi | 3 |
| `luogu.json` | Luogu → `luogu` | `https://www.luogu.com.cn/problem/P1001` | A+B Problem | 2 |

Every file is a bare Problem object with all fields the helper relies on:
`name`, `group`, `url`, `interactive`, `memoryLimit` (MB), `timeLimit` (ms),
`tests[{input,output}]`, `testType`, `input{type}`, `output{type}`, `languages`.

## Provenance

`cf.json` and `ac.json` are the `.result` field of real parser test fixtures from
[jmerle/competitive-companion](https://github.com/jmerle/competitive-companion)
(master, tree `df90fabb52e8f566ea3382405e22d246af5d6a69`), which wrap the body as
`{url, parser, result}`:

* `tests/data/codeforces/problem/normal.json` (parser `CodeforcesProblemParser`) → `cf.json`
* `tests/data/atcoder/problem/normal.json` (parser `AtCoderProblemParser`) → `ac.json`

Downloaded from
`https://raw.githubusercontent.com/jmerle/competitive-companion/master/tests/data/...`
(the paths were located through
`https://api.github.com/repos/jmerle/competitive-companion/git/trees/master?recursive=1`),
then re-serialized as just the `.result` object. No other edits were made.

`luogu.json` is **hand-written**, since the upstream repo has no Luogu fixture:
it follows the same Problem schema with URL `https://www.luogu.com.cn/problem/P1001`
(so the helper must extract PID `P1001` from `/problem/<PID>`), two `a + b` test
points, `memoryLimit` 125 MB and `timeLimit` 1000 ms.

### Expected id extraction

| Fixture | OJ dir | Contest dir | Problem dir |
|---|---|---|---|
| `cf.json` | `cf` | `118` | `A` |
| `ac.json` | `ac` | `abc096` | `abc096_a` |
| `luogu.json` | `luogu` | *(no contest segment in the URL; the current helper writes `luogu/P1001`, and `scripts/e2e.sh` accepts several layouts)* | `P1001` |

## Usage

Start the helper, then POST a fixture (defaults to `fixtures/cf.json`):

```sh
# helper: 无参数 = serve(stdio LSP + HTTP listener)
ZEDCOMP_WORKSPACE=/tmp/zc-demo ZEDCOMP_PORT=27121 \
  helper/target/debug/zedcomp-helper &

scripts/post_problem.sh                     # fixtures/cf.json
scripts/post_problem.sh fixtures/ac.json    # explicit file
scripts/post_problem.sh fixtures/luogu.json
```

`post_problem.sh` sends `Content-Type: application/json` with the file verbatim
as the body, prints the HTTP status, and exits non-zero unless the helper
answered `200`. `ZEDCOMP_PORT` (default `27121`) and `ZEDCOMP_HOST` (default
`localhost`) select the target.

### End-to-end smoke test

```sh
scripts/e2e.sh
```

It builds `helper/`, starts it against a throwaway workspace + port, POSTs all
three fixtures, asserts `main.cpp` / `in1` / `ans1` / `problem.json` exist for
`cf`, `ac` and `luogu`, checks that a second POST does not overwrite an existing
`main.cpp`, then writes a sample-passing solution into each `main.cpp` and runs
`zedcomp-helper judge <dir>`, requiring `Test #1: AC` and exit code 0. Exit code
is 0 only when every check passed; the temp workspace and helper logs are kept
when it fails.

Environment knobs: `ZEDCOMP_PORT`, `ZEDCOMP_HELPER_BIN`,
`ZEDCOMP_E2E_SKIP_BUILD=1`, `ZEDCOMP_E2E_KEEP=1`, `ZEDCOMP_E2E_STRICT_PORT=1`,
`ZEDCOMP_E2E_NO_ZED_OPEN=1`.

Verified against the current `helper/` build: builds, all three fixtures are
accepted with HTTP 200, the generated layout is `cf/118/A`, `ac/abc096/abc096_a`
and `luogu/P1001`, the no-clobber check holds, and all three judges report
`Test #1: AC` and exit 0. The judge assertion is sound in the other direction
too — a wrong solution yields `Test #1: WA` and a non-zero exit, so the script
cannot pass vacuously.

> **Port 27121 is often taken.** It is also the port the real CPH VS Code
> extension uses. If it is busy and `ZEDCOMP_PORT` was not set explicitly,
> `e2e.sh` warns and falls back to the first free port in 27122–27126 (set
> `ZEDCOMP_E2E_STRICT_PORT=1` to fail instead). A real helper is expected to log
> the conflict and degrade to a pure LSP no-op server rather than exit.

> The judge step overwrites the generated `main.cpp` files inside the temp
> workspace only; nothing outside the temp directory is modified. `e2e.sh`
> never touches `helper/`, `src/` or any other source file.

> The helper calls `zed <main.cpp>` after each POST. On a machine with the Zed
> CLI on `PATH` (as on this one) that opens editor tabs for files in a temp
> directory that is deleted right after the run; pass
> `ZEDCOMP_E2E_NO_ZED_OPEN=1` to prepend a no-op `zed` shim and keep the run
> side-effect free. The default stays faithful to the contract and lets the
> auto-open happen.
