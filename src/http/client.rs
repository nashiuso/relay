//! A small, blocking HTTP/1.1 client.
//!
//! The client is intentionally small: it speaks HTTP/1.1 over plain TCP or a
//! TLS pipe, understands `Content-Length` and `chunked` framing, keeps
//! connections alive between requests and enforces connect, read and write
//! deadlines. It is used by the provider adapters and by the optional
//! OpenAI-compatible server.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::{ErrorKind, RelayError};
use crate::json::Value;

use super::tls::{self, TlsConfig, TlsSession};
use super::url::{Scheme, Url};

const READ_BUFFER: usize = 16 * 1024;
const MAX_POOLED_CONNECTIONS: usize = 8;

/// Configuration for [`HttpClient`].
#[derive(Debug, Clone)]
pub struct HttpClientBuilder {
    connect_timeout: Duration,
    request_timeout: Duration,
    max_body_bytes: usize,
    user_agent: String,
    tls: TlsConfig,
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(120),
            max_body_bytes: 32 * 1024 * 1024,
            user_agent: concat!("relay-rs/", env!("CARGO_PKG_VERSION")).to_owned(),
            tls: TlsConfig::new(),
        }
    }
}

impl HttpClientBuilder {
    /// Sets the maximum time spent establishing a connection.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the timeout applied to a single request.
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Sets the maximum size of a buffered response body.
    pub fn max_body_bytes(mut self, bytes: usize) -> Self {
        self.max_body_bytes = bytes;
        self
    }

    /// Sets the `User-Agent` header sent with every request.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Sets the TLS configuration.
    pub fn tls(mut self, tls: TlsConfig) -> Self {
        self.tls = tls;
        self
    }

    /// Builds the client.
    pub fn build(self) -> HttpClient {
        HttpClient {
            inner: Arc::new(HttpClientInner {
                connect_timeout: self.connect_timeout,
                request_timeout: self.request_timeout,
                max_body_bytes: self.max_body_bytes,
                user_agent: self.user_agent,
                tls: self.tls,
                pool: Mutex::new(Vec::new()),
            }),
        }
    }
}

/// A reusable HTTP client.
///
/// Cloning a client shares the underlying connection pool.
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: Arc<HttpClientInner>,
}

#[derive(Debug)]
struct HttpClientInner {
    connect_timeout: Duration,
    request_timeout: Duration,
    max_body_bytes: usize,
    user_agent: String,
    tls: TlsConfig,
    pool: Mutex<Vec<PooledConnection>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    scheme: Scheme,
    host: String,
    port: u16,
}

impl HttpClient {
    /// Creates a client with the default settings.
    pub fn new() -> Self {
        HttpClientBuilder::default().build()
    }

    /// Creates a client builder.
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// The default request timeout of the client.
    pub fn request_timeout(&self) -> Duration {
        self.inner.request_timeout
    }

