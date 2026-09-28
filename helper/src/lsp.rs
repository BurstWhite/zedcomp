//! Minimal LSP server over stdio.
//!
//! Only what ZedComp needs is implemented:
//! `initialize` (captures `rootUri` / `rootPath` / `initializationOptions.port`
//! / `initializationOptions.templatePath` / `initializationOptions.template`),
//! `initialized`, `shutdown`, `exit`; every other request is answered with a
//! `null` result and every other notification is ignored.
//!
//! Framing is the standard `Content-Length:` header set. `window/logMessage`
//! notifications are pushed from the HTTP thread while the session is active.

use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::template::{self, Template, TemplateSpec};

/// Port used when neither `ZEDCOMP_PORT` nor `initializationOptions.port` is set.
pub const DEFAULT_PORT: u16 = 27121;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Shared state between the LSP loop and the HTTP listener thread.
pub struct ServerState {
    port: AtomicU16,
    workspace_root: Mutex<Option<PathBuf>>,
    template_spec: Mutex<TemplateSpec>,
    initialized: AtomicBool,
    shutdown_requested: AtomicBool,
    stdout: Mutex<io::Stdout>,
}

impl ServerState {
    /// New state, using `port` as the initial HTTP port and the compiled-in
    /// default template until the client sends `initialize`.
    pub fn new(port: u16) -> Self {
        Self {
            port: AtomicU16::new(port),
            workspace_root: Mutex::new(None),
            template_spec: Mutex::new(TemplateSpec::default()),
            initialized: AtomicBool::new(false),
            shutdown_requested: AtomicBool::new(false),
            stdout: Mutex::new(io::stdout()),
        }
    }

    /// Port the HTTP listener should currently be bound to.
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    /// Change the desired HTTP port (the listener rebinds within ~250 ms).
    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    /// Workspace root advertised by the LSP client, if any.
    pub fn workspace_root(&self) -> Option<PathBuf> {
        self.workspace_root
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn set_workspace_root(&self, root: PathBuf) {
        let mut guard = self.workspace_root.lock().unwrap_or_else(|err| err.into_inner());
        *guard = Some(root);
    }

    /// Template selection advertised by the LSP client, if any.
    pub fn template_spec(&self) -> TemplateSpec {
        self.template_spec
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn set_template_spec(&self, spec: TemplateSpec) {
        let mut guard = self
            .template_spec
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *guard = spec;
    }

    /// Resolve the template to render `main.cpp` with.
    ///
    /// Resolution happens per problem fetch, so editing
    /// `~/.config/zedcomp/template.cpp` takes effect without restarting the
    /// helper. Unreadable `templatePath` values only warn (see
    /// [`template::resolve`]).
    pub fn resolve_template(&self) -> Template {
        template::resolve(&self.template_spec())
    }

    /// True once the client sent `initialized`.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }

    fn set_initialized(&self) {
        self.initialized.store(true, Ordering::Relaxed);
    }

    fn is_shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::Relaxed)
    }

    fn set_shutdown_requested(&self) {
        self.shutdown_requested.store(true, Ordering::Relaxed);
    }

    /// Write a raw framed JSON-RPC message to stdout.
    pub fn send(&self, message: &Value) {
        let body = message.to_string();
        let mut out = self.stdout.lock().unwrap_or_else(|err| err.into_inner());
        let _ = write!(
            out,
            "Content-Length: {}\r\n\r\n{}",
            body.as_bytes().len(),
            body
        );
        let _ = out.flush();
    }

    /// Send `window/logMessage` (in the LSP `MessageType` scale) when a session
    /// is active. Always mirrors the message to stderr for standalone runs.
    pub fn log_message(&self, message_type: u8, message: &str) {
        eprintln!("[zedcomp-helper] {message}");
        if !self.is_initialized() {
            return;
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": message_type, "message": message },
        }));
    }
}

/// Resolve the workspace root: `ZEDCOMP_WORKSPACE` wins over the LSP value.
pub fn resolve_workspace_root(state: &ServerState) -> Option<PathBuf> {
    if let Ok(value) = std::env::var("ZEDCOMP_WORKSPACE") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    state.workspace_root()
}

