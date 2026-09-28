//! Compiler and compile-flag resolution for `judge`.
//!
//! `judge` is a standalone CLI process — it is never started through the LSP,
//! so `initializationOptions` does not (and cannot) configure it. Two sources
//! are consulted instead, plus a built-in default; the **first one that is
//! configured wins**:
//!
//! 1. `ZEDCOMP_CXXFLAGS` — environment variable,
//! 2. `$ZEDCOMP_CONFIG_DIR/cxxflags` — default `~/.config/zedcomp/cxxflags`,
//! 3. [`DEFAULT_FLAGS`] — `-std=c++17 -O2`.
//!
//! Splitting is deliberately naive: the value is split on ASCII whitespace
//! (spaces, tabs, newlines) and empty pieces are dropped. There is **no**
//! quoting or backslash escaping, so a flag cannot contain whitespace; write
//! `-DNAME=VALUE` instead of `-D NAME=VALUE` if you need that. An unset, empty
//! or whitespace-only value counts as "not configured" and falls through to the
//! next source.
//!
//! The compiler itself is `ZEDCOMP_CXX` (default `g++`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{config_dir, env_var};

/// Environment variable selecting the compiler.
pub const CXX_ENV: &str = "ZEDCOMP_CXX";
/// Environment variable holding the compile flags.
pub const CXXFLAGS_ENV: &str = "ZEDCOMP_CXXFLAGS";
/// File name looked up inside the config directory.
pub const CONFIG_FLAGS_NAME: &str = "cxxflags";
/// Compiler used when `ZEDCOMP_CXX` is not configured.
pub const DEFAULT_COMPILER: &str = "g++";
/// Compile flags used when neither `ZEDCOMP_CXXFLAGS` nor the config file apply.
pub const DEFAULT_FLAGS: [&str; 2] = ["-std=c++17", "-O2"];

/// Where the compiler and flags came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagsOrigin {
    /// `ZEDCOMP_CXXFLAGS`.
    Env,
    /// `$ZEDCOMP_CONFIG_DIR/cxxflags` or `~/.config/zedcomp/cxxflags`.
    ConfigFile(PathBuf),
    /// [`DEFAULT_FLAGS`], compiled into the binary.
    Default,
}

impl FlagsOrigin {
    /// Short human-readable description, e.g. for `--help`-style debugging.
    pub fn describe(&self) -> String {
        match self {
            Self::Env => format!("${CXXFLAGS_ENV}"),
            Self::ConfigFile(path) => path.display().to_string(),
            Self::Default => "built-in default".to_string(),
        }
    }
}

/// Everything `judge` needs to build `<dir>/main.cpp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileConfig {
    /// The compiler executable (`ZEDCOMP_CXX`, default [`DEFAULT_COMPILER`]).
    pub compiler: String,
    /// Flags in the order they will be passed on the command line.
    pub flags: Vec<String>,
    /// Where [`CompileConfig::flags`] came from.
    pub origin: FlagsOrigin,
}

impl CompileConfig {
    /// `<compiler> <flags…>`, the way it appears on the command line.
    pub fn describe(&self) -> String {
        if self.flags.is_empty() {
            self.compiler.clone()
        } else {
            format!("{} {}", self.compiler, self.flags.join(" "))
        }
    }
}

/// Split a raw flag string on ASCII whitespace, dropping empty pieces.
///
/// No quoting, no escaping: `"-O2  -Wall"` is two flags, `""` is none.
pub fn split_flags(raw: &str) -> Vec<String> {
    raw.split_ascii_whitespace().map(str::to_string).collect()
}

/// Resolve the compiler and flags from the process environment.
pub fn resolve() -> CompileConfig {
    resolve_with(
        env_var(CXX_ENV).as_deref(),
        env_var(CXXFLAGS_ENV).as_deref(),
        config_dir().as_deref(),
    )
}

