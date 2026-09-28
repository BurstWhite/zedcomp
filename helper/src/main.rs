//! zedcomp-helper — native helper for the ZedComp Zed extension.
//!
//! Three modes:
//! * *(no arguments)* / `serve`: minimal LSP server on stdio **and** a
//!   Competitive Companion HTTP listener on `127.0.0.1` (default `27121`,
//!   overridable via `ZEDCOMP_PORT` or `initializationOptions.port`).
//! * `judge <problem_dir>`: compile + run + compare all test cases.
//! * `--version`: print the version.
//!
//! See `README.md` for the full usage notes.

mod cc;
mod config;
mod cxxflags;
mod http;
mod judge;
mod lsp;
mod template;

use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "\
zedcomp-helper — ZedComp native helper (Competitive Companion receiver + local judge)

USAGE:
    zedcomp-helper                     Run as a language server (stdio LSP) and
                                       listen for Competitive Companion POSTs
    zedcomp-helper serve               Same as above (explicit form)
    zedcomp-helper judge <dir> [opts]  Compile <dir>/main.cpp and check every inK
                                       against ansK. Options: --time-limit <ms>
    zedcomp-helper --version           Print version
    zedcomp-helper --help              Print this help

ENVIRONMENT:
    ZEDCOMP_PORT       HTTP port for Competitive Companion (default 27121)
    ZEDCOMP_WORKSPACE  Workspace root; wins over the LSP rootUri/rootPath
    ZEDCOMP_CONFIG_DIR Directory holding template.cpp and cxxflags
                       (default ~/.config/zedcomp)
    ZEDCOMP_CXX        C++ compiler used by `judge` (default g++)
    ZEDCOMP_CXXFLAGS   Compile flags for `judge` (default \"-std=c++2a -O2 -Wall -Wextra\")
    ZEDCOMP_ZED_CLI    `zed` CLI path used to open main.cpp
    ZEDCOMP_NO_OPEN    Set to 1 to never launch `zed`

LSP initializationOptions:
    { \"port\": 27121, \"workspaceRoot\": \"/path/to/workspace\",
      \"templatePath\": \"/path/to/template.cpp\", \"template\": \"// {{PROBLEM_NAME}}\\n\" }

TEMPLATE (first source that is available wins):
    1. initializationOptions.templatePath (absolute path, ~/ is expanded)
    2. initializationOptions.template (inline string)
    3. $ZEDCOMP_CONFIG_DIR/template.cpp (default ~/.config/zedcomp/template.cpp)
    4. nothing configured: main.cpp is created as an empty (0-byte) file
Placeholders: {{PROBLEM_NAME}} {{URL}} {{CONTEST}} {{PROBLEM_ID}} {{OJ}}

COMPILE FLAGS for `judge` (first source that is available wins):
    1. ZEDCOMP_CXXFLAGS
    2. $ZEDCOMP_CONFIG_DIR/cxxflags (default ~/.config/zedcomp/cxxflags)
    3. built-in default: -std=c++2a -O2 -Wall -Wextra
Values are split on ASCII whitespace: no quoting, no backslash escaping.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        None => serve(),
        Some((command, rest)) => match command.as_str() {
            "--version" | "-V" | "version" => {
                println!("zedcomp-helper {}", env!("CARGO_PKG_VERSION"));
                ExitCode::SUCCESS
            }
            "--help" | "-h" | "help" => {
                println!("{USAGE}");
                ExitCode::SUCCESS
            }
            "serve" => {
                if rest.is_empty() {
                    serve()
                } else {
                    eprintln!("zedcomp-helper: `serve` takes no arguments\n\n{USAGE}");
                    ExitCode::from(2)
                }
            }
            "judge" => ExitCode::from(judge::run(rest) as u8),
            other => {
                eprintln!("zedcomp-helper: unknown subcommand or option: {other}\n\n{USAGE}");
                ExitCode::from(2)
            }
        },
    }
}

/// `serve` mode: HTTP listener on a background thread, LSP on the main thread.
fn serve() -> ExitCode {
    let state = Arc::new(lsp::ServerState::new(lsp::port_from_env()));
    http::spawn(Arc::clone(&state));
    let code = lsp::run_stdio(state);
    ExitCode::from(code as u8)
}
