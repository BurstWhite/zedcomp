//! `zedcomp-helper judge <problem_dir>`
//!
//! Compiles `<problem_dir>/main.cpp` with
//! `g++ -std=c++17 -O2 -o .main main.cpp`, runs it against every `inK` file and
//! compares stdout with `ansK` (trailing whitespace on a line and trailing blank
//! lines are ignored).
//!
//! Output format (one line per test, `k` is the test number):
//! ```text
//! Test #1: AC (12ms)
//! Test #2: WA
//! ```
//! Exit code `0` only when every test passed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::cc;

const COMPILER: &str = "g++";
const COMPILE_FLAGS: [&str; 4] = ["-std=c++17", "-O2", "-o", ".main"];
const POLL_INTERVAL: Duration = Duration::from_millis(1);
const MIN_TIME_LIMIT_MS: u64 = 50;

/// Outcome of a single test run.
#[derive(Debug)]
enum RunOutcome {
    Finished { millis: u64, code: Option<i32>, success: bool },
    TimedOut { millis: u64 },
    SpawnFailed(String),
}

/// Entry point for the `judge` subcommand. `args` are the arguments after
/// `judge`; the first positional argument is the problem directory.
pub fn run(args: &[String]) -> i32 {
    let mut dir_arg: Option<String> = None;
    let mut time_limit_override: Option<u64> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--time-limit" | "-t" => {
                index += 1;
                match args.get(index).and_then(|value| value.parse::<u64>().ok()) {
                    Some(value) if value > 0 => time_limit_override = Some(value),
                    _ => {
                        eprintln!("zedcomp-helper judge: --time-limit expects milliseconds");
                        return 2;
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("zedcomp-helper judge: unknown option {other}");
                return 2;
            }
            other => {
                if dir_arg.replace(other.to_string()).is_some() {
                    eprintln!("zedcomp-helper judge: only one problem directory is supported");
                    return 2;
                }
            }
        }
        index += 1;
    }

    let dir = PathBuf::from(dir_arg.unwrap_or_else(|| ".".to_string()));
    if !dir.is_dir() {
        eprintln!("zedcomp-helper judge: not a directory: {}", dir.display());
        return 2;
    }
    judge_dir(&dir, time_limit_override)
}

fn judge_dir(dir: &Path, time_limit_override: Option<u64>) -> i32 {
    let source = dir.join("main.cpp");
    if !source.is_file() {
        eprintln!(
            "zedcomp-helper judge: {} not found (expected main.cpp)",
            source.display()
        );
        return 1;
    }

    let problem = fs::read_to_string(dir.join("problem.json"))
        .ok()
        .and_then(|raw| match cc::CcProblem::parse(&raw) {
            Ok(problem) => Some(problem),
            Err(err) => {
                eprintln!("zedcomp-helper judge: ignoring problem.json: {err}");
                None
            }
        });
    if problem.is_none() {
        eprintln!("zedcomp-helper judge: no usable problem.json, using defaults");
    }
    let time_limit_ms = time_limit_override
        .unwrap_or_else(|| problem.as_ref().map(|p| p.time_limit_ms()).unwrap_or(cc::DEFAULT_TIME_LIMIT_MS))
        .max(MIN_TIME_LIMIT_MS);

    let cases = match discover_cases(dir) {
        Ok(cases) => cases,
        Err(err) => {
            eprintln!("zedcomp-helper judge: cannot read {}: {err}", dir.display());
            return 1;
        }
    };

    println!(
        "Judging {} (time limit {time_limit_ms} ms, {} test(s))",
        dir.display(),
        cases.len()
    );

    let compiler = std::env::var("ZEDCOMP_CXX")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| COMPILER.to_string());
    if let Err(code) = compile(dir, &compiler) {
        return code;
    }

    if cases.is_empty() {
        eprintln!("zedcomp-helper judge: no inK test files found in {}", dir.display());
        return 1;
    }

    let binary = dir.join(".main");
    let limit = Duration::from_millis(time_limit_ms);
    let mut passed = 0usize;
    let mut all_ok = true;

    for (number, input, answer) in &cases {
        let stdout_path = dir.join(format!(".out{number}"));
        let stderr_path = dir.join(format!(".err{number}"));
        let expected = fs::read_to_string(answer).unwrap_or_default();
        let verdict = match run_one(&binary, dir, input, &stdout_path, &stderr_path, limit) {
            RunOutcome::Finished {
                millis,
                code,
                success,
            } => {
                let actual = fs::read_to_string(&stdout_path).unwrap_or_default();
                if !success {
                    println!("Test #{number}: RE");
                    println!("  exit code: {}", code.map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string()));
                    if let Some(line) = first_line(&fs::read_to_string(&stderr_path).unwrap_or_default()) {
                        println!("  stderr: {line}");
                    }
                    None
                } else if outputs_match(&expected, &actual) {
                    println!("Test #{number}: AC ({millis}ms)");
                    Some(true)
                } else {
                    println!("Test #{number}: WA");
                    print_first_difference(&expected, &actual);
                    None
                }
            }
            RunOutcome::TimedOut { millis } => {
                println!("Test #{number}: TLE");
                println!("  exceeded {time_limit_ms} ms (stopped after {millis} ms)");
                None
            }
            RunOutcome::SpawnFailed(err) => {
                eprintln!("zedcomp-helper judge: could not run {}: {err}", binary.display());
                return 1;
            }
        };

        match verdict {
            Some(true) => passed += 1,
            _ => all_ok = false,
        }

        let _ = fs::remove_file(&stdout_path);
        let _ = fs::remove_file(&stderr_path);
    }

    println!("{passed}/{} test(s) passed", cases.len());
    if all_ok {
        0
    } else {
        1
    }
}

