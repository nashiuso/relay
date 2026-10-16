//! Test helpers.
//!
//! The module provides a small mock HTTP server so that the provider
//! adapters can be tested against real HTTP traffic without contacting a
//! provider. It is part of the public API so that integration tests can use
//! it as well, but it is not covered by the stability guarantees of the
//! crate.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::json::Value;

/// A canned response of the [`MockServer`].
#[derive(Debug, Clone)]
pub struct MockResponse {
    /// The status code of the response.
    pub status: u16,
    /// The headers of the response.
    pub headers: Vec<(String, String)>,
    /// The body of the response.
    pub body: String,
    /// The body split into chunks that are written one after another.
    pub chunks: Vec<String>,
    /// The delay between two chunks.
    pub delay: Duration,
    /// The `Content-Length` announced in the header.
    ///
    /// When it is larger than the body, the client waits for data that never
    /// arrives. This is used to test timeouts.
    pub declared_length: Option<usize>,
}

impl MockResponse {
    /// Creates a response with the given status and body.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            chunks: Vec::new(),
            delay: Duration::from_millis(5),
            declared_length: None,
        }
    }

    /// Creates a `200 OK` response.
    pub fn ok(body: impl Into<String>) -> Self {
        Self::new(200, body)
    }

    /// Creates a `200 OK` response with a JSON content type.
    pub fn json(body: impl Into<String>) -> Self {
        Self::ok(body).with_header("Content-Type", "application/json")
    }

    /// Creates a response that is sent in chunks, one after another.
    pub fn streaming(chunks: Vec<String>) -> Self {
        let body = chunks.concat();
        Self {
            status: 200,
            headers: Vec::new(),
            body,
            chunks,
            delay: Duration::from_millis(5),
            declared_length: None,
        }
    }

    /// Creates a response that announces more bytes than it sends.
    pub fn partial(body: impl Into<String>, declared_length: usize) -> Self {
        let mut response = Self::ok(body);
        response.declared_length = Some(declared_length);
        response
    }

    /// Adds a header to the response.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the delay between chunks.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// A request the [`MockServer`] received.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// The HTTP method.
    pub method: String,
    /// The path of the request.
    pub path: String,
    /// The headers of the request.
    pub headers: Vec<(String, String)>,
    /// The body of the request.
    pub body: String,
}

impl RecordedRequest {
    /// Returns the value of a header, ignoring case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Parses the body as JSON.
    pub fn body_json(&self) -> Option<Value> {
        crate::json::parse(&self.body).ok()
    }
}

/// A mock HTTP server.
///
/// The server answers requests with the queued [`MockResponse`]s, in order.
/// When the queue is exhausted the last response is repeated.
#[derive(Debug)]
pub struct MockServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    /// Starts a server that answers with the given responses.
    pub fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server could not bind");
        let address = listener
            .local_addr()
            .expect("the mock server has no address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let thread_requests = Arc::clone(&requests);
        let thread_shutdown = Arc::clone(&shutdown);
        let index = Arc::new(AtomicUsize::new(0));
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let requests = Arc::clone(&thread_requests);
                let responses = responses.clone();
                let index = Arc::clone(&index);
                std::thread::spawn(move || {
                    let _ = serve_connection(stream, responses, requests, index);
                });
            }
        });

        Self {
            address,
            requests,
            shutdown,
            handle: Some(handle),
        }
    }

    /// The address the server listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Builds an absolute URL for a path.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    /// Every request the server received.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .map(|log| log.clone())
            .unwrap_or_default()
    }

    /// The number of requests the server received.
    pub fn request_count(&self) -> usize {
        self.requests.lock().map(|log| log.len()).unwrap_or(0)
    }

    /// The most recent request.
    pub fn last_request(&self) -> Option<RecordedRequest> {
        self.requests
            .lock()
            .ok()
            .and_then(|log| log.last().cloned())
    }

    /// Stops the server.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Unblock the accept loop with a throwaway connection.
        let _ = TcpStream::connect(self.address);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve_connection(
    stream: TcpStream,
    responses: Vec<MockResponse>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    index: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    loop {
        let Some(request) = read_request(&mut reader)? else {
            return Ok(());
        };
        if let Ok(mut log) = requests.lock() {
            log.push(request);
        }

        let response = if responses.is_empty() {
            MockResponse::new(500, "no response configured")
        } else {
            responses[index
                .fetch_add(1, Ordering::SeqCst)
                .min(responses.len() - 1)]
            .clone()
        };
        write_response(&mut writer, &response)?;
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<RecordedRequest>> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }
    if request_line.trim().is_empty() {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_owned();
            let value = value.trim().to_owned();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    Ok(Some(RecordedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }))
}

fn write_response(writer: &mut TcpStream, response: &MockResponse) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {} {}\r\n", response.status, reason);
    let mut has_content_type = false;
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !has_content_type {
        head.push_str("Content-Type: application/json\r\n");
    }
    let length = response.declared_length.unwrap_or(response.body.len());
    head.push_str(&format!("Content-Length: {length}\r\n"));
    head.push_str("Connection: keep-alive\r\n\r\n");
    writer.write_all(head.as_bytes())?;

    if response.chunks.is_empty() {
        writer.write_all(response.body.as_bytes())?;
    } else {
        for chunk in &response.chunks {
            writer.write_all(chunk.as_bytes())?;
            writer.flush()?;
            std::thread::sleep(response.delay);
        }
    }
    writer.flush()?;
    Ok(())
}
