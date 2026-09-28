//! C++ solution template resolution and rendering.
//!
//! ZedComp ships **no** built-in template: when nothing is configured, the
//! generated `main.cpp` is a 0-byte empty file and you write the first line
//! yourself. Configure one source once and every fetched problem gets your
//! usual skeleton instead — without rebuilding the helper.
//!
//! The template written to `main.cpp` is resolved from the highest-priority
//! source that is actually available:
//!
//! 1. `initializationOptions.templatePath` — absolute path of a template file,
//! 2. `initializationOptions.template` — inline template string,
//! 3. `$ZEDCOMP_CONFIG_DIR/template.cpp`, defaulting to
//!    `$HOME/.config/zedcomp/template.cpp`,
//! 4. nothing configured — an empty `main.cpp` ([`EMPTY_TEMPLATE`]).
//!
//! Every level is optional: a level that is not configured falls through to the
//! next one, and a level that is configured but unusable (unreadable
//! `templatePath`, for instance) is reported on stderr and skipped rather than
//! aborting the fetch.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{config_dir, env_var};

/// Placeholder for the problem name inside a template.
pub const PLACEHOLDER_NAME: &str = "{{PROBLEM_NAME}}";
/// Placeholder for the problem URL inside a template.
pub const PLACEHOLDER_URL: &str = "{{URL}}";
/// Placeholder for the contest identifier (`118`, `abc300`, `12345`).
pub const PLACEHOLDER_CONTEST: &str = "{{CONTEST}}";
/// Placeholder for the problem identifier (`A`, `abc300_a`, `P1000`).
pub const PLACEHOLDER_PROBLEM_ID: &str = "{{PROBLEM_ID}}";
/// Placeholder for the short OJ code (`cf`, `ac`, `luogu`).
pub const PLACEHOLDER_OJ: &str = "{{OJ}}";

/// File name looked up inside the config directory (`$ZEDCOMP_CONFIG_DIR`,
/// default `~/.config/zedcomp`; see [`crate::config`]).
pub const CONFIG_TEMPLATE_NAME: &str = "template.cpp";

/// Default content written to `main.cpp` when no template is configured: none.
///
/// This is deliberately empty — no comment banner, no `#include`, not even a
/// newline — so the generated `main.cpp` is a 0-byte file. Configure
/// [`TemplateSpec`] options or `$ZEDCOMP_CONFIG_DIR/template.cpp` to get a
/// skeleton.
pub const EMPTY_TEMPLATE: &str = "";

/// Where a resolved template's content came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateOrigin {
    /// `initializationOptions.templatePath` (the path that was read).
    OptionsPath(PathBuf),
    /// `initializationOptions.template`.
    OptionsInline,
    /// `$ZEDCOMP_CONFIG_DIR/template.cpp` or `~/.config/zedcomp/template.cpp`.
    ConfigFile(PathBuf),
    /// Nothing was configured: [`EMPTY_TEMPLATE`], i.e. an empty `main.cpp`.
    EmptyDefault,
}

impl TemplateOrigin {
    /// Short human-readable description used in `window/logMessage` output.
    pub fn describe(&self) -> String {
        match self {
            Self::OptionsPath(path) => {
                format!("initializationOptions.templatePath ({})", path.display())
            }
            Self::OptionsInline => "initializationOptions.template".to_string(),
            Self::ConfigFile(path) => path.display().to_string(),
            Self::EmptyDefault => "none (main.cpp is written empty)".to_string(),
        }
    }
}

/// Template text plus the source it was resolved from.
#[derive(Debug, Clone)]
pub struct Template {
    content: String,
    origin: TemplateOrigin,
}

impl Template {
    /// The default "template": an empty string, producing a 0-byte `main.cpp`.
    pub fn default_empty() -> Self {
        Self {
            content: EMPTY_TEMPLATE.to_string(),
            origin: TemplateOrigin::EmptyDefault,
        }
    }

    fn new(content: String, origin: TemplateOrigin) -> Self {
        Self::from_parts(content, origin)
    }

    /// Build a template from explicit content and origin.
    pub fn from_parts(content: impl Into<String>, origin: TemplateOrigin) -> Self {
        Self {
            content: content.into(),
            origin,
        }
    }

    /// Raw template text, placeholders included.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Where [`Template::content`] came from.
    pub fn origin(&self) -> &TemplateOrigin {
        &self.origin
    }