fn compile(dir: &Path, compiler: &str) -> Result<(), i32> {
    let output = Command::new(compiler)
        .args(COMPILE_FLAGS)
        .arg("main.cpp")
        .current_dir(dir)
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            println!("Compile error:");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stdout.trim().is_empty() {
                print!("{stdout}");
            }
            if !stderr.trim().is_empty() {
                print!("{stderr}");
            }
            if stdout.trim().is_empty() && stderr.trim().is_empty() {
                println!("  {compiler} exited with {}", output.status);
            }
            if stderr.contains("bits/stdc++.h") {
                println!(
                    "hint: `bits/stdc++.h` needs GCC's libstdc++; install GCC (e.g. `brew install gcc`) \
                     and/or set ZEDCOMP_CXX=g++-14"
                );
            }
            Err(1)
        }
        Err(err) => {
            eprintln!(
                "zedcomp-helper judge: failed to run {compiler}: {err}\n\
                 hint: install g++ or set ZEDCOMP_CXX (e.g. g++-14, clang++)"
            );
            Err(1)
        }
    }
}

/// `(test number, inK path, ansK path)` sorted by number.
fn discover_cases(dir: &Path) -> std::io::Result<Vec<(u64, PathBuf, PathBuf)>> {
    let mut cases: Vec<(u64, PathBuf, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(number) = name.strip_prefix("in").and_then(|rest| rest.parse::<u64>().ok()) else {
            continue;
        };
        let answer = dir.join(format!("ans{number}"));
        if !answer.is_file() {
            eprintln!("zedcomp-helper judge: skipping in{number}: ans{number} is missing");
            continue;
        }
        cases.push((number, entry.path(), answer));
    }
    cases.sort_by_key(|(number, _, _)| *number);
    Ok(cases)
}

fn run_one(
    binary: &Path,
    dir: &Path,
    input: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    limit: Duration,
) -> RunOutcome {
    let stdin = match fs::File::open(input) {
        Ok(file) => file,
        Err(err) => return RunOutcome::SpawnFailed(format!("cannot open {}: {err}", input.display())),
    };
    let stdout = match fs::File::create(stdout_path) {
        Ok(file) => file,
        Err(err) => {
            return RunOutcome::SpawnFailed(format!("cannot create {}: {err}", stdout_path.display()))
        }
    };
    let stderr = match fs::File::create(stderr_path) {
        Ok(file) => file,
        Err(err) => {
            return RunOutcome::SpawnFailed(format!("cannot create {}: {err}", stderr_path.display()))
        }
    };

    let mut child = match Command::new(binary)
        .current_dir(dir)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return RunOutcome::SpawnFailed(err.to_string()),
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return RunOutcome::Finished {
                    millis: start.elapsed().as_millis() as u64,
                    code: status.code(),
                    success: status.success(),
                }
            }
            Ok(None) => {}
            Err(err) => return RunOutcome::SpawnFailed(err.to_string()),
        }

        if start.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return RunOutcome::TimedOut {
                millis: start.elapsed().as_millis() as u64,
            };
        }
        thread::sleep(POLL_INTERVAL);
    }
}

