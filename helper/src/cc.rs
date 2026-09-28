//! Competitive Companion payload handling.
//!
//! Responsibilities:
//! * tolerant parsing of the CC POST body ("problem.json"),
//! * OJ / contest / problem resolution from the problem URL,
//! * generation of `<workspace_root>/<oj>/<contest>/<problem>/` workspaces.
//!
//! The raw POST body is always stored verbatim as `problem.json`.

use serde_json::Value;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::template;

/// Fallback time limit when `problem.json` (or the CC payload) has none.
pub const DEFAULT_TIME_LIMIT_MS: u64 = 2000;
/// Fallback memory limit (informational only).
pub const DEFAULT_MEMORY_LIMIT_MB: u64 = 256;

/// Result of resolving a problem URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlInfo {
    /// Short OJ code used as the first path segment: `cf`, `ac`, `luogu`, …
    pub oj: String,
    /// Contest identifier (`118`, `abc300`, …); `None` when the URL has none.
    pub contest: Option<String>,
    /// Problem identifier (`A`, `abc300_a`, `P1000`, …).
    pub problem: String,
    /// Lower-cased URL host (without `www.`).
    pub host: String,
    /// True when the host matched one of the OJs with dedicated rules.
    pub known_oj: bool,
}

impl UrlInfo {
    /// Relative workspace directory, e.g. `cf/118/A` or `luogu/P1000`.
    pub fn relative_dir(&self) -> PathBuf {
        let mut dir = PathBuf::from(&self.oj);
        if let Some(contest) = &self.contest {
            dir.push(contest);
        }
        dir.push(&self.problem);
        dir
    }

    /// Human readable description used in log messages.
    pub fn describe(&self) -> String {
        match &self.contest {
            Some(contest) => format!("{}/{}/{}", self.oj, contest, self.problem),
            None => format!("{}/{}", self.oj, self.problem),
        }
    }
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

/// Resolve a problem URL into `(oj, contest, problem)`.
///
/// Supported shapes (query string / fragment / port / `www.` are ignored):
/// * codeforces: `/problemset/problem/<cid>/<letter>`, `/contest/<cid>/problem/<letter>`,
///   `/gym/<cid>/problem/<letter>`
/// * atcoder: `/contests/<cid>/tasks/<task_id>`
/// * luogu: `/problem/<PID>`, `/contest/<cid>/problem/<PID>`
///
/// Unknown hosts fall back to `<first-host-label>/<last-path-segment>` so other
/// OJs still get a usable workspace instead of an error.
pub fn parse_url(url: &str) -> Option<UrlInfo> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let after_scheme = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let (host_port, rest) = match after_scheme.find(['/', '?', '#']) {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
        None => (after_scheme, ""),
    };
    let host_port = host_port.rsplit('@').next().unwrap_or(host_port);
    let raw_host = host_port.split(':').next().unwrap_or(host_port).trim();
    let host = raw_host
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if host.is_empty() {
        return None;
    }

    let path = rest.split(['?', '#']).next().unwrap_or("");
    let segments: Vec<String> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(percent_decode)
        .collect();

    if host_matches(&host, "codeforces.com") {
        return resolve_codeforces(&host, &segments);
    }
    if host_matches(&host, "atcoder.jp") {
        return resolve_atcoder(&host, &segments);
    }
    if host_matches(&host, "luogu.com.cn") || host_matches(&host, "luogu.org") {
        return resolve_luogu(&host, &segments);
    }

