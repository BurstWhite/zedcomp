//! Integration tests for the compile flags of `zedcomp-helper judge`.
//!
//! These drive the **real binary** (not the library) with a controlled
//! environment, because that is the only way to exercise the parts that read
//! `ZEDCOMP_CXXFLAGS` / `$ZEDCOMP_CONFIG_DIR/cxxflags` from the process
//! environment — the same path a user hits from a Zed task or a shell.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The helper binary built for this test run.
const BIN: &str = env!("CARGO_BIN_EXE_zedcomp-helper");

/// Program that changes behaviour depending on `-DZEDCOMP_TEST_FLAG`: the
/// expected answer is `FLAG`, so the test is AC only when the flag arrived.
const PROGRAM: &str = r#"#include <cstdio>

int main() {
#ifdef ZEDCOMP_TEST_FLAG
    std::puts("FLAG");
#else
    std::puts("NOFLAG");
#endif
    return 0;
}
"#;

/// Scratch problem directory, removed on drop.
struct Problem {
    dir: PathBuf,
}

impl Problem {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "zedcomp-judge-flags-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.cpp"), PROGRAM).unwrap();
        fs::write(dir.join("in1"), "").unwrap();
        fs::write(dir.join("ans1"), "FLAG\n").unwrap();
        fs::write(dir.join("problem.json"), r#"{"timeLimit":20000}"#).unwrap();
        Self { dir }
    }

    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Problem {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Run `judge` with a controlled flags environment: the ambient
/// `ZEDCOMP_CXXFLAGS` / `ZEDCOMP_CONFIG_DIR` of the developer machine never
/// leak in, while `ZEDCOMP_CXX` (compiler) and `PATH` are inherited.
fn judge(dir: &Path, cxxflags: Option<&str>, config_dir: Option<&Path>) -> (i32, String) {
    let mut command = Command::new(BIN);
    command.arg("judge").arg(dir);
    command.env_remove("ZEDCOMP_CXXFLAGS");
    command.env_remove("ZEDCOMP_CONFIG_DIR");
    if let Some(flags) = cxxflags {
        command.env("ZEDCOMP_CXXFLAGS", flags);
    }
    if let Some(config_dir) = config_dir {
        command.env("ZEDCOMP_CONFIG_DIR", config_dir);
    }
    let output = command.output().expect("run zedcomp-helper judge");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code().unwrap_or(-1), combined)
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zedcomp-judge-cfg-{}-{label}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn judging_line_reports_the_compiler_and_flags_in_use() {
    let problem = Problem::new("summary");
    let (code, output) = judge(problem.dir(), Some("-std=c++17 -DZEDCOMP_TEST_FLAG"), None);

    assert_eq!(code, 0, "expected AC, got:\n{output}");
    let first = output.lines().next().unwrap_or_default();
    assert!(
        first.starts_with(&format!("Judging {}", problem.dir().display())),
        "unexpected summary line: {first}"
    );
    // The exact compiler depends on ZEDCOMP_CXX (g++, g++-14, clang++, …), so
    // only the shape around it is asserted.
    assert!(
        first.contains("(time limit 20000 ms, 1 test(s), cxx: "),
        "summary line does not report the toolchain: {first}"
    );
    assert!(
        first.ends_with("-std=c++17 -DZEDCOMP_TEST_FLAG)"),
        "summary line is missing the effective flags: {first}"
    );
}

#[test]
fn env_flags_change_what_the_program_compiles_to() {
    let problem = Problem::new("env");

    // Without the define the program prints NOFLAG -> WA.
    let (code, output) = judge(problem.dir(), None, None);
    assert_eq!(code, 1, "expected WA without the flag, got:\n{output}");
    assert!(output.contains("Test #1: WA"), "unexpected output:\n{output}");
    // …and the summary advertises the built-in default flags.
    assert!(
        output.contains("-std=c++17 -O2"),
        "default flags missing from the summary:\n{output}"
    );

    // With the define it prints FLAG -> AC.
    let (code, output) = judge(problem.dir(), Some("-DZEDCOMP_TEST_FLAG"), None);
    assert_eq!(code, 0, "expected AC with the flag, got:\n{output}");
    assert!(output.contains("Test #1: AC"), "unexpected output:\n{output}");
}

#[test]
fn config_file_supplies_flags_and_env_wins_over_it() {
    let problem = Problem::new("config-file");
    let config_dir = scratch_dir("config-file");
    fs::write(config_dir.join("cxxflags"), "-DZEDCOMP_TEST_FLAG\n").unwrap();

    // No env var, but the config file defines the macro -> AC.
    let (code, output) = judge(problem.dir(), None, Some(&config_dir));
    assert_eq!(code, 0, "config file flags not applied:\n{output}");
    assert!(output.contains("Test #1: AC"), "unexpected output:\n{output}");

    // A blank (whitespace-only) env var is "not configured" -> config file wins.
    let (code, output) = judge(problem.dir(), Some("   "), Some(&config_dir));
    assert_eq!(code, 0, "blank ZEDCOMP_CXXFLAGS must fall back to the file:\n{output}");

    // A real env var wins over the file: the macro is gone -> WA again.
    let (code, output) = judge(problem.dir(), Some("-std=c++17"), Some(&config_dir));
    assert_eq!(code, 1, "env flags must beat the config file:\n{output}");
    assert!(output.contains("Test #1: WA"), "unexpected output:\n{output}");

    let _ = fs::remove_dir_all(&config_dir);
}

#[test]
fn unknown_flag_fails_the_compile_instead_of_being_ignored() {
    let problem = Problem::new("bad-flag");
    let (code, output) = judge(problem.dir(), Some("-fnonexistent-flag"), None);

    assert_ne!(code, 0, "an unknown flag must fail the build:\n{output}");
    assert!(
        output.contains("Compile error:"),
        "compile failure not reported:\n{output}"
    );
    assert!(
        output.contains("-fnonexistent-flag"),
        "the failing flags should be visible in the output:\n{output}"
    );
}
