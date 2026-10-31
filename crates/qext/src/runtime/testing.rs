//! Test-only HTTP fixture server.
//!
//! Kept in one place because both the HTTP client tests and the JavaScript
//! `fetch` polyfill tests need a real socket to talk to. It deliberately uses
//! only the standard library: a dev-dependency that pulls a web framework into
//! a sandbox test would be more surface than the fixture is worth.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Read one shipped fixture package source.
///
/// Every test that needs a real package reads the same bytes the registry
/// serves, so a divergence found in a test is a divergence in production.
pub fn fixture_source(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("sources")
        .join(format!("{name}.index.js"));

    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read fixture {}: {error}", path.display()))
}

/// One parsed request, as the fixture handler sees it.
pub struct TestRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl TestRequest {
    /// Header lookup is case-insensitive because HTTP names are.
    pub fn header(&self, name: &str) -> Option<&str> {
        let wanted = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == wanted)
            .map(|(_, value)| value.as_str())
    }
}

/// Status, headers, body.
pub type TestResponse = (u16, Vec<(String, String)>, Vec<u8>);

/// Start a fixture server on a random loopback port and return its base URL.
///
/// The listener thread is detached on purpose: it lives for the test process,
/// which is the simplest way to keep a std-only server without a shutdown
/// handshake.
pub fn spawn_server<F>(handler: F) -> String
where
    F: Fn(&TestRequest) -> TestResponse + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = listener.local_addr().expect("local addr");
    let handler = Arc::new(handler);

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let request = match read_test_request(&mut stream) {
                Ok(request) => request,
                Err(_) => continue,
            };
            let (status, headers, body) = handler(&request);
            let _ = write_test_response(&mut stream, status, &headers, &body);
        }
    });

    format!("http://{address}")
}

fn read_test_request(stream: &mut TcpStream) -> std::io::Result<TestRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];

    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }

    let head_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(buffer.len());

    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);

    let mut body = buffer[head_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }

    Ok(TestRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_test_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        303 => "See Other",
        404 => "Not Found",
        _ => "Status",
    };

    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Connection: close\r\n\r\n");

    stream.write_all(response.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}
