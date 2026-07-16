//! A minimal, dependency-free static HTTP server for the dashboard package.
//!
//! `simard cartographer serve --out <dir>` serves the built `dashboard.html`
//! (and its sibling artifacts) over HTTP so the interactive dashboard can be
//! opened in a browser — the "served" half of the end-to-end contract. The
//! server is intentionally tiny: `GET` only, files resolved strictly inside the
//! package directory (no traversal), correct content types.
//!
//! `--self-check` binds an ephemeral port, serves a single request to itself,
//! verifies a `200` with a non-empty body, and returns — a fast, hang-free
//! proof that the package is servable, used by tests and CI.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

use serde::{Deserialize, Serialize};

use super::error::{CartographerError, CartographerResult};

/// The default index file served for `/`.
pub const INDEX_FILE: &str = "dashboard.html";

/// Result of a `serve --self-check` run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServeReport {
    pub addr: String,
    pub status: u16,
    pub body_bytes: usize,
    pub served_ok: bool,
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Percent-decode a URL path (enough for spaces / simple escapes).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(h) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(h);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve a request path to a file strictly inside `root`. Returns `None` when
/// the path escapes the root or names no component safely.
fn resolve_path(root: &Path, request_path: &str) -> Option<PathBuf> {
    let raw = request_path.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(raw);
    let rel = decoded.trim_start_matches('/');
    let rel = if rel.is_empty() { INDEX_FILE } else { rel };

    let mut resolved = root.to_path_buf();
    for comp in Path::new(rel).components() {
        use std::path::Component;
        match comp {
            Component::Normal(part) => resolved.push(part),
            // Any traversal / rooting is rejected outright.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            Component::CurDir => {}
        }
    }
    Some(resolved)
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Read the request line and serve one file. Returns the status code sent.
fn handle_connection(mut stream: TcpStream, root: &Path) -> std::io::Result<u16> {
    let mut buf = [0u8; 2048];
    let mut collected = Vec::new();
    // Read until we have the request line (terminated by CRLF).
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        collected.extend_from_slice(&buf[..n]);
        if collected.windows(2).any(|w| w == b"\r\n") || collected.len() > 8192 {
            break;
        }
    }
    let request = String::from_utf8_lossy(&collected);
    let first = request.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    if method != "GET" {
        write_response(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain",
            b"GET only",
        )?;
        return Ok(405);
    }

    match resolve_path(root, path) {
        Some(file) if file.is_file() => match std::fs::read(&file) {
            Ok(body) => {
                write_response(&mut stream, 200, "OK", content_type(&file), &body)?;
                Ok(200)
            }
            Err(_) => {
                write_response(
                    &mut stream,
                    500,
                    "Internal Server Error",
                    "text/plain",
                    b"read error",
                )?;
                Ok(500)
            }
        },
        _ => {
            write_response(&mut stream, 404, "Not Found", "text/plain", b"not found")?;
            Ok(404)
        }
    }
}

/// Perform a single `GET /` against `addr`, returning `(status, body_len)`.
fn self_request(addr: std::net::SocketAddr) -> CartographerResult<(u16, usize)> {
    let mut stream = TcpStream::connect(addr)
        .map_err(|e| CartographerError::serve(format!("self-check connect: {e}")))?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| CartographerError::serve(format!("self-check write: {e}")))?;
    let mut resp = Vec::new();
    stream
        .read_to_end(&mut resp)
        .map_err(|e| CartographerError::serve(format!("self-check read: {e}")))?;
    let text = String::from_utf8_lossy(&resp);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let body_len = text.split("\r\n\r\n").nth(1).map(|b| b.len()).unwrap_or(0);
    Ok((status, body_len))
}

/// Serve the package in `dir`.
///
/// * `self_check == true`: bind an ephemeral port (ignoring `port`), serve one
///   self-request, and return a [`ServeReport`].
/// * `self_check == false`: bind `127.0.0.1:port` and serve until the process is
///   terminated (this call blocks). `port == 0` picks an ephemeral port.
pub fn serve(dir: &Path, port: u16, self_check: bool) -> CartographerResult<ServeReport> {
    let index = dir.join(INDEX_FILE);
    if !index.is_file() {
        return Err(CartographerError::serve(format!(
            "no {INDEX_FILE} in {} — build the package first",
            dir.display()
        )));
    }

    if self_check {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| CartographerError::serve(format!("bind: {e}")))?;
        let addr = listener
            .local_addr()
            .map_err(|e| CartographerError::serve(format!("local_addr: {e}")))?;
        let client = thread::spawn(move || self_request(addr));
        let (stream, _) = listener
            .accept()
            .map_err(|e| CartographerError::serve(format!("accept: {e}")))?;
        let root = dir.to_path_buf();
        handle_connection(stream, &root)
            .map_err(|e| CartographerError::serve(format!("serve: {e}")))?;
        let (status, body_bytes) = client
            .join()
            .map_err(|_| CartographerError::serve("self-check thread panicked"))??;
        return Ok(ServeReport {
            addr: addr.to_string(),
            status,
            body_bytes,
            served_ok: status == 200 && body_bytes > 0,
        });
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| CartographerError::serve(format!("bind 127.0.0.1:{port}: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| CartographerError::serve(format!("local_addr: {e}")))?;
    println!(
        "cartographer: serving {} at http://{}/",
        dir.display(),
        addr
    );
    println!("  open http://{addr}/{INDEX_FILE} — press Ctrl-C to stop");
    let root = dir.to_path_buf();
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let _ = handle_connection(s, &root);
            }
            Err(e) => eprintln!("cartographer: connection error: {e}"),
        }
    }
    Ok(ServeReport {
        addr: addr.to_string(),
        status: 0,
        body_bytes: 0,
        served_ok: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pkg() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(INDEX_FILE),
            "<!DOCTYPE html><html><body>dashboard</body></html>",
        )
        .unwrap();
        std::fs::write(dir.path().join("data.json"), "{\"a\":1}").unwrap();
        dir
    }

    #[test]
    fn self_check_serves_the_dashboard() {
        let dir = write_pkg();
        let report = serve(dir.path(), 0, true).unwrap();
        assert_eq!(report.status, 200);
        assert!(report.body_bytes > 0);
        assert!(report.served_ok);
    }

    #[test]
    fn serve_errors_without_a_dashboard() {
        let dir = tempfile::tempdir().unwrap();
        let err = serve(dir.path(), 0, true).unwrap_err();
        assert!(matches!(err, CartographerError::Serve { .. }));
    }

    #[test]
    fn resolve_rejects_traversal() {
        let root = Path::new("/pkg");
        assert!(resolve_path(root, "/../etc/passwd").is_none());
        assert!(resolve_path(root, "/..%2f..%2fetc").is_none());
        assert_eq!(
            resolve_path(root, "/"),
            Some(PathBuf::from("/pkg/dashboard.html"))
        );
        assert_eq!(
            resolve_path(root, "/data.json?v=1"),
            Some(PathBuf::from("/pkg/data.json"))
        );
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
        assert_eq!(content_type(Path::new("a.csv")), "text/csv; charset=utf-8");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(percent_decode("/plain"), "/plain");
    }

    #[test]
    fn full_request_serves_missing_as_404() {
        // Bind, then in a client thread request a missing file.
        let dir = write_pkg();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"GET /nope.html HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).unwrap();
            resp
        });
        let (stream, _) = listener.accept().unwrap();
        let status = handle_connection(stream, dir.path()).unwrap();
        let resp = client.join().unwrap();
        assert_eq!(status, 404);
        assert!(resp.contains("404"));
    }
}
