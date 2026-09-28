//! Tiny hand-written HTTP/1.1 server for Competitive Companion.
//!
//! Only `POST /` carries meaning: the CC payload is turned into a workspace and
//! answered with `200` and an empty body. `OPTIONS` (CORS preflight) is answered
//! with `204`. Everything else gets a plain-text error status.
//!
//! The listener is deliberately dependency-free (`std::net::TcpListener`) and
//! rebinds whenever `ServerState::port()` changes, so
//! `initializationOptions.port` can override the startup port.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::cc;
use crate::lsp;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_POLL: Duration = Duration::from_millis(20);
const REBIND_POLL: Duration = Duration::from_millis(250);

/// Start the HTTP listener on a background thread.
pub fn spawn(state: Arc<lsp::ServerState>) {
    let builder = thread::Builder::new().name("zedcomp-http".to_string());
    if let Err(err) = builder.spawn(move || serve_loop(state)) {
        eprintln!("[zedcomp-helper] could not start HTTP thread: {err}");
    }
}

fn serve_loop(state: Arc<lsp::ServerState>) {
    let mut listener: Option<TcpListener> = None;
    let mut bound_port: u16 = 0;
    let mut failed_port: Option<u16> = None;

    loop {
        let wanted = state.port();
        if listener.is_none() || bound_port != wanted {
            listener = None;
            bound_port = 0;
            match TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, wanted))) {
                Ok(socket) => {
                    let _ = socket.set_nonblocking(true);
                    eprintln!(
                        "[zedcomp-helper] listening for Competitive Companion on http://127.0.0.1:{wanted}"
                    );
                    failed_port = None;
                    listener = Some(socket);
                    bound_port = wanted;
                }
                Err(err) => {
                    if failed_port != Some(wanted) {
                        eprintln!(
                            "[zedcomp-helper] cannot bind 127.0.0.1:{wanted}: {err}; \
                             continuing as LSP-only server (no HTTP intake)"
                        );
                        failed_port = Some(wanted);
                    }
                    thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            }
        }

        match listener.as_ref() {
            Some(socket) => match socket.accept() {
                Ok((stream, _peer)) => {
                    let state = Arc::clone(&state);
                    let _ = thread::Builder::new()
                        .name("zedcomp-http-conn".to_string())
                        .spawn(move || {
                            if let Err(err) = handle_connection(stream, &state) {
                                eprintln!("[zedcomp-helper] HTTP connection error: {err}");
                            }
                        });
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(IDLE_POLL);
                }
                Err(err) => {
                    eprintln!("[zedcomp-helper] accept failed: {err}");
                    thread::sleep(REBIND_POLL);
                }
            },
            None => thread::sleep(REBIND_POLL),
        }
    }
}

#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}

/// Request line + headers, before the body is read.
#[derive(Debug)]
struct RequestHead {
    method: String,
    target: String,
    content_length: usize,
    expect_continue: bool,
}

fn handle_connection(mut stream: TcpStream, state: &Arc<lsp::ServerState>) -> io::Result<()> {
    // On macOS/BSD an accepted socket inherits O_NONBLOCK from the listener,
    // which would turn a slow/segmented body into a spurious EAGAIN. Force
    // blocking reads and rely on the timeouts below instead.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);

    let head = match read_head(&mut reader) {
        Ok(head) => head,
        Err(err) => {
            let status = if err.kind() == io::ErrorKind::InvalidData {
                "413 Payload Too Large"
            } else {
                "400 Bad Request"
            };
            respond(&mut stream, status, err.to_string().as_bytes(), Some("text/plain"))?;
            return Ok(());
        }
    };

    // curl adds `Expect: 100-continue` for bodies above ~1 KiB; answering the
    // interim response immediately avoids its one-second stall.
    if head.expect_continue && head.content_length > 0 {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        let _ = stream.flush();
    }

    let request = match read_body(&mut reader, head.content_length) {
        Ok(body) => Request {
            method: head.method,
            target: head.target,
            body,
        },
        Err(err) => {
            respond(&mut stream, "400 Bad Request", err.to_string().as_bytes(), Some("text/plain"))?;
            return Ok(());
        }
    };

    if request.method.eq_ignore_ascii_case("OPTIONS") {
        respond(&mut stream, "204 No Content", b"", None)?;
        return Ok(());
    }
    if !request.method.eq_ignore_ascii_case("POST") {
        respond(
            &mut stream,
            "405 Method Not Allowed",
            b"zedcomp-helper: only POST / is supported\n",
            Some("text/plain"),
        )?;
        return Ok(());
    }

    let path = request.target.split(['?', '#']).next().unwrap_or("");
    if !(path.is_empty() || path == "/") {
        respond(
            &mut stream,
            "404 Not Found",
            b"zedcomp-helper: POST /\n",
            Some("text/plain"),
        )?;
        return Ok(());
    }

    let raw_body = match std::str::from_utf8(&request.body) {
        Ok(text) => text,
        Err(_) => {
            respond(
                &mut stream,
                "400 Bad Request",
                b"payload is not valid UTF-8\n",
                Some("text/plain"),
            )?;
            return Ok(());
        }
    };

    let problem = match cc::CcProblem::parse(raw_body) {
        Ok(problem) => problem,
        Err(err) => {
            eprintln!("[zedcomp-helper] rejected payload: {err}");
            respond(
                &mut stream,
                "400 Bad Request",
                format!("invalid Competitive Companion payload: {err}\n").as_bytes(),
                Some("text/plain"),
            )?;
            return Ok(());
        }
    };

    let info = match cc::parse_url(&problem.url()) {
        Some(info) => info,
        None => {
            let message = format!(
                "cannot derive an OJ/problem from url {:?}\n",
                problem.url()
            );
            eprintln!("[zedcomp-helper] {message}");
            respond(&mut stream, "400 Bad Request", message.as_bytes(), Some("text/plain"))?;
            return Ok(());
        }
    };

    let root = match lsp::resolve_workspace_root(state) {
        Some(root) => root,
        None => {
            let root = lsp::fallback_workspace_root();
            eprintln!(
                "[zedcomp-helper] no ZEDCOMP_WORKSPACE / LSP rootUri; falling back to {}",
                root.display()
            );
            root
        }
    };

    let tests = problem.tests();
    let outcome = match cc::generate(&root, &info, &problem, &tests) {
        Ok(outcome) => outcome,
        Err(err) => {
            let message = format!("failed to create {}: {err}\n", info.relative_dir().display());
            eprintln!("[zedcomp-helper] {message}");
            respond(
                &mut stream,
                "500 Internal Server Error",
                message.as_bytes(),
                Some("text/plain"),
            )?;
            return Ok(());
        }
    };

    // 200 with an empty body, then open the file / notify out of band.
    respond(&mut stream, "200 OK", b"", None)?;

    let group = problem
        .group()
        .map(|group| format!(" ({group})"))
        .unwrap_or_default();
    let interactive = if problem.interactive() { " [interactive]" } else { "" };
    let summary = format!(
        "{}{group}{interactive}: {} test(s), {} ms / {} MB -> {} [{}]",
        info.describe(),
        outcome.test_count,
        problem.time_limit_ms(),
        problem.memory_limit_mb(),
        outcome.dir.display(),
        if outcome.wrote_main {
            "created main.cpp"
        } else {
            "kept existing main.cpp"
        }
    );
    let background_state = Arc::clone(state);
    let main_cpp = outcome.main_cpp.clone();
    let _ = thread::Builder::new()
        .name("zedcomp-open".to_string())
        .spawn(move || {
            open_in_zed(&main_cpp);
            background_state.log_message(3, &format!("ZedComp: {summary}"));
        });

    Ok(())
}