    /// Render the template with the problem values substituted.
    pub fn render(&self, values: &TemplateValues<'_>) -> String {
        render(self.content(), values)
    }
}

/// Values substituted into a template.
///
/// Every placeholder is replaced; values that are missing (or empty after
/// whitespace flattening) become the empty string.
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateValues<'a> {
    /// `{{PROBLEM_NAME}}`; falls back to `problem` when empty.
    pub problem_name: &'a str,
    /// `{{URL}}`.
    pub url: &'a str,
    /// `{{CONTEST}}`; `None` when the URL has no contest segment.
    pub contest: Option<&'a str>,
    /// `{{PROBLEM_ID}}`.
    pub problem_id: &'a str,
    /// `{{OJ}}`.
    pub oj: &'a str,
}

/// Substitute the placeholders of `template`.
///
/// Newlines are stripped from the substitutions so a leading `//` comment block
/// cannot be broken by a hostile (or just multi-line) problem name.
pub fn render(template: &str, values: &TemplateValues<'_>) -> String {
    let problem_name = single_line(values.problem_name, "problem");
    let url = single_line(values.url, "");
    let contest = single_line(values.contest.unwrap_or(""), "");
    let problem_id = single_line(values.problem_id, "");
    let oj = single_line(values.oj, "");

    template
        .replace(PLACEHOLDER_NAME, &problem_name)
        .replace(PLACEHOLDER_URL, &url)
        .replace(PLACEHOLDER_CONTEST, &contest)
        .replace(PLACEHOLDER_PROBLEM_ID, &problem_id)
        .replace(PLACEHOLDER_OJ, &oj)
}

/// Template selection coming from the LSP `initialize` request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateSpec {
    /// `initializationOptions.templatePath`.
    pub path: Option<String>,
    /// `initializationOptions.template`.
    pub inline: Option<String>,
}

impl TemplateSpec {
    /// Extract `templatePath` / `template` from `initializationOptions`,
    /// accepting both the top level and a nested `"zedcomp"` object (the same
    /// scoping the port and `workspaceRoot` options use).
    pub fn from_options(options: Option<&Value>) -> Self {
        let Some(options) = options else {
            return Self::default();
        };
        let scoped = options.get("zedcomp").unwrap_or(options);
        Self {
            path: string_field(scoped, &["templatePath", "template_path"])
                .or_else(|| string_field(options, &["templatePath", "template_path"])),
            inline: string_field(scoped, &["template"])
                .or_else(|| string_field(options, &["template"])),
        }
    }

    /// True when neither level of the LSP options configures a template.
    pub fn is_unset(&self) -> bool {
        self.path.is_none() && self.inline.is_none()
    }
}

fn string_field(object: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

/// Resolve the template using the process environment for the config directory.
pub fn resolve(spec: &TemplateSpec) -> Template {
    resolve_with(spec, config_dir().as_deref())
}

/// Resolve the template against an explicit config directory.
///
/// The directory is a parameter (instead of being read from the environment
/// here) so the priority order can be unit-tested deterministically.
pub fn resolve_with(spec: &TemplateSpec, config_dir: Option<&Path>) -> Template {
    if let Some(raw) = spec
        .path
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        let path = expand_tilde(raw);
        match fs::read_to_string(&path) {
            Ok(content) => return Template::new(content, TemplateOrigin::OptionsPath(path)),
            Err(err) => eprintln!(
                "[zedcomp-helper] cannot read templatePath {}: {err}; \
                 trying the next template source",
                path.display()
            ),
        }
    }

    if let Some(inline) = spec
        .inline
        .as_deref()
        .filter(|inline| !inline.trim().is_empty())
    {
        return Template::new(inline.to_string(), TemplateOrigin::OptionsInline);
    }

    if let Some(dir) = config_dir {
        let path = dir.join(CONFIG_TEMPLATE_NAME);
        match fs::read_to_string(&path) {
            Ok(content) => return Template::new(content, TemplateOrigin::ConfigFile(path)),
            // A missing `template.cpp` is the normal case: stay quiet and write
            // an empty `main.cpp`. Anything else is worth a warning.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => eprintln!(
                "[zedcomp-helper] cannot read {}: {err}; main.cpp will be written empty",
                path.display()
            ),
        }
    }

    Template::default_empty()
}

/// Path of the config template file, whether or not it exists.
pub fn config_template_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(CONFIG_TEMPLATE_NAME))
}