// ---------------------------------------------------------------------------
// Output comparison
// ---------------------------------------------------------------------------

/// Split into lines, drop trailing whitespace on each line and trailing blank
/// lines at the end of the file (CRLF included).
pub fn normalize_output(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end().to_string())
        .collect();
    while matches!(lines.last(), Some(line) if line.is_empty()) {
        lines.pop();
    }
    lines
}

/// True when actual output matches the expected answer under the rules above.
pub fn outputs_match(expected: &str, actual: &str) -> bool {
    normalize_output(expected) == normalize_output(actual)
}

/// First differing line (1-based index) as `(index, expected, actual)`.
pub fn first_difference(expected: &str, actual: &str) -> Option<(usize, String, String)> {
    let expected_lines = normalize_output(expected);
    let actual_lines = normalize_output(actual);
    let max = expected_lines.len().max(actual_lines.len());
    for index in 0..max {
        let expected_line = expected_lines.get(index).cloned().unwrap_or_default();
        let actual_line = actual_lines.get(index).cloned().unwrap_or_default();
        if expected_line != actual_line {
            return Some((index + 1, expected_line, actual_line));
        }
    }
    None
}

fn print_first_difference(expected: &str, actual: &str) {
    if let Some((line, expected_line, actual_line)) = first_difference(expected, actual) {
        println!("  first difference at line {line}");
        println!("    expected: {}", truncate(&expected_line));
        println!("    actual  : {}", truncate(&actual_line));
    }
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 160;
    if value.chars().count() <= LIMIT {
        value.to_string()
    } else {
        let head: String = value.chars().take(LIMIT).collect();
        format!("{head}…")
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_trailing_whitespace_and_blank_lines() {
        assert!(outputs_match("1 2 3\n", "1 2 3   \n"));
        assert!(outputs_match("1 2 3\n", "1 2 3"));
        assert!(outputs_match("a\nb\n\n\n", "a\nb\n"));
        assert!(outputs_match("a\r\nb\r\n", "a\nb\n"));
        assert!(outputs_match("", "\n\n"));
        // only *trailing* whitespace is insignificant: leading spaces matter.
        assert!(!outputs_match("  x\n", "x\n"));
        assert!(outputs_match("x  \n", "x\n"));
    }

    #[test]
    fn detects_real_differences() {
        assert!(!outputs_match("a\nb\n", "a\nc\n"));
        assert!(!outputs_match("a\nb\n", "b\na\n"));
        assert!(!outputs_match("a\n\nb\n", "a\nb\n"));
        assert!(!outputs_match("a\n", "a\nb\n"));
        assert!(!outputs_match("a\n", "\na\n"));
    }

    #[test]
    fn reports_first_differing_line() {
        let diff = first_difference("one\ntwo\nthree\n", "one\nTWO\nthree\n").expect("difference");
        assert_eq!(diff, (2, "two".to_string(), "TWO".to_string()));
        assert!(first_difference("1\n", "1\n").is_none());
        assert_eq!(
            first_difference("1\n", "1\n2\n"),
            Some((2, String::new(), "2".to_string()))
        );
    }

    #[test]
    fn normalization_drops_only_trailing_blanks() {
        assert_eq!(normalize_output("a\n\nb\n\n"), vec!["a", "", "b"]);
        assert_eq!(normalize_output(""), Vec::<String>::new());
        assert_eq!(normalize_output("   \n\t\n"), Vec::<String>::new());
    }
}