/// Resolve the HTTP port: `ZEDCOMP_PORT` wins over the compiled-in default.
pub fn port_from_env() -> u16 {
    match std::env::var("ZEDCOMP_PORT") {
        Ok(value) => match value.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                eprintln!(
                    "[zedcomp-helper] ignoring invalid ZEDCOMP_PORT={value:?}, using {DEFAULT_PORT}"
                );
                DEFAULT_PORT
            }
        },
        Err(_) => DEFAULT_PORT,
    }
}

/// Run the stdio LSP loop until EOF / `exit`. Returns the process exit code.
pub fn run_stdio(state: Arc<ServerState>) -> i32 {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    loop {
        match read_message(&mut reader) {
            Ok(Some(message)) => {
                if let Some(code) = handle_message(&state, &message) {
                    return code;
                }
            }
            Ok(None) => {
                // stdin closed: the editor is gone, so are we.
                return 0;
            }
            Err(err) => {
                eprintln!("[zedcomp-helper] LSP stream error: {err}");
                return 1;
            }
        }
    }
}

fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut header_bytes = 0usize;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            if header_bytes == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended inside LSP headers",
            ));
        }
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP headers too large",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }

    let length = content_length.unwrap_or(0);
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message too large",
        ));
    }
    let mut buffer = vec![0u8; length];
    reader.read_exact(&mut buffer)?;
    match serde_json::from_slice::<Value>(&buffer) {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            eprintln!("[zedcomp-helper] ignoring malformed LSP payload: {err}");
            Ok(Some(Value::Null))
        }
    }
}

/// Handle one message. `Some(code)` means "terminate with this exit code".
fn handle_message(state: &Arc<ServerState>, message: &Value) -> Option<i32> {
    let method = message.get("method").and_then(Value::as_str)?;
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            capture_initialize(state, &params);
            if let Some(id) = id {
                state.send(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "capabilities": {} },
                }));
            }
            None
        }
        "initialized" => {
            state.set_initialized();
            let workspace = resolve_workspace_root(state)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unset>".to_string());
            let spec = state.template_spec();
            let template = state.resolve_template();
            // Nudge towards the config file only when nothing is configured at
            // all and the compiled-in default is what problems will get.
            let hint = if spec.is_unset()
                && matches!(template.origin(), template::TemplateOrigin::Embedded)
            {
                match template::config_template_path() {
                    Some(path) => format!(" -- create {} to customize", path.display()),
                    None => String::new(),
                }
            } else {
                String::new()
            };
            state.log_message(
                3,
                &format!(
                    "ZedComp helper ready: http://127.0.0.1:{} (workspace: {workspace}, template: {}{hint})",
                    state.port(),
                    template.origin().describe()
                ),
            );
            None
        }
        "shutdown" => {
            state.set_shutdown_requested();
            if let Some(id) = id {
                state.send(&json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }));
            }
            None
        }
        "exit" => Some(if state.is_shutdown_requested() { 0 } else { 1 }),
        _ => {
            // Unknown request: answer `null` so the client is never left hanging.
            if let Some(id) = id {
                state.send(&json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }));
            }
            None
        }
    }
}