    // Unknown OJ: keep something usable.
    let problem = sanitize_component(segments.last()?);
    let oj = sanitize_component(host.split('.').next().unwrap_or(&host));
    if oj.is_empty() || oj == "_" {
        return None;
    }
    Some(UrlInfo {
        oj,
        contest: None,
        problem,
        host,
        known_oj: false,
    })
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn resolve_codeforces(host: &str, segments: &[String]) -> Option<UrlInfo> {
    let problem_pos = find_segment(segments, &["problem"]);

    if let Some(pi) = problem_pos {
        let after_problem = segments.get(pi + 1);
        // `/contest/<cid>/problem/<letter>` and `/gym/<cid>/problem/<letter>`
        if pi >= 2 && is_marker(&segments[pi - 2], &["contest", "gym"]) {
            if let (Some(cid), Some(letter)) = (segments.get(pi - 1), after_problem) {
                return Some(UrlInfo {
                    oj: "cf".to_string(),
                    contest: Some(sanitize_component(cid)),
                    problem: sanitize_component(letter),
                    host: host.to_string(),
                    known_oj: true,
                });
            }
        }
        // `/problemset/problem/<cid>/<letter>`
        if let (Some(cid), Some(letter)) = (after_problem, segments.get(pi + 2)) {
            return Some(UrlInfo {
                oj: "cf".to_string(),
                contest: Some(sanitize_component(cid)),
                problem: sanitize_component(letter),
                host: host.to_string(),
                known_oj: true,
            });
        }
        if let Some(letter) = after_problem {
            return Some(UrlInfo {
                oj: "cf".to_string(),
                contest: None,
                problem: sanitize_component(letter),
                host: host.to_string(),
                known_oj: true,
            });
        }
    }

    let problem = sanitize_component(segments.last()?);
    Some(UrlInfo {
        oj: "cf".to_string(),
        contest: None,
        problem,
        host: host.to_string(),
        known_oj: true,
    })
}

fn resolve_atcoder(host: &str, segments: &[String]) -> Option<UrlInfo> {
    let task_pos = find_segment(segments, &["tasks", "task"]);

    // `/contests/<cid>/tasks/<task_id>`
    if let Some(ti) = task_pos {
        let contest = if ti >= 1 { segments.get(ti - 1) } else { None };
        if let (Some(contest), Some(task_id)) = (contest, segments.get(ti + 1)) {
            return Some(UrlInfo {
                oj: "ac".to_string(),
                contest: Some(sanitize_component(contest)),
                problem: sanitize_component(task_id),
                host: host.to_string(),
                known_oj: true,
            });
        }
    }

    let problem = sanitize_component(segments.last()?);
    Some(UrlInfo {
        oj: "ac".to_string(),
        contest: None,
        problem,
        host: host.to_string(),
        known_oj: true,
    })
}

fn resolve_luogu(host: &str, segments: &[String]) -> Option<UrlInfo> {
    let problem_pos = find_segment(segments, &["problem"]);

    if let Some(pi) = problem_pos {
        if let Some(pid) = segments.get(pi + 1) {
            // `/contest/<cid>/problem/<PID>` (and `/training/<tid>/problem/<PID>`)
            let contest = if pi >= 2 && is_marker(&segments[pi - 2], &["contest", "training"]) {
                segments.get(pi - 1)
            } else {
                None
            };
            return Some(UrlInfo {
                oj: "luogu".to_string(),
                contest: contest.map(|value| sanitize_component(value)),
                problem: sanitize_component(pid),
                host: host.to_string(),
                known_oj: true,
            });
        }
    }

    let problem = sanitize_component(segments.last()?);
    Some(UrlInfo {
        oj: "luogu".to_string(),
        contest: None,
        problem,
        host: host.to_string(),
        known_oj: true,
    })
}

fn find_segment(segments: &[String], needles: &[&str]) -> Option<usize> {
    segments
        .iter()
        .position(|segment| is_marker(segment, needles))
}

fn is_marker(segment: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| segment.eq_ignore_ascii_case(needle))
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[idx + 1..idx + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Make a single path component safe: no separators, no traversal, no empties.
pub fn sanitize_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    let candidate = if trimmed.is_empty() { out } else { trimmed };
    let candidate = if candidate == "." || candidate == ".." || candidate.is_empty() {
        "_".to_string()
    } else {
        candidate
    };
    candidate.chars().take(120).collect()
}

// ---------------------------------------------------------------------------
// Competitive Companion payload
// ---------------------------------------------------------------------------

/// A parsed Competitive Companion POST body, keeping the raw text around.
#[derive(Debug, Clone)]
pub struct CcProblem {
    raw: String,
    value: Value,
}

impl CcProblem {
    /// Parse and validate a CC payload.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(raw).map_err(|err| format!("invalid JSON: {err}"))?;
        if !value.is_object() {
            return Err("payload is not a JSON object".to_string());
        }
        Ok(Self {
            raw: raw.to_string(),
            value,
        })
    }