fn read_head(reader: &mut impl BufRead) -> io::Result<RequestHead> {
    let mut header_bytes = 0usize;
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "empty request"));
    }
    if request_line.len() > MAX_HEADER_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "request line too long"));
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "malformed request line"));
    }

    let mut content_length: Option<usize> = None;
    let mut expect_continue = false;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "headers too large"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            } else if key.eq_ignore_ascii_case("expect") {
                expect_continue = value.trim().eq_ignore_ascii_case("100-continue");
            }
        }
    }

    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }

    Ok(RequestHead {
        method,
        target,
        content_length,
        expect_continue,
    })
}

fn read_body(reader: &mut impl BufRead, length: usize) -> io::Result<Vec<u8>> {
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body)?;
    }
    Ok(body)
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    body: &[u8],
    content_type: Option<&str>,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\
         Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n",
        body.len()
    );
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Best-effort `zed <path>`; failures are ignored (Zed may not be installed).
fn open_in_zed(path: &Path) {
    if std::env::var("ZEDCOMP_NO_OPEN").map(|value| value == "1").unwrap_or(false) {
        return;
    }
    let candidates: Vec<String> = std::env::var("ZEDCOMP_ZED_CLI")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .into_iter()
        .chain(
            ["zed", "/Applications/Zed.app/Contents/MacOS/cli"]
                .iter()
                .map(|value| value.to_string()),
        )
        .collect();

    for candidate in candidates {
        match Command::new(&candidate)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => return,
            Ok(status) => {
                eprintln!("[zedcomp-helper] `{candidate}` exited with {status}; trying next");
            }
            Err(_) => continue,
        }
    }
    eprintln!(
        "[zedcomp-helper] could not open {} in Zed (set ZEDCOMP_ZED_CLI to override)",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(raw: &str) -> io::Result<Request> {
        let mut reader = BufReader::new(io::Cursor::new(raw.as_bytes().to_vec()));
        let head = read_head(&mut reader)?;
        let body = read_body(&mut reader, head.content_length)?;
        Ok(Request {
            method: head.method,
            target: head.target,
            body,
        })
    }

    fn head(raw: &str) -> io::Result<RequestHead> {
        let mut reader = BufReader::new(io::Cursor::new(raw.as_bytes().to_vec()));
        read_head(&mut reader)
    }

    #[test]
    fn parses_post_with_content_length() {
        let raw = "POST / HTTP/1.1\r\nHost: 127.0.0.1:27121\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\r\n{\"name\":\"x\"}";
        let parsed = request(raw).expect("request parses");
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.target, "/");
        assert_eq!(parsed.body, b"{\"name\":\"x\"}");
    }

    #[test]
    fn parses_options_without_body() {
        let raw = "OPTIONS / HTTP/1.1\r\nOrigin: chrome-extension://x\r\n\r\n";
        let parsed = request(raw).expect("request parses");
        assert_eq!(parsed.method, "OPTIONS");
        assert!(parsed.body.is_empty());
    }

    #[test]
    fn detects_expect_continue() {
        let raw = "POST / HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\nbody";
        let parsed = head(raw).expect("head parses");
        assert!(parsed.expect_continue);
        assert_eq!(parsed.content_length, 4);

        let plain = head("POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n").expect("head parses");
        assert!(!plain.expect_continue);
    }

    #[test]
    fn rejects_oversized_body_advertisement() {
        let raw = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        let err = head(&raw).expect_err("must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