    /// Sends a request and returns the response.
    pub fn send(&self, request: &HttpRequest) -> Result<HttpResponse, RelayError> {
        let deadline = Instant::now() + self.inner.request_timeout;
        let key = PoolKey {
            scheme: request.url.scheme(),
            host: request.url.host().to_owned(),
            port: request.url.port(),
        };

        let mut conn = match self.take_pooled(&key) {
            Some(conn) => conn,
            None => self.connect(&request.url)?,
        };

        let raw = encode_request(request, &self.inner.user_agent);
        if let Err(err) = conn.write_all(&raw, deadline) {
            return Err(RelayError::provider(
                ErrorKind::Connection,
                request.url.host(),
                format!("failed to send the request: {err}"),
            ));
        }

        let status_line = match conn.read_line(deadline) {
            Ok(Some(line)) => line,
            Ok(None) => {
                return Err(RelayError::provider(
                    ErrorKind::Connection,
                    request.url.host(),
                    "the provider closed the connection before sending a response",
                ));
            }
            Err(err) => return Err(err),
        };
        let (status, reason) = parse_status_line(&status_line)?;
        let headers = read_headers(&mut conn, deadline)?;

        let connection_header = headers
            .get("connection")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        let reusable = !connection_header.contains("close") && request.url.scheme() == Scheme::Http
            || (request.url.scheme() == Scheme::Https && !connection_header.contains("close"));

        let mode = if request.method == "HEAD" || status == 204 || status == 304 {
            BodyMode::Length(0)
        } else if headers
            .get("transfer-encoding")
            .map(|value| value.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false)
        {
            BodyMode::Chunked { current: 0 }
        } else if let Some(length) = headers.get("content-length") {
            let length: u64 = length.trim().parse().map_err(|_| {
                RelayError::provider(
                    ErrorKind::Decode,
                    request.url.host(),
                    format!("invalid Content-Length `{length}`"),
                )
            })?;
            BodyMode::Length(length)
        } else {
            BodyMode::Eof
        };

        let body = Body {
            conn: Some(conn),
            key,
            pool: Arc::clone(&self.inner),
            mode,
            decoded: Vec::new(),
            decoded_pos: 0,
            line_buffer: Vec::new(),
            finished: false,
            reusable,
            idle_timeout: self.inner.request_timeout,
        };

        Ok(HttpResponse {
            status,
            reason,
            headers,
            body,
        })
    }

    fn take_pooled(&self, key: &PoolKey) -> Option<Conn> {
        let mut pool = self.inner.pool.lock().ok()?;
        let index = pool.iter().position(|entry| &entry.key == key)?;
        Some(pool.swap_remove(index).conn)
    }

