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
├── main.cpp        # created once, never overwritten
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