fn capture_initialize(state: &Arc<ServerState>, params: &Value) {
    if let Some(options) = params.get("initializationOptions") {
        let scoped = options.get("zedcomp").unwrap_or(options);
        if let Some(port) = extract_port(scoped.get("port").or_else(|| options.get("port"))) {
            state.set_port(port);
        }
        let root = scoped
            .get("workspaceRoot")
            .or_else(|| scoped.get("workspace_root"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(root) = root {
            if !root.trim().is_empty() {
                state.set_workspace_root(PathBuf::from(root.trim()));
            }
        }
    }

    // `templatePath` / `template` are resolved lazily at generation time so an
    // edited config template is picked up without restarting the helper.
    state.set_template_spec(TemplateSpec::from_options(
        params.get("initializationOptions"),
    ));

    let root_uri = params
        .get("rootUri")
        .and_then(Value::as_str)
        .or_else(|| params.get("rootPath").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| {
            params
                .get("workspaceFolders")
                .and_then(Value::as_array)
                .and_then(|folders| folders.first())
                .and_then(|folder| folder.get("uri"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    if let Some(uri) = root_uri {
        if let Some(path) = path_from_uri(&uri) {
            if !path.as_os_str().is_empty() {
                state.set_workspace_root(path);
            }
        }
    }
}

fn extract_port(value: Option<&Value>) -> Option<u16> {
    let value = value?;
    let port = match value {
        Value::Number(number) => number.as_u64()?,
        Value::String(text) => text.trim().parse::<u64>().ok()?,
        _ => return None,
    };
    if port == 0 || port > u16::MAX as u64 {
        return None;
    }
    Some(port as u16)
}

/// `file:///Users/me/ws` -> `/Users/me/ws` (percent-decoding + localhost host).
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(rest);
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[idx + 1..idx + 3]) {
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    idx += 3;
                    continue;
                }
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a POST should write when nothing else is known (standalone `serve`).
pub fn fallback_workspace_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_uris() {
        assert_eq!(
            path_from_uri("file:///Users/bw/Documents/ZedComp"),
            Some(PathBuf::from("/Users/bw/Documents/ZedComp"))
        );
        assert_eq!(
            path_from_uri("file://localhost/tmp/ws"),
            Some(PathBuf::from("/tmp/ws"))
        );
        assert_eq!(
            path_from_uri("file:///Users/bw/My%20Workspace"),
            Some(PathBuf::from("/Users/bw/My Workspace"))
        );
        assert_eq!(path_from_uri("https://example.com/ws"), None);
    }

    #[test]
    fn extracts_ports_from_numbers_and_strings() {
        assert_eq!(extract_port(Some(&json!(27121))), Some(27121));
        assert_eq!(extract_port(Some(&json!("54321"))), Some(54321));
        assert_eq!(extract_port(Some(&json!(0))), None);
        assert_eq!(extract_port(Some(&json!(70000))), None);
        assert_eq!(extract_port(None), None);
    }

    #[test]
    fn initialize_captures_root_and_port() {
        let state = Arc::new(ServerState::new(DEFAULT_PORT));
        let params = json!({
            "rootUri": "file:///tmp/zedcomp%20ws",
            "initializationOptions": { "port": 23456 }
        });
        capture_initialize(&state, &params);
        assert_eq!(state.port(), 23456);
        assert_eq!(
            state.workspace_root(),
            Some(PathBuf::from("/tmp/zedcomp ws"))
        );
    }

    #[test]
    fn initialize_falls_back_to_root_path_and_nested_options() {
        let state = Arc::new(ServerState::new(DEFAULT_PORT));
        let params = json!({
            "rootPath": "/tmp/rootpath",
            "initializationOptions": { "zedcomp": { "port": "1234", "workspaceRoot": "/tmp/explicit" } }
        });
        capture_initialize(&state, &params);
        assert_eq!(state.port(), 1234);
        assert_eq!(state.workspace_root(), Some(PathBuf::from("/tmp/explicit")));
    }

    #[test]
    fn initialize_captures_template_options() {
        let state = Arc::new(ServerState::new(DEFAULT_PORT));
        capture_initialize(
            &state,
            &json!({
                "initializationOptions": {
                    "templatePath": "/tmp/zedcomp-does-not-exist/template.cpp",
                    "template": "inline {{OJ}}\n"
                }
            }),
        );
        assert_eq!(
            state.template_spec().path.as_deref(),
            Some("/tmp/zedcomp-does-not-exist/template.cpp")
        );
        assert_eq!(state.template_spec().inline.as_deref(), Some("inline {{OJ}}\n"));
        // The unreadable path warns and falls back to the inline template.
        let resolved = state.resolve_template();
        assert_eq!(resolved.content(), "inline {{OJ}}\n");
        assert_eq!(resolved.origin(), &template::TemplateOrigin::OptionsInline);

        // No initializationOptions at all: nothing is configured.
        let bare = Arc::new(ServerState::new(DEFAULT_PORT));
        capture_initialize(&bare, &json!({ "rootPath": "/tmp/rootpath" }));
        assert!(bare.template_spec().is_unset());
    }

    #[test]
    fn read_message_handles_framing_and_eof() {
        let payload = br#"{"jsonrpc":"2.0","method":"exit"}"#;
        let mut raw = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
        raw.extend_from_slice(payload);
        let mut reader = BufReader::new(io::Cursor::new(raw));
        let message = read_message(&mut reader).unwrap().expect("one message");
        assert_eq!(message["method"], "exit");
        assert!(read_message(&mut reader).unwrap().is_none());
    }
}