    /// Original payload text, stored verbatim as `problem.json`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    fn field(&self, key: &str) -> Option<&Value> {
        self.value.get(key)
    }

    /// CC `name` field, e.g. `A. String Task`.
    pub fn name(&self) -> String {
        self.field("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "problem".to_string())
    }

    /// CC `group` field, e.g. `Codeforces - Codeforces Beta Round 89 (Div. 2)`.
    pub fn group(&self) -> Option<String> {
        self.field("group")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
    }

    /// CC `url` field (may be empty for some OJs).
    pub fn url(&self) -> String {
        self.field("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    /// CC `interactive` flag.
    pub fn interactive(&self) -> bool {
        self.field("interactive")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// `timeLimit` in milliseconds (CC sends milliseconds).
    pub fn time_limit_ms(&self) -> u64 {
        as_u64(self.field("timeLimit")).unwrap_or(DEFAULT_TIME_LIMIT_MS)
    }

    /// `memoryLimit` in MB.
    pub fn memory_limit_mb(&self) -> u64 {
        as_u64(self.field("memoryLimit")).unwrap_or(DEFAULT_MEMORY_LIMIT_MB)
    }

    /// `tests` as `(input, output)` pairs; missing fields become empty strings.
    pub fn tests(&self) -> Vec<(String, String)> {
        self.field("tests")
            .and_then(Value::as_array)
            .map(|tests| {
                tests
                    .iter()
                    .map(|test| {
                        let input = test
                            .get("input")
                            .map(value_to_text)
                            .unwrap_or_default();
                        let output = test
                            .get("output")
                            .map(value_to_text)
                            .unwrap_or_default();
                        (input, output)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Best-effort conversion of a CC test field into text.
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

fn as_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|float| float.max(0.0).round() as u64)),
        Value::String(text) => text.trim().parse::<f64>().ok().map(|float| {
            if float.is_finite() && float > 0.0 {
                float.round() as u64
            } else {
                0
            }
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Workspace generation
// ---------------------------------------------------------------------------

/// What [`generate`] wrote to disk.
#[derive(Debug, Clone)]
pub struct GenerateOutcome {
    /// Absolute problem directory.
    pub dir: PathBuf,
    /// Absolute path of `main.cpp`.
    pub main_cpp: PathBuf,
    /// True when `main.cpp` had to be created (it is never overwritten).
    pub wrote_main: bool,
    /// Number of `inK`/`ansK` pairs written.
    pub test_count: usize,
}

/// Create/refresh the problem workspace below `root`.
///
/// * `main.cpp` is created only when missing (your code is never clobbered) and
///   rendered from `template` (see [`crate::template`] for how it is resolved).
/// * `inK` / `ansK` are regenerated from the payload each time; stale pairs from
///   an earlier fetch of the same problem are removed.
/// * `problem.json` is the raw POST body, byte for byte.
pub fn generate(
    root: &Path,
    info: &UrlInfo,
    problem: &CcProblem,
    tests: &[(String, String)],
    template: &template::Template,
) -> io::Result<GenerateOutcome> {
    let dir = root.join(info.relative_dir());
    fs::create_dir_all(&dir)?;

    let main_cpp = dir.join("main.cpp");
    let wrote_main = if main_cpp.exists() {
        false
    } else {
        let name = problem.name();
        let url = problem.url();
        let values = template::TemplateValues {
            problem_name: &name,
            url: &url,
            contest: info.contest.as_deref(),
            problem_id: &info.problem,
            oj: &info.oj,
        };
        let rendered = template.render(&values);
        write_file(&main_cpp, rendered.as_bytes())?;
        true
    };

    for (index, (input, output)) in tests.iter().enumerate() {
        let number = index + 1;
        write_file(&dir.join(format!("in{number}")), input.as_bytes())?;
        write_file(&dir.join(format!("ans{number}")), output.as_bytes())?;
    }
    remove_stale_tests(&dir, tests.len())?;
    write_file(&dir.join("problem.json"), problem.raw().as_bytes())?;

    Ok(GenerateOutcome {
        dir,
        main_cpp,
        wrote_main,
        test_count: tests.len(),
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.flush()
}

/// Remove `inK`/`ansK` files with `K > keep`.
fn remove_stale_tests(dir: &Path, keep: usize) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => return Err(err),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let number = name
            .strip_prefix("in")
            .or_else(|| name.strip_prefix("ans"))
            .and_then(|rest| rest.parse::<usize>().ok());
        if let Some(number) = number {
            if number > keep {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn info(url: &str) -> UrlInfo {
        parse_url(url).unwrap_or_else(|| panic!("failed to parse {url}"))
    }

    #[test]
    fn parses_codeforces_problemset_url() {
        let parsed = info("https://codeforces.com/problemset/problem/118/A?locale=en");
        assert_eq!(parsed.oj, "cf");
        assert_eq!(parsed.contest.as_deref(), Some("118"));
        assert_eq!(parsed.problem, "A");
        assert_eq!(parsed.host, "codeforces.com");
        assert!(parsed.known_oj);
        assert_eq!(parsed.relative_dir(), PathBuf::from("cf/118/A"));
    }

    #[test]
    fn parses_codeforces_contest_and_gym_urls() {
        let contest = info("https://codeforces.com/contest/1772/problem/C");
        assert_eq!(contest.contest.as_deref(), Some("1772"));
        assert_eq!(contest.problem, "C");

        let gym = info("https://codeforces.com/gym/104160/problem/B");
        assert_eq!(gym.oj, "cf");
        assert_eq!(gym.contest.as_deref(), Some("104160"));
        assert_eq!(gym.problem, "B");

        let mirror = info("http://m1.codeforces.com/contest/4/problem/A");
        assert_eq!(mirror.oj, "cf");
        assert_eq!(mirror.contest.as_deref(), Some("4"));
        assert_eq!(mirror.problem, "A");
    }

    #[test]
    fn parses_atcoder_url() {
        let parsed = info("https://atcoder.jp/contests/abc300/tasks/abc300_a?lang=en");
        assert_eq!(parsed.oj, "ac");
        assert_eq!(parsed.contest.as_deref(), Some("abc300"));
        assert_eq!(parsed.problem, "abc300_a");
        assert_eq!(parsed.relative_dir(), PathBuf::from("ac/abc300/abc300_a"));
    }

    #[test]
    fn parses_luogu_urls() {
        let problem = info("https://www.luogu.com.cn/problem/P1000");
        assert_eq!(problem.oj, "luogu");
        assert_eq!(problem.contest, None);
        assert_eq!(problem.problem, "P1000");
        assert_eq!(problem.relative_dir(), PathBuf::from("luogu/P1000"));

        let contest = info("https://www.luogu.com.cn/contest/12345/problem/P2000");
        assert_eq!(contest.oj, "luogu");
        assert_eq!(contest.contest.as_deref(), Some("12345"));
        assert_eq!(contest.problem, "P2000");
    }

    #[test]
    fn unknown_host_falls_back_to_host_and_last_segment() {
        let parsed = info("https://vjudge.net/problem/CodeForces-118A");
        assert_eq!(parsed.oj, "vjudge");
        assert_eq!(parsed.contest, None);
        assert_eq!(parsed.problem, "CodeForces-118A");
        assert!(!parsed.known_oj);
    }

    #[test]
    fn rejects_empty_or_hostless_urls() {
        assert!(parse_url("").is_none());
        assert!(parse_url("   ").is_none());
        assert!(parse_url("https://codeforces.com/").is_none());
    }

    #[test]
    fn sanitizes_path_components() {
        assert_eq!(sanitize_component("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component("A"), "A");
        assert_eq!(sanitize_component("abc300_a"), "abc300_a");
    }

    #[test]
    fn parses_cc_payload_fields() {
        let payload = r#"{
            "name": "A. String Task",
            "group": "Codeforces - Codeforces Beta Round 89 (Div. 2)",
            "url": "https://codeforces.com/problemset/problem/118/A?locale=en",
            "interactive": false,
            "memoryLimit": 256,
            "timeLimit": 2000,
            "tests": [{ "input": "tour\n", "output": ".t.r\n" }],
            "testType": "single",
            "input": { "type": "stdin" },
            "output": { "type": "stdout" }
        }"#;
        let problem = CcProblem::parse(payload).expect("payload parses");
        assert_eq!(problem.name(), "A. String Task");
        assert_eq!(problem.group().as_deref(), Some("Codeforces - Codeforces Beta Round 89 (Div. 2)"));
        assert!(!problem.interactive());
        assert_eq!(problem.time_limit_ms(), 2000);
        assert_eq!(problem.memory_limit_mb(), 256);
        assert_eq!(
            problem.tests(),
            vec![("tour\n".to_string(), ".t.r\n".to_string())]
        );
        assert_eq!(problem.raw(), payload);
    }

    #[test]
    fn tolerates_float_and_string_limits() {
        let problem = CcProblem::parse(r#"{"name":"x","timeLimit":2000.0,"memoryLimit":"256"}"#).unwrap();
        assert_eq!(problem.time_limit_ms(), 2000);
        assert_eq!(problem.memory_limit_mb(), 256);

        let missing = CcProblem::parse(r#"{"name":"x"}"#).unwrap();
        assert_eq!(missing.time_limit_ms(), DEFAULT_TIME_LIMIT_MS);
        assert_eq!(missing.memory_limit_mb(), DEFAULT_MEMORY_LIMIT_MB);
        assert!(missing.tests().is_empty());
    }

    #[test]
    fn rejects_broken_payloads() {
        assert!(CcProblem::parse("not json").is_err());
        assert!(CcProblem::parse("[1,2,3]").is_err());
    }

    #[test]
    fn generates_workspace_with_test_files() {
        let root = std::env::temp_dir().join(format!("zedcomp-cc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let payload = r#"{"name":"A. String Task","url":"https://codeforces.com/problemset/problem/118/A","timeLimit":2000,"tests":[{"input":"tour\n","output":".t.r\n"},{"input":"a\n","output":"a\n"}]}"#;
        let problem = CcProblem::parse(payload).unwrap();
        let info = info(&problem.url());
        let template = template::Template::default_empty();
        let outcome = generate(&root, &info, &problem, &problem.tests(), &template).unwrap();

        assert_eq!(outcome.dir, root.join("cf/118/A"));
        assert!(outcome.wrote_main);
        assert_eq!(outcome.test_count, 2);
        assert!(outcome.main_cpp.ends_with("main.cpp"));
        // No template configured: main.cpp is created as a 0-byte empty file.
        assert_eq!(fs::read(&outcome.main_cpp).unwrap(), Vec::<u8>::new());
        assert_eq!(fs::read_to_string(outcome.dir.join("in1")).unwrap(), "tour\n");
        assert_eq!(fs::read_to_string(outcome.dir.join("ans2")).unwrap(), "a\n");
        assert_eq!(fs::read_to_string(outcome.dir.join("problem.json")).unwrap(), payload);

        // Second run: main.cpp is preserved, stale tests are pruned.
        fs::write(&outcome.main_cpp, "// my solution\n").unwrap();
        let problem2 = CcProblem::parse(
            r#"{"name":"A. String Task","url":"https://codeforces.com/problemset/problem/118/A","tests":[{"input":"z\n","output":"z\n"}]}"#,
        )
        .unwrap();
        let outcome2 = generate(&root, &info, &problem2, &problem2.tests(), &template).unwrap();
        assert!(!outcome2.wrote_main);
        assert_eq!(fs::read_to_string(&outcome.main_cpp).unwrap(), "// my solution\n");
        assert!(outcome2.dir.join("in1").is_file());
        assert!(!outcome2.dir.join("in2").exists());
        assert!(!outcome2.dir.join("ans2").exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// A user template receives the OJ / contest / problem values of the payload.
    #[test]
    fn generates_workspace_from_a_custom_template() {
        let root =
            std::env::temp_dir().join(format!("zedcomp-cc-custom-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let payload = r#"{"name":"A. String Task","url":"https://codeforces.com/problemset/problem/118/A","tests":[]}"#;
        let problem = CcProblem::parse(payload).unwrap();
        let info = info(&problem.url());
        let custom = template::Template::from_parts(
            "// {{OJ}} {{CONTEST}} {{PROBLEM_ID}}\n// {{PROBLEM_NAME}}\n// {{URL}}\n",
            template::TemplateOrigin::OptionsInline,
        );
        let outcome = generate(&root, &info, &problem, &problem.tests(), &custom).unwrap();
        assert_eq!(
            fs::read_to_string(&outcome.main_cpp).unwrap(),
            "// cf 118 A\n// A. String Task\n// https://codeforces.com/problemset/problem/118/A\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Optional fixture check: uses `../fixtures` (repo root) when sample CC
    /// payloads are present, and skips silently otherwise.
    #[test]
    fn parses_fixture_payloads_when_available() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
        let cases = [
            ("cf.json", "cf", "cc_codeforces.json"),
            ("ac.json", "ac", "cc_atcoder.json"),
            ("luogu.json", "luogu", "cc_luogu.json"),
        ];
        let mut checked = 0;
        for (name, expected_oj, alternate) in cases {
            let path = [fixtures.join(name), fixtures.join(alternate)]
                .into_iter()
                .find(|path| path.is_file());
            let Some(path) = path else { continue };
            let raw = fs::read_to_string(&path).expect("fixture is readable");
            let problem = CcProblem::parse(&raw)
                .unwrap_or_else(|err| panic!("fixture {name} failed to parse: {err}"));
            let url = problem.url();
            assert!(!url.is_empty(), "fixture {name} has no url");
            let info = parse_url(&url)
                .unwrap_or_else(|| panic!("fixture {name} url did not resolve: {url}"));
            assert_eq!(info.oj, expected_oj, "fixture {name} wrong OJ");
            assert!(info.known_oj, "fixture {name} host not recognized: {}", info.host);
            assert!(!problem.tests().is_empty(), "fixture {name} has no tests");
            checked += 1;
        }
        assert!(checked > 0, "no CC fixtures found under {}", fixtures.display());
    }
}
