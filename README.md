# ZedComp

Competitive programming companion for [Zed](https://zed.dev), modelled after
[CPH](https://github.com/agrawal-d/cph).

ZedComp receives problems pushed by the
[Competitive Companion](https://github.com/jmerle/competitive-companion) browser
extension, creates a problem folder with a C++ template and the sample test
files, opens the source file in Zed, and judges your solution locally in the
terminal.

## How it works

Zed extensions run in a WASM sandbox and cannot listen on a TCP port, so ZedComp
is split in two pieces:

| Piece | Where | Job |
| --- | --- | --- |
| **Extension** (WASM) | this directory (`extension.toml`, `src/lib.rs`) | Finds/starts the helper as an *additional* language server for C, C++ and Python, and tells it which worktree it belongs to. |
| **Helper** (native) | [`helper/`](./helper) (`zedcomp-helper`) | Listens on `127.0.0.1:27121` for Competitive Companion `POST`s, writes the problem folder, opens the file via the `zed` CLI, and implements `judge`. |

The helper is registered as an *additional* language server: it never replaces
clangd or pyright. Zed starts it whenever you open a C, C++ or Python file in
the project, and stops it when the last one closes.

## Requirements

- Zed with support for extension API `0.7.0`.
- The [Competitive Companion](https://github.com/jmerle/competitive-companion)
  browser extension.
- A `zedcomp-helper` binary. The extension looks for it in this order:
  1. `lsp.zedcomp-helper.binary.path` in your `settings.json`;
  2. `zedcomp-helper` on your `$PATH`;
  3. a binary it downloaded earlier into Zed's extension working directory;
  4. the newest release asset for your platform on
     [`BurstWhite/zedcomp`](https://github.com/BurstWhite/zedcomp/releases)
     (assets are named `zedcomp-helper-<target-triple>`, e.g.
     `zedcomp-helper-aarch64-apple-darwin`).
- `g++` on your `$PATH` for `judge`.

## Installation

1. **Get the helper.**

   The extension downloads a prebuilt binary automatically on first use. If you
   prefer to install it yourself (recommended: tasks can then simply call
   `zedcomp-helper`):

   ```sh
   git clone https://github.com/BurstWhite/zedcomp
   cd zedcomp
   cargo install --path helper
   # -> ~/.cargo/bin/zedcomp-helper
   ```

2. **Install the extension.**

   - Development: open the command palette, run `zed: extensions`, click
     **Install Dev Extension** and pick this directory.
   - Once published: search for **ZedComp** on the extensions page.

3. **Open a project.** ZedComp writes problems relative to the worktree root, so
   open the folder where you keep your solutions (for example `~/cp`) *before*
   pulling a problem.

## Usage

1. Open a folder in Zed and open any `.c`/`.cpp`/`.py` file, so that Zed starts
   the helper.
2. Configure Competitive Companion to send problems to port **27121** — that is
   the default CPH port and also ZedComp's default. If you also use the VSCode
   CPH extension, give one of them a different port (in Competitive Companion:
   *Settings → Custom ports*) and mirror it in `settings.json` (below).
3. Click the Competitive Companion icon on a problem page (or on a contest page
   to pull every problem, one by one).

ZedComp then creates:

```
<workspace_root>/<oj>/<contest>/<problem>/
├── main.cpp        # rendered from the template, created once, never overwritten
├── in1  ans1       # sample tests from the POST body
├── in2  ans2
├── ...
└── problem.json    # the raw Competitive Companion payload
```

`main.cpp` is opened in Zed automatically (via the `zed` CLI).

| OJ host | `<oj>` | Directory layout |
| --- | --- | --- |
| `codeforces.com` | `cf` | `cf/<contest-id>/<problem-letter>/` |
| `atcoder.jp` | `ac` | `ac/<contest-id>/<task-id>/` |
| `luogu.com.cn` | `luogu` | `luogu/<problem-id>/` |

## Settings

```jsonc
// settings.json
{
  "lsp": {
    "zedcomp-helper": {
      // Optional: only needed when the helper is not on your $PATH.
      "binary": {
        "path": "/Users/you/.cargo/bin/zedcomp-helper",
        "arguments": [],
        "env": {}
      },
      // Optional: the port Competitive Companion should POST to.
      // Defaults to 27121 (or $ZEDCOMP_PORT when set).
      "initialization_options": {
        "port": 27121
        // Optional: template / templatePath — see
        // "Customizing the code template" below.
      }
    }
  }
}
```

The extension always exports `ZEDCOMP_WORKSPACE=<worktree root>` to the helper,
which is why problem folders land in the project you opened. When you start the
helper by hand, set it yourself:

```sh
ZEDCOMP_WORKSPACE=~/cp zedcomp-helper
```

## Customizing the code template

The helper renders `main.cpp` from a template. Four sources are consulted and
the **first one that is usable wins** — every level is optional:

| # | Source | Notes |
| --- | --- | --- |
| 1 | `initializationOptions.templatePath` | Absolute path of a template file. A leading `~/` is expanded. An unreadable path logs a warning on stderr and falls through to the next level. |
| 2 | `initializationOptions.template` | The template as an inline string in `settings.json`. |
| 3 | `$ZEDCOMP_CONFIG_DIR/template.cpp` | `ZEDCOMP_CONFIG_DIR` defaults to `~/.config/zedcomp` (on macOS too — `XDG_CONFIG_HOME` is deliberately not consulted). |
| 4 | built-in default | `#include <bits/stdc++.h>`, fast IO, empty `main()`. Used when nothing above is configured. |

These placeholders are replaced when a problem is fetched; all of them are
optional, and a value that does not exist for a problem becomes an empty string:

| Placeholder | Value | Example |
| --- | --- | --- |
| `{{PROBLEM_NAME}}` | Competitive Companion `name` | `A. String Task` |
| `{{URL}}` | Competitive Companion `url` | `https://codeforces.com/problemset/problem/118/A` |
| `{{CONTEST}}` | Contest segment of the URL (empty for e.g. bare Luogu problems) | `118` |
| `{{PROBLEM_ID}}` | Problem segment of the URL | `A`, `abc300_a`, `P1000` |
| `{{OJ}}` | Short OJ code, i.e. the first directory level | `cf`, `ac`, `luogu` |

Newlines in substituted values are flattened to spaces, so a multi-line problem
name cannot break the comment block of a template. `main.cpp` is written once
and never overwritten — delete it (or edit it in place) to pick up a new
template for an existing problem. The template *is* re-read for every fetched
problem, so editing the config file does not require restarting the helper; the
`window/logMessage` ("ZedComp helper ready: … template: …") reports which source
is in use.

### A template file in `~/.config/zedcomp`

Create `~/.config/zedcomp/template.cpp` and it applies to every problem without
touching `settings.json`:

```cpp
// {{PROBLEM_NAME}}
// {{OJ}}/{{CONTEST}}/{{PROBLEM_ID}}
// {{URL}}
#include <bits/stdc++.h>
using namespace std;

using ll = long long;

void solve() {
}

int main() {
    ios::sync_with_stdio(false);
    cin.tie(nullptr);

    int t = 1;
    // cin >> t;
    while (t--) solve();
    return 0;
}
```

Any other location works too — point the helper at it (see below), or keep the
file outside `~/.config` and set `ZEDCOMP_CONFIG_DIR` in the environment that
launches Zed (or in the `env` block below).

### Pointing `settings.json` at a template

```jsonc
// settings.json
{
  "lsp": {
    "zedcomp-helper": {
      "initialization_options": {
        "port": 27121,
        // 1. highest priority: read the template from this file
        "templatePath": "/Users/you/cp/template.cpp"
        // 2. or inline it (templatePath wins when both are set):
        // "template": "// {{PROBLEM_NAME}} ({{OJ}}/{{PROBLEM_ID}})\n#include <bits/stdc++.h>\n"
      }
    }
  }
}
```

`~/cp/template.cpp` also works (`~` is expanded). Both keys may live inside a
nested `"zedcomp"` object instead, e.g.
`"initialization_options": { "zedcomp": { "templatePath": "…" } }`.

## Judging from a task

`judge` compiles `main.cpp` in a problem folder
(`g++ -std=c++17 -O2 -o .main main.cpp`), runs every test pair within the
problem's `timeLimit`, and prints one line per test:

```
Test #1: AC (12ms)
Test #2: WA (8ms)
```

It exits with status `0` only when every test is AC. Comparison ignores trailing
whitespace per line and blank lines at the end of the file.

Add tasks to `.zed/tasks.json` (project-local) or to your global `tasks.json`:

```jsonc
[
  {
    // `$ZED_DIRNAME` is the folder of the file you are editing, i.e. the
    // problem folder. Requires `zedcomp-helper` on your $PATH.
    "label": "ZedComp: Judge",
    "command": "zedcomp-helper",
    "args": ["judge", "$ZED_DIRNAME"],
    "cwd": "$ZED_WORKTREE_ROOT",
    "reveal": "always",
    "use_new_terminal": false,
    "allow_concurrent_runs": false
  },
  {
    // Same thing, but deriving the folder from `${ZED_FILE}` through a shell.
    "label": "ZedComp: Judge (from ${ZED_FILE})",
    "command": "sh",
    "args": ["-c", "exec zedcomp-helper judge \"$(dirname \"${ZED_FILE}\")\""],
    "reveal": "always",
    "use_new_terminal": false,
    "allow_concurrent_runs": false
  },
  {
    // If you rely on the auto-downloaded helper, point the task at the binary
    // in Zed's extension working directory instead (adjust the triple for your
    // platform: aarch64-apple-darwin, x86_64-apple-darwin,
    // x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc.exe, ...).
    "label": "ZedComp: Judge (auto-downloaded helper)",
    "command": "$HOME/Library/Application Support/Zed/extensions/work/zedcomp/zedcomp-helper-aarch64-apple-darwin",
    "args": ["judge", "$ZED_DIRNAME"],
    "reveal": "always",
    "use_new_terminal": false,
    "allow_concurrent_runs": false
  }
]
```

Run a task with `cmd-shift-r` (macOS) / `ctrl-shift-r` (Linux, Windows) and pick
**ZedComp: Judge**.

## Snippets

Typing `cpt` in an editor and accepting the completion inserts a C++17
competitive programming template (`#include <bits/stdc++.h>`, fast IO,
multi-test `solve()`), with the cursor placed inside `solve()`.

Zed selects a snippet scope from the file stem: the lowercased language name
(`c++.json` for C++, `python.json` for Python), while `snippets.json` is the
*global* scope. The file shipped here is `snippets/snippets.json`, so `cpt`
works in every language. To restrict it to C++, copy the file to
`snippets/c++.json` and update the `snippets` key in `extension.toml`.

## Troubleshooting

- **Nothing happens when I click the Competitive Companion icon.**
  Make sure the helper is running (`ps aux | grep zedcomp-helper`) — Zed starts
  it when a C/C++/Python file is open — and that Competitive Companion posts to
  the same port. Check the worktree root the helper was given.
- **`port 27121 is already in use`.** Another ZedComp helper (one per Zed
  window) or VSCode's CPH already owns the port. The helper logs the error to
  stderr and degrades to a plain LSP no-op instead of exiting. Change the port
  for one of the tools.
- **Where are the logs?** Extension and helper stdout/stderr go to Zed's log;
  start Zed from a terminal with `zed --foreground` to see them.
- **Download failed / unsupported platform.** Build the helper from source
  (`cargo install --path helper`) and either put it on your `$PATH` or point
  `lsp.zedcomp-helper.binary.path` at it.
- **`fatal error: 'bits/stdc++.h' file not found` (macOS, `judge`).** Apple's
  clang ships no `bits/stdc++.h`, so the helper must compile with GCC. Install
  it (`brew install gcc`) and point the helper at the real `g++`:
  either export `ZEDCOMP_CXX=g++-14` in the shell that launches Zed, or add it
  to the language server's environment so it works no matter how Zed is started:

  ```jsonc
  {
    "lsp": {
      "zedcomp-helper": {
        "binary": { "env": { "ZEDCOMP_CXX": "g++-14" } }
      }
    }
  }
  ```

  (`g++-14` is the binary Homebrew installs for GCC 14; check
  `ls /opt/homebrew/bin/g++-*` and adjust the version.)

## Distribution / Installing for others

### As a dev extension (from this repository)

1. `git clone https://github.com/BurstWhite/zedcomp && cd zedcomp`.
2. In Zed, open the command palette and run `zed: extensions`, then click
   **Install Dev Extension** and pick the repository root (the directory holding
   `extension.toml`).
3. Zed compiles `src/lib.rs` for `wasm32-wasip2`, so the machine needs Rust and
   that target: `rustup target add wasm32-wasip2`. Rebuilding after an edit is
   the same command Zed runs:
   `cargo build --target wasm32-wasip2 --release`.
4. That is all: the first time the helper is needed, the extension downloads
   the matching prebuilt binary (below). Nothing has to be installed by hand,
   although `cargo install --path helper` (putting `zedcomp-helper` on `$PATH`)
   or `lsp.zedcomp-helper.binary.path` both take precedence and are handy for
   local hacking.
5. Users already on a published version should uninstall that one first — Zed
   keys extensions by id.

### How the helper downloads itself

`language_server_command()` resolves the binary in this order, and only the last
step touches the network:

1. `lsp.zedcomp-helper.binary.path` from `settings.json`,
2. `zedcomp-helper` on `$PATH`,
3. a binary downloaded into Zed's extension working directory during an earlier
   session (this is cached per session in memory too),
4. the newest GitHub release of [`BurstWhite/zedcomp`](https://github.com/BurstWhite/zedcomp/releases):
   the extension asks for the latest non-pre-release release that has assets,
   picks the asset named after the current target triple, downloads it into the
   extension working directory and marks it executable.

Asset names are the contract between CI and the extension —
`zedcomp-helper-<target-triple>` (`.exe` appended on Windows), e.g.
`zedcomp-helper-aarch64-apple-darwin`, `zedcomp-helper-x86_64-unknown-linux-gnu`,
`zedcomp-helper-x86_64-pc-windows-msvc.exe`. A `.gz` variant of any of those is
also accepted and decompressed while downloading. If the release has no asset
for the platform (or the download fails), the error tells the user to
`cargo install --path helper` instead.

### Publishing a new version

1. Bump `version` in `extension.toml` (and in `Cargo.toml` /
   `helper/Cargo.toml` when the helper changed), then commit.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. [`.github/workflows/release.yml`](.github/workflows/release.yml) triggers on
   any `v*` tag, cross-builds the helper for the four platforms in a matrix
   (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
   `x86_64-pc-windows-msvc`), strips the unix binaries and publishes a GitHub
   release with one asset per platform. No manual upload step.
4. [`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on `main`:
   `cargo test --manifest-path helper/Cargo.toml` plus the `wasm32-wasip2`
   build of the extension. Keep both green before tagging.
5. Users on the dev extension get the new binary the next time the helper is
   resolved (the file name is fixed per platform, so it is re-downloaded and
   overwritten). Nothing needs to be re-installed manually.

### Getting into the official extension registry

Publishing upstream is a separate, later step: the extension is not in
[`zed-industries/extensions`](https://github.com/zed-industries/extensions) yet.
To propose it, fork that repository, add this repository as a submodule under
`extensions/zedcomp`, add the matching entry (id `zedcomp`) to
`extensions.toml`, and open a PR; the helper binaries keep living on this
repository's GitHub Releases, so nothing else changes. Until then the dev
extension flow above is the supported install path.

## Building the extension

```sh
cargo build --target wasm32-wasip2 --release
# -> target/wasm32-wasip2/release/zedcomp_extension.wasm
```

Zed builds dev extensions the same way; it needs the `wasm32-wasip2` target
(`rustup target add wasm32-wasip2`).

## Repository layout

```
extension.toml        # Zed extension manifest (Cargo.toml + this WASM crate)
Cargo.toml            # zedcomp-extension, cdylib, depends on zed_extension_api 0.7
src/lib.rs            # Extension impl: language_server_command / initialization options
snippets/snippets.json# `cpt` competitive programming template
helper/               # zedcomp-helper: HTTP listener, file generation, `judge`
PLAN.md               # design notes and architecture
```