    fn connect(&self, url: &Url) -> Result<Conn, RelayError> {
        let addresses = resolve(url.host(), url.port())?;
        let mut last_error: Option<io::Error> = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.inner.connect_timeout) {
                Ok(socket) => {
                    let _ = socket.set_nodelay(true);
                    return match url.scheme() {
                        Scheme::Http => Conn::plain(socket).map_err(|err| {
                            RelayError::provider(
                                ErrorKind::Connection,
                                url.host(),
                                format!("could not prepare the connection: {err}"),
                            )
                        }),
                        Scheme::Https => {
                            let session = tls::connect(url.host(), url.port(), &self.inner.tls)
                                .map_err(|err| {
                                    RelayError::provider(
                                        ErrorKind::Connection,
                                        url.host(),
                                        format!("TLS handshake failed: {err}"),
                                    )
                                })?;
                            Conn::tls(session).map_err(|err| {
                                RelayError::provider(
                                    ErrorKind::Connection,
                                    url.host(),
                                    format!("could not prepare the connection: {err}"),
                                )
                            })
                        }
                    };
                }
                Err(err) => last_error = Some(err),
            }
        }
        let detail = last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "no address found".to_owned());
        Err(RelayError::provider(
            ErrorKind::Connection,
            url.host(),
            format!("could not connect to {}: {detail}", url.authority()),
        ))
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// An outgoing HTTP request.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    method: String,
    url: Url,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    /// Creates a request for the given method and URL.
    pub fn new(method: &str, url: Url) -> Self {
        Self {
            method: method.to_ascii_uppercase(),
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Creates a `GET` request.
    pub fn get(url: Url) -> Self {
        Self::new("GET", url)
    }

    /// Creates a `POST` request.
    pub fn post(url: Url) -> Self {
        Self::new("POST", url)
    }

    /// Adds a header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the request body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// Sets a JSON body and the matching content type.
    pub fn json(self, value: &Value) -> Self {
        self.body(value.to_string().into_bytes())
            .header("Content-Type", "application/json")
    }

    /// The method of the request.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The URL of the request.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The headers of the request.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The body of the request.
    pub fn body_bytes(&self) -> &[u8] {
        &self.body
    }
}

/// A response header collection with case insensitive lookups.
#[derive(Debug, Clone, Default)]
pub struct Headers {
    entries: Vec<(String, String)>,
}

impl Headers {
    /// Returns the first value of `name`, ignoring case.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Iterates over all headers.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// An HTTP response.
pub struct HttpResponse {
    status: u16,
    reason: String,
    headers: Headers,
    body: Body,
}

impl HttpResponse {
    /// The status code of the response.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The reason phrase of the response.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The response headers.
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// Looks up a header value.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)
    }

    /// Returns `true` for 2xx status codes.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Reads the whole body as bytes.
    pub fn bytes(&mut self) -> Result<Vec<u8>, RelayError> {
        self.body.read_to_end()
    }

    /// Reads the whole body as UTF-8 text.
    pub fn text(&mut self) -> Result<String, RelayError> {
        let bytes = self.body.read_to_end()?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Takes ownership of the response body for incremental reading.
    pub fn into_body(self) -> Body {
        self.body
    }
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum BodyMode {
    Length(u64),
    Chunked { current: u64 },
    Eof,
}

/// A response body that can be read incrementally.
pub struct Body {
    conn: Option<Conn>,
    key: PoolKey,
    pool: Arc<HttpClientInner>,
    mode: BodyMode,
    decoded: Vec<u8>,
    decoded_pos: usize,
    line_buffer: Vec<u8>,
    finished: bool,
    reusable: bool,
    idle_timeout: Duration,
}

impl Body {
    /// Reads the next line of the body.
    ///
    /// The trailing newline is removed. `None` is returned at the end of the
    /// body.
    pub fn read_line(&mut self) -> Result<Option<String>, RelayError> {
        loop {
            if let Some(index) = find_newline(&self.decoded[self.decoded_pos..]) {
                let end = self.decoded_pos + index;
                self.line_buffer
                    .extend_from_slice(&self.decoded[self.decoded_pos..end]);
                self.decoded_pos = end + 1;
                let line = String::from_utf8_lossy(&self.line_buffer).into_owned();
                self.line_buffer.clear();
                return Ok(Some(line));
            }
            let available = self.decoded.len() - self.decoded_pos;
            self.line_buffer
                .extend_from_slice(&self.decoded[self.decoded_pos..]);
            self.decoded.clear();
            self.decoded_pos = 0;
            if !self.fill()? {
                if self.line_buffer.is_empty() && available == 0 {
                    return Ok(None);
                }
                let line = String::from_utf8_lossy(&self.line_buffer).into_owned();
                self.line_buffer.clear();
                return Ok(Some(line));
            }
        }
    }

    /// Reads the remaining body into a buffer.
    ///
    /// The read fails when the body exceeds the configured maximum size.
    pub fn read_to_end(&mut self) -> Result<Vec<u8>, RelayError> {
        let limit = self.pool.max_body_bytes;
        let mut out = Vec::new();
        while self.fill()? {
            let available = self.decoded.len() - self.decoded_pos;
            if out.len() + available > limit {
                return Err(RelayError::new(
                    ErrorKind::Decode,
                    format!("response body exceeds the {limit} byte limit"),
                ));
            }
            out.extend_from_slice(&self.decoded[self.decoded_pos..]);
            self.decoded.clear();
            self.decoded_pos = 0;
        }
        Ok(out)
    }

    /// Returns `true` when the body has been fully consumed.
    pub fn is_finished(&self) -> bool {
        self.finished && self.decoded_pos >= self.decoded.len()
    }

    /// Sets the maximum time a single read may block.
    ///
    /// Streaming responses use a short interval so that cancellation is
    /// noticed quickly.
    pub fn set_idle_timeout(&mut self, timeout: Duration) {
        self.idle_timeout = timeout;
    }

    fn fill(&mut self) -> Result<bool, RelayError> {
        if self.decoded_pos < self.decoded.len() {
            return Ok(true);
        }
        if self.finished {
            return Ok(false);
        }
        self.decoded.clear();
        self.decoded_pos = 0;

        enum Action {
            Finish,
            Data(usize),
            ChunkHeader,
        }

        let action = match self.mode {
            BodyMode::Length(0) => Action::Finish,
            BodyMode::Length(remaining) => Action::Data(remaining.min(READ_BUFFER as u64) as usize),
            BodyMode::Chunked { current } if current > 0 => {
                Action::Data(current.min(READ_BUFFER as u64) as usize)
            }
            BodyMode::Chunked { .. } => Action::ChunkHeader,
            BodyMode::Eof => Action::Data(READ_BUFFER),
        };

        match action {
            Action::Finish => {
                self.finish();
                Ok(false)
            }
            Action::Data(max) => {
                let chunk = {
                    let conn = self.conn.as_mut().ok_or_else(|| {
                        RelayError::stream("the response body was already consumed")
                    })?;
                    conn.read_some(max, self.idle_timeout)?
                };
                match chunk {
                    Some(data) => {
                        let read = data.len() as u64;
                        match &mut self.mode {
                            BodyMode::Length(remaining) => *remaining -= read,
                            BodyMode::Chunked { current } => *current -= read,
                            BodyMode::Eof => {}
                        }
                        self.decoded = data;
                        if matches!(self.mode, BodyMode::Chunked { current: 0 }) {
                            // Consume the CRLF that terminates the chunk.
                            let conn = self.conn.as_mut().ok_or_else(|| {
                                RelayError::stream("the response body was already consumed")
                            })?;
                            conn.read_exact(2, Instant::now() + self.idle_timeout)?;
                        }
                        Ok(true)
                    }
                    None => {
                        self.finish();
                        Ok(false)
                    }
                }
            }
            Action::ChunkHeader => {
                let line = {
                    let conn = self.conn.as_mut().ok_or_else(|| {
                        RelayError::stream("the response body was already consumed")
                    })?;
                    conn.read_line(Instant::now() + self.idle_timeout)?
                };
                let line = line.ok_or_else(|| {
                    RelayError::stream("the connection closed while reading a chunk header")
                })?;
                let size_text = line.split(';').next().unwrap_or_default().trim();
                let size = u64::from_str_radix(size_text, 16)
                    .map_err(|_| RelayError::stream(format!("invalid chunk size `{size_text}`")))?;
                if size == 0 {
                    loop {
                        let trailer = {
                            let conn = self.conn.as_mut().ok_or_else(|| {
                                RelayError::stream("the response body was already consumed")
                            })?;
                            conn.read_line(Instant::now() + self.idle_timeout)?
                        };
                        match trailer {
                            Some(line) if line.trim().is_empty() => break,
                            Some(_) => continue,
                            None => break,
                        }
                    }
                    self.finish();
                    Ok(false)
                } else {
                    self.mode = BodyMode::Chunked { current: size };
                    self.fill()
                }
            }
        }
    }

    fn finish(&mut self) {
        self.finished = true;
        if self.reusable {
            if let Some(conn) = self.conn.take() {
                if let Ok(mut pool) = self.pool.pool.lock() {
                    if pool.len() < MAX_POOLED_CONNECTIONS {
                        pool.push(PooledConnection {
                            key: self.key.clone(),
                            conn,
                        });
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body")
            .field("finished", &self.finished)
            .field("reusable", &self.reusable)
            .finish()
    }
}

#[derive(Debug)]
struct PooledConnection {
    key: PoolKey,
    conn: Conn,
}

/// A live connection to a server.
#[derive(Debug)]
struct Conn {
    rx: Receiver<io::Result<Vec<u8>>>,
    wtx: Sender<Vec<u8>>,
    socket: Option<TcpStream>,
    tls: Option<TlsSession>,
    buf: Vec<u8>,
    eof: bool,
}

impl Conn {
    fn plain(socket: TcpStream) -> io::Result<Conn> {
        let read_socket = socket.try_clone()?;
        let write_socket = socket.try_clone()?;

        let (data_tx, data_rx) = mpsc::channel::<io::Result<Vec<u8>>>();
        let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>();

        let mut read_socket = read_socket;
        std::thread::spawn(move || {
            let mut chunk = [0u8; READ_BUFFER];
            loop {
                match read_socket.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if data_tx.send(Ok(chunk[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = data_tx.send(Err(err));
                        break;
                    }
                }
            }
        });

        let mut write_socket = write_socket;
        std::thread::spawn(move || {
            while let Ok(data) = write_rx.recv() {
                if write_socket.write_all(&data).is_err() {
                    break;
                }
                let _ = write_socket.flush();
            }
            let _ = write_socket.shutdown(Shutdown::Write);
        });

        Ok(Conn {
            rx: data_rx,
            wtx: write_tx,
            socket: Some(socket),
            tls: None,
            buf: Vec::new(),
            eof: false,
        })
    }

    fn tls(session: TlsSession) -> io::Result<Conn> {
        let (data_tx, data_rx) = mpsc::channel::<io::Result<Vec<u8>>>();
        let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>();

        let mut session = session;
        let mut reader = session
            .take_reader()
            .ok_or_else(|| io::Error::other("the TLS reader was already taken"))?;
        let mut writer = session
            .take_writer()
            .ok_or_else(|| io::Error::other("the TLS writer was already taken"))?;

        std::thread::spawn(move || {
            let mut chunk = [0u8; READ_BUFFER];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if data_tx.send(Ok(chunk[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = data_tx.send(Err(err));
                        break;
                    }
                }
            }
        });

        std::thread::spawn(move || {
            while let Ok(data) = write_rx.recv() {
                if writer.write_all(&data).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });

        Ok(Conn {
            rx: data_rx,
            wtx: write_tx,
            socket: None,
            tls: Some(session),
            buf: Vec::new(),
            eof: false,
        })
    }

    fn write_all(&mut self, data: &[u8], deadline: Instant) -> Result<(), RelayError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RelayError::new(ErrorKind::Timeout, "the request timed out"));
        }
        let _ = remaining;
        self.wtx
            .send(data.to_vec())
            .map_err(|_| RelayError::new(ErrorKind::Connection, "the connection was closed"))
    }

    /// Reads at most `max` bytes, returning `None` at the end of the stream.
    fn read_some(&mut self, max: usize, timeout: Duration) -> Result<Option<Vec<u8>>, RelayError> {
        if !self.buf.is_empty() {
            let take = max.min(self.buf.len());
            let out: Vec<u8> = self.buf.drain(..take).collect();
            return Ok(Some(out));
        }
        if self.eof {
            return Ok(None);
        }
        match self.rx.recv_timeout(timeout) {
            Ok(Ok(data)) => {
                if data.is_empty() {
                    self.eof = true;
                    return Ok(None);
                }
                let take = max.min(data.len());
                let mut data = data;
                let out: Vec<u8> = data.drain(..take).collect();
                self.buf = data;
                Ok(Some(out))
            }
            Ok(Err(err)) => {
                let mut error = io_to_relay(err);
                if let Some(session) = &self.tls {
                    let diagnostics = session.diagnostics();
                    if !diagnostics.trim().is_empty() {
                        error = RelayError::new(
                            error.kind(),
                            format!("{} ({})", error.message(), diagnostics.trim()),
                        );
                    }
                }
                Err(error)
            }
            Err(RecvTimeoutError::Timeout) => Err(RelayError::new(
                ErrorKind::Timeout,
                "the provider did not answer in time",
            )),
            Err(RecvTimeoutError::Disconnected) => {
                self.eof = true;
                Ok(None)
            }
        }
    }

    /// Reads exactly `count` bytes.
    fn read_exact(&mut self, count: usize, deadline: Instant) -> Result<Vec<u8>, RelayError> {
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RelayError::new(ErrorKind::Timeout, "the request timed out"));
            }
            match self.read_some(count - out.len(), remaining)? {
                Some(data) => out.extend_from_slice(&data),
                None => {
                    return Err(RelayError::new(
                        ErrorKind::Decode,
                        "the connection closed before the response was complete",
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Reads a single line, without the trailing newline.
    fn read_line(&mut self, deadline: Instant) -> Result<Option<String>, RelayError> {
        loop {
            if let Some(index) = find_newline(&self.buf) {
                let line: Vec<u8> = self.buf.drain(..index + 1).collect();
                let text = String::from_utf8_lossy(&line[..line.len() - 1])
                    .trim_end_matches('\r')
                    .to_owned();
                return Ok(Some(text));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RelayError::new(ErrorKind::Timeout, "the request timed out"));
            }
            match self.read_some(READ_BUFFER, remaining)? {
                Some(data) => self.buf.extend_from_slice(&data),
                None => {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    let text = String::from_utf8_lossy(&self.buf).into_owned();
                    self.buf.clear();
                    return Ok(Some(text));
                }
            }
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

fn io_to_relay(err: io::Error) -> RelayError {
    let kind = match err.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => ErrorKind::Timeout,
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::NotConnected => ErrorKind::Connection,
        _ => ErrorKind::Io,
    };
    RelayError::new(kind, err.to_string())
}

fn find_newline(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|byte| *byte == b'\n')
}

fn parse_status_line(line: &str) -> Result<(u16, String), RelayError> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err(RelayError::new(
            ErrorKind::Decode,
            format!("invalid status line `{line}`"),
        ));
    }
    let status = parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            RelayError::new(ErrorKind::Decode, format!("invalid status line `{line}`"))
        })?;
    let reason = parts.next().unwrap_or_default().trim().to_owned();
    Ok((status, reason))
}

fn read_headers(conn: &mut Conn, deadline: Instant) -> Result<Headers, RelayError> {
    let mut entries = Vec::new();
    loop {
        let line = conn.read_line(deadline)?.ok_or_else(|| {
            RelayError::new(
                ErrorKind::Decode,
                "the connection closed before the response headers were complete",
            )
        })?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            entries.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    Ok(Headers { entries })
}

fn encode_request(request: &HttpRequest, user_agent: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(256 + request.body.len());
    out.extend_from_slice(
        format!(
            "{} {} HTTP/1.1\r\n",
            request.method,
            request.url.request_target()
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("Host: {}\r\n", request.url.authority()).as_bytes());
    out.extend_from_slice(format!("User-Agent: {user_agent}\r\n").as_bytes());
    out.extend_from_slice(b"Accept: */*\r\n");
    out.extend_from_slice(b"Connection: keep-alive\r\n");

    let mut has_content_length = false;
    for (name, value) in &request.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("connection") {
            continue;
        }
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    if !has_content_length && !request.body.is_empty() {
        out.extend_from_slice(format!("Content-Length: {}\r\n", request.body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&request.body);
    out
}

fn resolve(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, RelayError> {
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|err| {
            RelayError::new(
                ErrorKind::Connection,
                format!("could not resolve `{host}`: {err}"),
            )
        })?
        .collect();
    if addresses.is_empty() {
        return Err(RelayError::new(
            ErrorKind::Connection,
            format!("could not resolve `{host}`"),
        ));
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_request() {
        let url = Url::parse("http://example.com/v1/chat?x=1").unwrap();
        let request = HttpRequest::post(url)
            .header("Content-Type", "application/json")
            .body("hi");
        let raw = encode_request(&request, "relay-test");
        let text = String::from_utf8(raw).unwrap();
        assert!(text.starts_with("POST /v1/chat?x=1 HTTP/1.1\r\n"));
        assert!(text.contains("Host: example.com\r\n"));
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\nhi"));
    }

    #[test]
    fn parses_status_lines() {
        assert_eq!(
            parse_status_line("HTTP/1.1 200 OK").unwrap(),
            (200, "OK".to_owned())
        );
        assert_eq!(
            parse_status_line("HTTP/1.1 404 Not Found").unwrap(),
            (404, "Not Found".to_owned())
        );
        assert!(parse_status_line("garbage").is_err());
    }
}
