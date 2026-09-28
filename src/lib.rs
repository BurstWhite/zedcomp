//! ZedComp — Zed extension glue for the native `zedcomp-helper` binary.
//!
//! A WASM extension cannot listen on a TCP port, so all of the Competitive
//! Companion work happens in the helper. Zed starts the helper as an
//! *additional* language server for C, C++ and Python: the helper speaks a
//! minimal LSP over stdio so Zed keeps it alive, and in parallel it listens on
//! `127.0.0.1:27121` for the `POST` requests sent by the Competitive Companion
//! browser extension.
//!
//! This module only has to answer one question: *which binary should Zed
//! spawn?* It looks, in order, at
//!
//! 1. `lsp.zedcomp-helper.binary.path` from the user's settings,
//! 2. `zedcomp-helper` on the user's `$PATH`,
//! 3. a helper previously downloaded into this extension's working directory,
//! 4. the newest matching asset of the `BurstWhite/zedcomp` GitHub release.
//!
//! The helper is told where the problem folders belong through the
//! `ZEDCOMP_WORKSPACE` environment variable, which we set to the root of the
//! worktree the language server was started for.

use zed_extension_api as zed;

/// Language server id; must match the key used in `extension.toml`.
const LANGUAGE_SERVER_ID: &str = "zedcomp-helper";
/// Name of the helper binary both on `$PATH` and inside release assets.
const BINARY_NAME: &str = "zedcomp-helper";
/// Repository that publishes prebuilt helper binaries.
const GITHUB_REPO: &str = "BurstWhite/zedcomp";
/// Environment variable the helper uses to resolve the workspace root.
const WORKSPACE_ENV_VAR: &str = "ZEDCOMP_WORKSPACE";

struct ZedCompExtension {
    /// Absolute path of a helper binary resolved during this session.
    cached_binary_path: Option<String>,
}

impl ZedCompExtension {
    /// The extension's working directory, which is also the directory
    /// `download_file` writes into and the process' current directory.
    fn work_dir() -> Option<std::path::PathBuf> {
        std::env::current_dir().ok()
    }

