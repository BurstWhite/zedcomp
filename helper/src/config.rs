//! Shared configuration-directory and environment helpers.
//!
//! Every optional ZedComp setting that can live in a file lives in the same
//! directory — `$ZEDCOMP_CONFIG_DIR`, defaulting to `~/.config/zedcomp`:
//!
//! * `template.cpp` (see [`crate::template`]),
//! * `cxxflags` (see [`crate::cxxflags`]).

use std::env;
use std::path::PathBuf;

/// Environment variable overriding the directory searched for config files.
pub const CONFIG_DIR_ENV: &str = "ZEDCOMP_CONFIG_DIR";

/// Read an environment variable, trimming surrounding whitespace.
///
/// A variable that is unset, empty or whitespace-only is reported as `None`, so
/// `ZEDCOMP_CXXFLAGS=""` behaves exactly like an unset variable.
pub fn env_var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Directory holding the user config files (`template.cpp`, `cxxflags`).
///
/// `ZEDCOMP_CONFIG_DIR` wins when set; otherwise this is `$HOME/.config/zedcomp`
/// (on macOS too — no XDG indirection, matching Zed's own config location).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env_var(CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    env_var("HOME").map(|home| PathBuf::from(home).join(".config").join("zedcomp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template;

    /// The **only** test in the suite that mutates the process environment:
    /// `cargo test` threads share one process, so keeping the mutation in a
    /// single test (and out of every other test) is what makes it race-free.
    #[test]
    fn env_override_wins_and_blank_values_fall_back_to_home() {
        std::env::set_var(CONFIG_DIR_ENV, "/tmp/zedcomp-config-test");
        assert_eq!(env_var(CONFIG_DIR_ENV).as_deref(), Some("/tmp/zedcomp-config-test"));
        assert_eq!(config_dir(), Some(PathBuf::from("/tmp/zedcomp-config-test")));
        assert_eq!(
            template::config_template_path(),
            Some(PathBuf::from("/tmp/zedcomp-config-test/template.cpp"))
        );

        // A blank override is "not configured": fall back to $HOME/.config/zedcomp.
        std::env::set_var(CONFIG_DIR_ENV, "   ");
        assert_eq!(env_var(CONFIG_DIR_ENV), None);

        // XDG_CONFIG_HOME is deliberately ignored (Zed's own config location).
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/zedcomp-xdg-must-be-ignored");
        if let Some(home) = env_var("HOME") {
            assert_eq!(
                config_dir(),
                Some(PathBuf::from(&home).join(".config").join("zedcomp"))
            );
        }

        std::env::remove_var(CONFIG_DIR_ENV);
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