/// Resolve against explicit values instead of the process environment.
///
/// Every parameter is optional and is a parameter (rather than being read from
/// the environment here) so the priority order can be unit-tested
/// deterministically:
///
/// * `compiler` — `None`/blank falls back to [`DEFAULT_COMPILER`],
/// * `env_flags` — `None`/blank falls through to the config file,
/// * `config_dir` — directory holding `cxxflags`; `None` disables that level.
pub fn resolve_with(
    compiler: Option<&str>,
    env_flags: Option<&str>,
    config_dir: Option<&Path>,
) -> CompileConfig {
    let compiler = compiler
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_COMPILER)
        .to_string();

    // 1. ZEDCOMP_CXXFLAGS
    if let Some(raw) = env_flags {
        let flags = split_flags(raw);
        if !flags.is_empty() {
            return CompileConfig {
                compiler,
                flags,
                origin: FlagsOrigin::Env,
            };
        }
    }

    // 2. $ZEDCOMP_CONFIG_DIR/cxxflags (missing file is the normal case).
    if let Some(dir) = config_dir {
        let path = dir.join(CONFIG_FLAGS_NAME);
        match fs::read_to_string(&path) {
            Ok(content) => {
                let flags = split_flags(&content);
                if !flags.is_empty() {
                    return CompileConfig {
                        compiler,
                        flags,
                        origin: FlagsOrigin::ConfigFile(path),
                    };
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => eprintln!(
                "[zedcomp-helper] cannot read {}: {err}; using the built-in compile flags",
                path.display()
            ),
        }
    }

    // 3. Built-in default.
    CompileConfig {
        compiler,
        flags: DEFAULT_FLAGS.iter().map(|flag| flag.to_string()).collect(),
        origin: FlagsOrigin::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zedcomp-cxxflags-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn default_flags() -> Vec<String> {
        DEFAULT_FLAGS.iter().map(|flag| flag.to_string()).collect()
    }

    #[test]
    fn splits_on_ascii_whitespace_without_quoting() {
        assert_eq!(split_flags("-std=c++20 -O2 -Wall"), vec!["-std=c++20", "-O2", "-Wall"]);
        // Tabs and newlines separate just like spaces; runs collapse.
        assert_eq!(split_flags("-O2\t-Wall\n  -g"), vec!["-O2", "-Wall", "-g"]);
        // No quoting: the quotes stay part of the flag.
        assert_eq!(split_flags("-DMSG=\"a b\""), vec!["-DMSG=\"a", "b\""]);
        assert!(split_flags("").is_empty());
        assert!(split_flags("   \t\n").is_empty());
    }

    #[test]
    fn env_flags_win_and_keep_their_order() {
        let dir = temp_dir("env-wins");
        fs::write(dir.join(CONFIG_FLAGS_NAME), "-O0\n").unwrap();

        let config = resolve_with(Some("clang++"), Some("-std=c++20 -O2 -Wall"), Some(&dir));
        assert_eq!(config.compiler, "clang++");
        assert_eq!(config.flags, vec!["-std=c++20", "-O2", "-Wall"]);
        assert_eq!(config.origin, FlagsOrigin::Env);
        assert_eq!(config.describe(), "clang++ -std=c++20 -O2 -Wall");
    }

    #[test]
    fn blank_env_flags_fall_back_to_the_config_file() {
        let dir = temp_dir("blank-env");
        let path = dir.join(CONFIG_FLAGS_NAME);
        fs::write(&path, "-std=c++20 -DCFG=1\n").unwrap();

        for blank in ["", " ", "\t\n "] {
            let config = resolve_with(None, Some(blank), Some(&dir));
            assert_eq!(
                config.flags,
                vec!["-std=c++20", "-DCFG=1"],
                "blank value {blank:?} must not win"
            );
            assert_eq!(config.origin, FlagsOrigin::ConfigFile(path.clone()));
        }
    }

    #[test]
    fn missing_or_blank_config_file_falls_back_to_the_default() {
        let dir = temp_dir("default-fallthrough");

        // No cxxflags file at all.
        let missing = resolve_with(None, None, Some(&dir));
        assert_eq!(missing.flags, default_flags());
        assert_eq!(missing.origin, FlagsOrigin::Default);
        assert_eq!(missing.compiler, DEFAULT_COMPILER);

        // An empty file is "not configured" too.
        for blank in ["", "  \n"] {
            fs::write(dir.join(CONFIG_FLAGS_NAME), blank).unwrap();
            let empty_file = resolve_with(None, Some("   "), Some(&dir));
            assert_eq!(empty_file.flags, default_flags());
            assert_eq!(empty_file.origin, FlagsOrigin::Default);
        }

        // No config directory available (unset HOME and ZEDCOMP_CONFIG_DIR).
        let no_dir = resolve_with(None, None, None);
        assert_eq!(no_dir.flags, default_flags());
        assert_eq!(no_dir.origin, FlagsOrigin::Default);
    }

    #[test]
    fn blank_compiler_falls_back_to_the_default_and_whitespace_is_trimmed() {
        for blank in [None, Some(""), Some("   ")] {
            assert_eq!(resolve_with(blank, None, None).compiler, DEFAULT_COMPILER);
        }
        assert_eq!(
            resolve_with(Some("  g++-14  "), None, None).compiler,
            "g++-14"
        );
    }

    #[test]
    fn origin_descriptions_are_useful() {
        assert_eq!(FlagsOrigin::Env.describe(), "$ZEDCOMP_CXXFLAGS");
        assert_eq!(FlagsOrigin::Default.describe(), "built-in default");
        assert_eq!(
            FlagsOrigin::ConfigFile(PathBuf::from("/tmp/cxxflags")).describe(),
            "/tmp/cxxflags"
        );
    }
}