    /// The Rust target triple of the platform Zed is running on, used to pick
    /// the matching release asset (`zedcomp-helper-aarch64-apple-darwin`).
    #[allow(unreachable_patterns)]
    fn target_triple() -> Option<&'static str> {
        let (os, arch) = zed::current_platform();
        match (os, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => Some("aarch64-apple-darwin"),
            (zed::Os::Mac, zed::Architecture::X8664) => Some("x86_64-apple-darwin"),
            (zed::Os::Mac, zed::Architecture::X86) => Some("i686-apple-darwin"),
            (zed::Os::Linux, zed::Architecture::Aarch64) => Some("aarch64-unknown-linux-gnu"),
            (zed::Os::Linux, zed::Architecture::X8664) => Some("x86_64-unknown-linux-gnu"),
            (zed::Os::Linux, zed::Architecture::X86) => Some("i686-unknown-linux-gnu"),
            (zed::Os::Windows, zed::Architecture::Aarch64) => Some("aarch64-pc-windows-msvc"),
            (zed::Os::Windows, zed::Architecture::X8664) => Some("x86_64-pc-windows-msvc"),
            (zed::Os::Windows, zed::Architecture::X86) => Some("i686-pc-windows-msvc"),
            _ => None,
        }
    }

    /// File names a helper binary may have inside the extension working
    /// directory on this platform.
    fn binary_names() -> Vec<String> {
        let Some(triple) = Self::target_triple() else {
            return Vec::new();
        };

        let mut names = vec![format!("{BINARY_NAME}-{triple}")];
        if matches!(zed::current_platform().0, zed::Os::Windows) {
            names.push(format!("{BINARY_NAME}-{triple}.exe"));
        }
        names
    }

    /// Release asset names to look for, in order of preference. Compressed
    /// assets are accepted as a fallback and are extracted while downloading.
    fn asset_names() -> Vec<(String, zed::DownloadedFileType)> {
        let mut assets = Vec::new();
        for name in Self::binary_names() {
            assets.push((name.clone(), zed::DownloadedFileType::Uncompressed));
            assets.push((format!("{name}.gz"), zed::DownloadedFileType::Gzip));
        }
        assets
    }

    fn is_file(path: &std::path::Path) -> bool {
        std::fs::metadata(path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
    }

    /// Returns the absolute path of a helper binary that is already present in
    /// the extension's working directory, if there is one.
    fn find_downloaded_binary(&mut self) -> Option<String> {
        if let Some(path) = self.cached_binary_path.take() {
            if Self::is_file(std::path::Path::new(&path)) {
                self.cached_binary_path = Some(path.clone());
                return Some(path);
            }
        }

        let work_dir = Self::work_dir();
        for name in Self::binary_names() {
            let found = match &work_dir {
                Some(work_dir) => Self::is_file(&work_dir.join(&name)),
                None => Self::is_file(std::path::Path::new(&name)),
            };

            if found && zed::make_file_executable(&name).is_ok() {
                let path = match &work_dir {
                    Some(work_dir) => work_dir.join(&name).to_string_lossy().into_owned(),
                    None => name,
                };
                self.cached_binary_path = Some(path.clone());
                return Some(path);
            }
        }

        None
    }

    /// Downloads the helper binary matching the current platform from the
    /// newest GitHub release of `BurstWhite/zedcomp`.
    fn download_binary(
        &mut self,
        language_server_id: &zed::LanguageServerId,
    ) -> zed::Result<String> {
        let asset_names = Self::asset_names();

        if asset_names.is_empty() {
            return Err(Self::installation_failed(
                language_server_id,
                format!(
                    "ZedComp does not know which {BINARY_NAME} binary to use on this platform. \
                     Build the helper from source (`cargo install --path helper`) and make sure it \
                     is on your `$PATH`, or set `lsp.{LANGUAGE_SERVER_ID}.binary.path`."
                ),
            ));
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        );

        let release = match release {
            Ok(release) => release,
            Err(error) => {
                return Err(Self::installation_failed(
                    language_server_id,
                    format!("failed to fetch the latest ZedComp release from {GITHUB_REPO}: {error}"),
                ));
            }
        };

        let mut selected = None;
        for (asset_name, file_type) in &asset_names {
            if let Some(asset) = release.assets.iter().find(|asset| &asset.name == asset_name) {
                // `foo.gz` is decompressed into `foo`, so the file we end up
                // executing is the name without the compression suffix.
                let file_name = asset_name
                    .strip_suffix(".gz")
                    .unwrap_or(asset_name)
                    .to_string();
                selected = Some((asset.clone(), file_name, *file_type));
                break;
            }
        }

        let Some((asset, file_name, file_type)) = selected else {
            let looked_for = asset_names
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Self::installation_failed(
                language_server_id,
                format!(
                    "release {} of {GITHUB_REPO} has no prebuilt {BINARY_NAME} binary for this platform \
                     (looked for: {looked_for}). Build the helper from source with \
                     `cargo install --path helper`, or point `lsp.{LANGUAGE_SERVER_ID}.binary.path` at an \
                     existing binary in your settings.",
                    release.version
                ),
            ));
        };

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Downloading,
        );

        if let Err(error) = zed::download_file(&asset.download_url, &file_name, file_type) {
            return Err(Self::installation_failed(
                language_server_id,
                format!("failed to download {}: {error}", asset.download_url),
            ));
        }

        if let Err(error) = zed::make_file_executable(&file_name) {
            return Err(Self::installation_failed(
                language_server_id,
                format!("failed to make {file_name} executable: {error}"),
            ));
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );

        let path = match Self::work_dir() {
            Some(work_dir) => work_dir.join(&file_name).to_string_lossy().into_owned(),
            None => file_name,
        };
        self.cached_binary_path = Some(path.clone());
        Ok(path)
    }

    fn installation_failed(
        language_server_id: &zed::LanguageServerId,
        message: String,
    ) -> String {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Failed(message.clone()),
        );
        message
    }
}

impl zed::Extension for ZedCompExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        // The helper generates problem folders relative to the worktree Zed
        // started it for, and inherits the user's shell environment so it can
        // find `zed`, `g++`, and friends.
        let mut env = worktree.shell_env();
        env.push((WORKSPACE_ENV_VAR.to_string(), worktree.root_path()));

        let mut args = Vec::new();

        // 0. Explicit user configuration:
        //    `"lsp": { "zedcomp-helper": { "binary": { "path": "..." } } }`
        if let Ok(settings) =
            zed::settings::LspSettings::for_worktree(language_server_id.as_ref(), worktree)
        {
            if let Some(binary) = settings.binary {
                if let Some(command) = binary.path {
                    if let Some(binary_args) = binary.arguments {
                        args = binary_args;
                    }
                    if let Some(binary_env) = binary.env {
                        env.extend(binary_env);
                    }
                    return Ok(zed::Command {
                        command,
                        args,
                        env,
                    });
                }
            }
        }

        // 1. A helper installed by the user (`cargo install --path helper`,
        //    the release archive, Homebrew, ...).
        if let Some(command) = worktree.which(BINARY_NAME) {
            return Ok(zed::Command {
                command,
                args,
                env,
            });
        }

        // 2. A helper downloaded earlier into this extension's working
        //    directory, 3. otherwise the newest matching release asset.
        let command = match self.find_downloaded_binary() {
            Some(command) => command,
            None => self.download_binary(language_server_id)?,
        };

        Ok(zed::Command {
            command,
            args,
            env,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<Option<zed::serde_json::Value>> {
        // Users may pin the Competitive Companion port, e.g.
        // `{ "lsp": { "zedcomp-helper": { "initialization_options": { "port": 27121 } } } }`.
        // When unset the helper falls back to `ZEDCOMP_PORT` and then to 27121.
        let settings =
            zed::settings::LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        Ok(settings.initialization_options)
    }
}

zed::register_extension!(ZedCompExtension);