/// `~/cp/template.cpp` -> `$HOME/cp/template.cpp` (other paths pass through).
fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = env_var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw)
}

fn single_line(value: &str, fallback: &str) -> String {
    let joined: String = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        fallback.to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values<'a>(contest: Option<&'a str>) -> TemplateValues<'a> {
        TemplateValues {
            problem_name: "A. String Task",
            url: "https://codeforces.com/problemset/problem/118/A",
            contest,
            problem_id: "A",
            oj: "cf",
        }
    }

    /// Fresh scratch directory; the label keeps parallel tests apart.
    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zedcomp-template-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_template(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn default_template_is_empty() {
        assert_eq!(EMPTY_TEMPLATE, "");
        let template = Template::default_empty();
        assert_eq!(template.content(), "");
        // Rendering the empty default stays empty: no banner, not even a newline.
        assert_eq!(template.render(&values(Some("118"))), "");
        assert_eq!(template.origin(), &TemplateOrigin::EmptyDefault);
    }

    #[test]
    fn render_replaces_both_legacy_placeholders() {
        let template = "// {{PROBLEM_NAME}}\n// {{URL}}\n#include <bits/stdc++.h>\nint main() {}\n";
        let out = render(template, &values(Some("118")));
        assert!(out.contains("// A. String Task"));
        assert!(out.contains("// https://codeforces.com/problemset/problem/118/A"));
        assert!(!out.contains(PLACEHOLDER_NAME));
        assert!(!out.contains(PLACEHOLDER_URL));
        // placeholders must be substituted in the header comment only; the rest
        // of the template is byte-identical.
        assert!(out.ends_with("#include <bits/stdc++.h>\nint main() {}\n"));
    }

    #[test]
    fn render_replaces_every_placeholder_including_new_ones() {
        let template =
            "// {{PROBLEM_NAME}}\n// {{OJ}}/{{CONTEST}}/{{PROBLEM_ID}}\n// {{URL}}\n";
        let out = render(template, &values(Some("118")));
        assert_eq!(
            out,
            "// A. String Task\n// cf/118/A\n// https://codeforces.com/problemset/problem/118/A\n"
        );
    }

    #[test]
    fn render_replaces_missing_values_with_empty_strings() {
        let template = "oj=[{{OJ}}] contest=[{{CONTEST}}] id=[{{PROBLEM_ID}}] url=[{{URL}}]";
        let out = render(template, &values(None));
        assert_eq!(out, "oj=[cf] contest=[] id=[A] url=[https://codeforces.com/problemset/problem/118/A]");

        // Every value defaulted to empty: all placeholders still disappear.
        let empty = render(
            "oj=[{{OJ}}] name=[{{PROBLEM_NAME}}]",
            &TemplateValues::default(),
        );
        assert_eq!(empty, "oj=[] name=[problem]");
    }

    #[test]
    fn render_flattens_multiline_names() {
        let out = render(
            "// {{PROBLEM_NAME}}\n",
            &TemplateValues {
                problem_name: "A. Weird\nName",
                ..values(Some("118"))
            },
        );
        assert_eq!(out, "// A. Weird Name\n");
    }

    #[test]
    fn template_path_wins_over_inline_config_and_empty_default() {
        let dir = temp_dir("priority");
        let path = write_template(&dir, "from-options.cpp", "PATH {{PROBLEM_ID}}\n");
        write_template(&dir, CONFIG_TEMPLATE_NAME, "CONFIG\n");

        let spec = TemplateSpec {
            path: Some(path.to_string_lossy().into_owned()),
            inline: Some("INLINE\n".to_string()),
        };
        let template = resolve_with(&spec, Some(&dir));
        assert_eq!(template.content(), "PATH {{PROBLEM_ID}}\n");
        assert_eq!(template.origin(), &TemplateOrigin::OptionsPath(path.clone()));
        assert_eq!(template.render(&values(Some("118"))), "PATH A\n");
    }

    #[test]
    fn inline_wins_over_config_and_empty_default() {
        let dir = temp_dir("inline");
        write_template(&dir, CONFIG_TEMPLATE_NAME, "CONFIG\n");

        let spec = TemplateSpec {
            path: Some(dir.join("does-not-exist.cpp").to_string_lossy().into_owned()),
            inline: Some("INLINE {{OJ}}\n".to_string()),
        };
        // The bad path warns and falls back to the inline template.
        let template = resolve_with(&spec, Some(&dir));
        assert_eq!(template.content(), "INLINE {{OJ}}\n");
        assert_eq!(template.origin(), &TemplateOrigin::OptionsInline);
        assert_eq!(template.render(&values(None)), "INLINE cf\n");
    }

    #[test]
    fn config_file_wins_over_empty_default() {
        let dir = temp_dir("config");
        let path = write_template(&dir, CONFIG_TEMPLATE_NAME, "CONFIG {{OJ}}/{{CONTEST}}\n");

        let template = resolve_with(&TemplateSpec::default(), Some(&dir));
        assert_eq!(template.content(), "CONFIG {{OJ}}/{{CONTEST}}\n");
        assert_eq!(template.origin(), &TemplateOrigin::ConfigFile(path));
        assert_eq!(template.render(&values(Some("118"))), "CONFIG cf/118\n");
    }

    #[test]
    fn empty_default_when_nothing_else_is_available() {
        let dir = temp_dir("empty-default");
        let template = resolve_with(&TemplateSpec::default(), Some(&dir));
        assert_eq!(template.content(), EMPTY_TEMPLATE);
        assert_eq!(template.origin(), &TemplateOrigin::EmptyDefault);

        let no_config = resolve_with(&TemplateSpec::default(), None);
        assert_eq!(no_config.origin(), &TemplateOrigin::EmptyDefault);
        assert_eq!(no_config.content(), "");
    }

    #[test]
    fn bad_template_path_falls_back_to_config_then_empty_default() {
        let dir = temp_dir("bad-path");
        let missing = dir.join("nope.cpp").to_string_lossy().into_owned();

        // Bad path + config file present -> config file.
        write_template(&dir, CONFIG_TEMPLATE_NAME, "CONFIG\n");
        let with_config = resolve_with(
            &TemplateSpec {
                path: Some(missing.clone()),
                inline: None,
            },
            Some(&dir),
        );
        assert_eq!(with_config.content(), "CONFIG\n");
        assert!(matches!(with_config.origin(), TemplateOrigin::ConfigFile(_)));

        // Bad path + no config file -> the empty default, never a panic.
        let empty = temp_dir("bad-path-empty");
        let fallback = resolve_with(
            &TemplateSpec {
                path: Some(missing),
                inline: None,
            },
            Some(&empty),
        );
        assert_eq!(fallback.origin(), &TemplateOrigin::EmptyDefault);
        assert_eq!(fallback.content(), EMPTY_TEMPLATE);
    }

    #[test]
    fn unreadable_template_path_and_blank_inline_are_ignored() {
        let dir = temp_dir("unreadable");
        let spec = TemplateSpec {
            // A directory is not readable as a file on every platform.
            path: Some(dir.to_string_lossy().into_owned()),
            inline: Some("   \n".to_string()),
        };
        let template = resolve_with(&spec, Some(&dir));
        assert_eq!(template.origin(), &TemplateOrigin::EmptyDefault);
        assert_eq!(template.content(), "");
    }

    #[test]
    fn spec_reads_options_at_top_level_and_under_zedcomp() {
        let flat = serde_json::json!({
            "port": 27121,
            "templatePath": "/tmp/flat.cpp",
            "template": "flat",
        });
        let spec = TemplateSpec::from_options(Some(&flat));
        assert_eq!(spec.path.as_deref(), Some("/tmp/flat.cpp"));
        assert_eq!(spec.inline.as_deref(), Some("flat"));

        let nested = serde_json::json!({
            "zedcomp": { "template_path": "/tmp/nested.cpp", "template": "nested" }
        });
        let spec = TemplateSpec::from_options(Some(&nested));
        assert_eq!(spec.path.as_deref(), Some("/tmp/nested.cpp"));
        assert_eq!(spec.inline.as_deref(), Some("nested"));

        assert!(TemplateSpec::from_options(None).is_unset());
        assert!(TemplateSpec::from_options(Some(&serde_json::json!({"port": 1}))).is_unset());
        // Non-string values are ignored instead of panicking.
        assert!(TemplateSpec::from_options(Some(&serde_json::json!({"template": 42}))).is_unset());
    }

    // NOTE: the environment-driven parts of the config directory
    // (`ZEDCOMP_CONFIG_DIR`, `XDG_CONFIG_HOME`, `HOME`) are covered by
    // `config::tests::env_override_wins_and_blank_values_fall_back_to_home`,
    // which is the single test allowed to mutate the process environment.
}
