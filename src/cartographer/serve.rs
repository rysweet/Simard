//! A minimal, dependency-free static HTTP server for serving a built
//! dashboard package.
//!
//! `simard cartographer serve --out <dir>` binds a local port and serves the
//! package directory, defaulting `/` to `dashboard.html`. This is what makes the
//! deliverable a *served* interactive dashboard end-to-end. The server is
//! deliberately tiny (blocking, single-threaded) and locked to the package
//! directory: it refuses any request that would escape `out_dir`.

use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};

use super::error::{CartographerError, CartographerResult};

/// A bound dashboard server ready to accept connections.
pub struct DashboardServer {
    listener: TcpListener,
    root: PathBuf,
}

impl DashboardServer {
    /// Bind the server to `host:port`, serving files from `out_dir`.
    ///
    /// A `port` of `0` asks the OS for an ephemeral port (useful for tests);
    /// read the assigned port back via [`local_addr`](Self::local_addr).
    pub fn bind(out_dir: &Path, host: &str, port: u16) -> CartographerResult<Self> {
        let root = out_dir
            .canonicalize()
            .map_err(|e| CartographerError::io(format!("resolving {}", out_dir.display()), e))?;
        if !root.is_dir() {
            return Err(CartographerError::serve(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        let listener = TcpListener::bind((host, port))
            .map_err(|e| CartographerError::serve(format!("binding {host}:{port}: {e}")))?;
        Ok(Self { listener, root })
    }

    /// The concrete local address (and port) the server is bound to.
    pub fn local_addr(&self) -> CartographerResult<SocketAddr> {
        self.listener
            .local_addr()
            .map_err(|e| CartographerError::serve(format!("local_addr: {e}")))
    }

    /// The package directory being served.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Accept and handle exactly one connection. Returns after the response is
    /// written. Errors from a single client are surfaced to the caller.
    pub fn accept_and_handle(&self) -> CartographerResult<()> {
        let (stream, _peer) = self
            .listener
            .accept()
            .map_err(|e| CartographerError::serve(format!("accept: {e}")))?;
        handle_connection(stream, &self.root);
        Ok(())
    }

    /// Serve connections forever (until the process is interrupted).
    pub fn serve_forever(&self) -> CartographerResult<()> {
        for stream in self.listener.incoming() {
            match stream {
                Ok(stream) => handle_connection(stream, &self.root),
                Err(e) => eprintln!("cartographer serve: connection error: {e}"),
            }
        }
        Ok(())
    }
}

fn handle_connection(mut stream: TcpStream, root: &Path) {
    let request_line = match read_request_line(&mut stream) {
        Some(line) => line,
        None => return,
    };
    let target = parse_request_target(&request_line);

    match resolve(root, &target) {
        Some(path) => match std::fs::read(&path) {
            Ok(bytes) => write_response(&mut stream, 200, "OK", content_type(&path), &bytes),
            Err(_) => write_text(&mut stream, 404, "Not Found", "404 not found"),
        },
        None => write_text(&mut stream, 404, "Not Found", "404 not found"),
    }
}

fn read_request_line(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    // Cap the request line length to avoid unbounded reads from a hostile peer.
    let mut limited = (&mut reader).take(8 * 1024);
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match limited.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if byte[0] != b'\r' {
                    buf.push(byte[0]);
                }
                if buf.len() >= 8 * 1024 {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    line.push_str(&String::from_utf8_lossy(&buf));
    if line.is_empty() { None } else { Some(line) }
}

/// Extract and normalise the request target from a request line like
/// `GET /path?query HTTP/1.1`.
fn parse_request_target(request_line: &str) -> String {
    let mut parts = request_line.split_whitespace();
    let _method = parts.next();
    let raw = parts.next().unwrap_or("/");
    let path = raw.split(['?', '#']).next().unwrap_or("/");
    percent_decode(path)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve a request target to a real file inside `root`, or `None` if it does
/// not exist or would escape the package directory.
fn resolve(root: &Path, target: &str) -> Option<PathBuf> {
    let rel = target.trim_start_matches('/');
    let rel = if rel.is_empty() {
        "dashboard.html"
    } else {
        rel
    };

    // Reject any path component that is not a plain filename segment.
    let mut safe = PathBuf::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(seg) => safe.push(seg),
            // `.` is harmless; everything else (`..`, root, prefix) is rejected.
            Component::CurDir => {}
            _ => return None,
        }
    }

    let candidate = root.join(&safe);
    let canonical = candidate.canonicalize().ok()?;
    if !canonical.starts_with(root) {
        return None;
    }
    if canonical.is_file() {
        Some(canonical)
    } else {
        None
    }
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn write_response(stream: &mut TcpStream, status: u16, reason: &str, ctype: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn write_text(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    write_response(
        stream,
        status,
        reason,
        "text/plain; charset=utf-8",
        body.as_bytes(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_target_and_strips_query() {
        assert_eq!(
            parse_request_target("GET /a/b.html?x=1 HTTP/1.1"),
            "/a/b.html"
        );
        assert_eq!(parse_request_target("GET / HTTP/1.1"), "/");
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(percent_decode("/plain"), "/plain");
    }

    #[test]
    fn resolve_defaults_root_to_dashboard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dashboard.html"), b"<html></html>").unwrap();
        let root = dir.path().canonicalize().unwrap();
        let resolved = resolve(&root, "/").unwrap();
        assert!(resolved.ends_with("dashboard.html"));
    }

    #[test]
    fn resolve_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dashboard.html"), b"x").unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(resolve(&root, "/../../etc/passwd").is_none());
        assert!(resolve(&root, "/..%2f..%2fetc/passwd").is_none());
    }

    #[test]
    fn resolve_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(resolve(&root, "/nope.html").is_none());
    }

    #[test]
    fn content_type_by_extension() {
        assert_eq!(
            content_type(Path::new("a.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("a.json")),
            "application/json; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("a.bin")), "application/octet-stream");
    }

    #[test]
    fn server_serves_dashboard_over_http() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("dashboard.html"),
            b"<!DOCTYPE html><html>hello plotly</html>",
        )
        .unwrap();

        let server = DashboardServer::bind(dir.path(), "127.0.0.1", 0).unwrap();
        let addr = server.local_addr().unwrap();

        let handle = std::thread::spawn(move || {
            server.accept_and_handle().unwrap();
        });

        let mut client = TcpStream::connect(addr).unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();

        handle.join().unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("text/html"));
        assert!(response.contains("hello plotly"));
    }

    #[test]
    fn server_returns_404_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dashboard.html"), b"x").unwrap();
        let server = DashboardServer::bind(dir.path(), "127.0.0.1", 0).unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            server.accept_and_handle().unwrap();
        });
        let mut client = TcpStream::connect(addr).unwrap();
        client
            .write_all(b"GET /missing.html HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        handle.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn bind_rejects_non_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(DashboardServer::bind(&file, "127.0.0.1", 0).is_err());
    }
}
