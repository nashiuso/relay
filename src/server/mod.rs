//! The optional OpenAI compatible HTTP server.
//!
//! The server exposes the providers of the active configuration through a
//! small subset of the OpenAI API:
//!
//! * `POST /v1/chat/completions` – chat completions, with and without
//!   streaming,
//! * `GET /v1/models` – the models of the configured providers,
//! * `GET /health` – a health check.
//!
//! The server is behind the `server` feature flag so that the default
//! installation does not carry it. It binds to `127.0.0.1` unless the
//! configuration explicitly allows a remote interface, and it requires an
//! API key in that case.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::client::Client;
use crate::config::ServerConfig;
use crate::error::{ErrorKind, RelayError};
use crate::json::{Map, Value};
use crate::model::ModelRef;
use crate::types::{ChatRequest, Message, Role};

/// The maximum time a client may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Starts the server and serves requests until the process is stopped.
pub fn serve(client: Client, config: ServerConfig) -> Result<(), RelayError> {
    let address = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&address).map_err(|err| {
        RelayError::new(
            ErrorKind::Connection,
            format!("could not bind to {address}: {err}"),
        )
    })?;
    serve_on(client, config, listener)
}

/// Starts the server on an already bound listener.
///
/// Binding the listener separately makes it possible to use a random port in
/// tests.
pub fn serve_on(
    client: Client,
    config: ServerConfig,
    listener: TcpListener,
) -> Result<(), RelayError> {
    let local = listener.local_addr().map_err(|err| {
        RelayError::new(
            ErrorKind::Connection,
            format!("could not read the address: {err}"),
        )
    })?;
    let exposed = !is_loopback(&local.ip());
    if exposed {
        if !config.allow_remote {
            return Err(RelayError::config(format!(
                "refusing to bind to {local}: set `allow_remote = true` in the `[server]` \
                 section to allow clients from other machines"
            )));
        }
        if config.resolve_api_key().is_none() {
            return Err(RelayError::config(format!(
                "refusing to bind to {local} without an API key: set `api_key` or \
                 `api_key_env` in the `[server]` section"
            )));
        }
    }

    eprintln!("relay server listening on http://{local}");

    let client = Arc::new(client);
    let config = Arc::new(config);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let client = Arc::clone(&client);
        let config = Arc::clone(&config);
        std::thread::spawn(move || {
            if let Err(err) = handle_connection(stream, &client, &config) {
                eprintln!("relay server: {err}");
            }
        });
    }
    Ok(())
}

/// Returns `true` when the address points at the local machine.
fn is_loopback(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(addr) => addr.is_loopback(),
        IpAddr::V6(addr) => addr.is_loopback(),
    }
}

/// A parsed request.
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn handle_connection(
    stream: TcpStream,
    client: &Client,
    config: &ServerConfig,
) -> Result<(), RelayError> {
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|err| RelayError::new(ErrorKind::Io, err.to_string()))?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|err| RelayError::new(ErrorKind::Io, err.to_string()))?;

    let mut writer = stream
        .try_clone()
        .map_err(|err| RelayError::new(ErrorKind::Io, err.to_string()))?;
    let mut reader = BufReader::new(stream);

    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };

    if request.method == "OPTIONS" {
        let status = write_response(&mut writer, 204, "text/plain", "", config, &request)?;
        log_request(&request, status);
        return Ok(());
    }

    if let Some(api_key) = config.resolve_api_key() {
        let provided = request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim);
        if provided != Some(api_key.expose()) {
            let status = write_error(
                &mut writer,
                401,
                "invalid_api_key",
                "a valid bearer token is required",
                config,
                &request,
            )?;
            log_request(&request, status);
            return Ok(());
        }
    }

    let status = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => {
            let mut body = Map::new();
            body.insert("status", "ok");
            body.insert("version", crate::VERSION);
            write_response(
                &mut writer,
                200,
                "application/json",
                &Value::Object(body).to_string(),
                config,
                &request,
            )?
        }
        ("GET", "/v1/models") => {
            let mut data = Vec::new();
            for provider in client.providers() {
                match client.models(&provider.name) {
                    Ok(models) => {
                        for model in models {
                            let mut entry = Map::new();
                            entry.insert("id", format!("{}/{}", provider.name, model.id()));
                            entry.insert("object", "model");
                            entry.insert("owned_by", provider.name.clone());
                            data.push(Value::Object(entry));
                        }
                    }
                    Err(_) => continue,
                }
            }
            let mut body = Map::new();
            body.insert("object", "list");
            body.insert("data", Value::Array(data));
            write_response(
                &mut writer,
                200,
                "application/json",
                &Value::Object(body).to_string(),
                config,
                &request,
            )?
        }
        ("POST", "/v1/chat/completions") => {
            if request.body.len() > config.max_body_bytes {
                let status = write_error(
                    &mut writer,
                    413,
                    "request_too_large",
                    "the request body is too large",
                    config,
                    &request,
                )?;
                log_request(&request, status);
                return Ok(());
            }
            let payload = match crate::json::parse(&request.body) {
                Ok(payload) => payload,
                Err(err) => {
                    let status = write_error(
                        &mut writer,
                        400,
                        "invalid_request_error",
                        &format!("invalid JSON body: {err}"),
                        config,
                        &request,
                    )?;
                    log_request(&request, status);
                    return Ok(());
                }
            };
            let (chat, streaming) = match parse_chat_request(&payload) {
                Ok(parsed) => parsed,
                Err(err) => {
                    let status = write_error(
                        &mut writer,
                        400,
                        "invalid_request_error",
                        &err.to_string(),
                        config,
                        &request,
                    )?;
                    log_request(&request, status);
                    return Ok(());
                }
            };
            handle_chat(&mut writer, client, config, &request, chat, streaming)?
        }
        _ => write_error(
            &mut writer,
            404,
            "not_found",
            "the requested route does not exist",
            config,
            &request,
        )?,
    };
    log_request(&request, status);
    Ok(())
}

/// Writes one line about the request to the log.
///
/// The prompt, the answer and the credentials are never part of the line.
fn log_request(request: &Request, status: u16) {
    eprintln!("{} {} {}", request.method, request.path, status);
}

/// Parses an OpenAI style chat completion request.
fn parse_chat_request(payload: &Value) -> Result<(ChatRequest, bool), RelayError> {
    let model = payload
        .get("model")
        .and_then(|model| model.as_str())
        .ok_or_else(|| {
            RelayError::new(ErrorKind::Decode, "the request is missing a `model` field")
        })?;
    let model_ref = ModelRef::parse(model)?;

    let mut chat = ChatRequest::new(model_ref);
    let messages = payload
        .get("messages")
        .and_then(|messages| messages.as_array())
        .ok_or_else(|| {
            RelayError::new(
                ErrorKind::Decode,
                "the request is missing a `messages` array",
            )
        })?;
    for message in messages {
        let role = message
            .get("role")
            .and_then(|role| role.as_str())
            .and_then(Role::parse)
            .unwrap_or(Role::User);
        let content = message
            .get("content")
            .and_then(|content| content.as_str())
            .unwrap_or_default();
        chat = chat.message(role, content);
    }
    if let Some(temperature) = payload.get("temperature").and_then(|value| value.as_f64()) {
        chat = chat.temperature(temperature as f32);
    }
    if let Some(max_tokens) = payload.get("max_tokens").and_then(|value| value.as_i64()) {
        chat = chat.max_tokens(max_tokens.max(0) as u32);
    }
    if let Some(stop) = payload.get("stop").and_then(|value| value.as_array()) {
        for entry in stop {
            if let Some(text) = entry.as_str() {
                chat = chat.stop(text);
            }
        }
    }

    let streaming = payload
        .get("stream")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Ok((chat, streaming))
}

fn handle_chat(
    writer: &mut TcpStream,
    client: &Client,
    config: &ServerConfig,
    request: &Request,
    chat: ChatRequest,
    streaming: bool,
) -> Result<u16, RelayError> {
    let model_ref = chat.model().clone();
    if !streaming {
        let response = client.complete_blocking(chat)?;
        let mut body = Map::new();
        body.insert("id", format!("chatcmpl-{}", unique_id()));
        body.insert("object", "chat.completion");
        body.insert("created", now_seconds());
        body.insert("model", model_ref.to_string());
        let mut choice = Map::new();
        choice.insert("index", 0i64);
        let mut message = Map::new();
        message.insert("role", "assistant");
        message.insert("content", response.text());
        choice.insert("message", Value::Object(message));
        choice.insert(
            "finish_reason",
            response
                .finish_reason()
                .map(|reason| Value::string(reason.as_str()))
                .unwrap_or(Value::Null),
        );
        body.insert("choices", Value::Array(vec![Value::Object(choice)]));
        if let Some(usage) = response.usage() {
            let mut usage_object = Map::new();
            usage_object.insert(
                "prompt_tokens",
                usage.prompt_tokens.map(Value::from).unwrap_or(Value::Null),
            );
            usage_object.insert(
                "completion_tokens",
                usage
                    .completion_tokens
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            usage_object.insert(
                "total_tokens",
                usage.total_tokens.map(Value::from).unwrap_or(Value::Null),
            );
            body.insert("usage", Value::Object(usage_object));
        }
        return write_response(
            writer,
            200,
            "application/json",
            &Value::Object(body).to_string(),
            config,
            request,
        );
    }

    // Streaming: the headers are sent first, then one event per chunk.
    let mut stream = client.stream_blocking(chat)?;
    write_head(writer, 200, "text/event-stream", None, config, request)?;
    let id = format!("chatcmpl-{}", unique_id());
    let mut sent_any = false;
    for chunk in stream.by_ref() {
        let chunk = chunk?;
        if let crate::StreamChunk::Text(text) = chunk {
            sent_any = true;
            let payload = chunk_event(&id, &model_ref, Some(&text), None);
            write!(writer, "data: {payload}\n\n").map_err(io_error)?;
            writer.flush().map_err(io_error)?;
        }
    }
    if !sent_any {
        let payload = chunk_event(&id, &model_ref, Some(""), None);
        write!(writer, "data: {payload}\n\n").map_err(io_error)?;
    }
    let payload = chunk_event(&id, &model_ref, None, Some("stop"));
    write!(writer, "data: {payload}\n\n").map_err(io_error)?;
    write!(writer, "data: [DONE]\n\n").map_err(io_error)?;
    writer.flush().map_err(io_error)?;
    Ok(200)
}

/// Builds a streaming chunk in the OpenAI format.
fn chunk_event(
    id: &str,
    model: &ModelRef,
    content: Option<&str>,
    finish_reason: Option<&str>,
) -> String {
    let mut delta = Map::new();
    if let Some(content) = content {
        delta.insert("content", content);
    }
    let mut choice = Map::new();
    choice.insert("index", 0i64);
    choice.insert("delta", Value::Object(delta));
    choice.insert(
        "finish_reason",
        finish_reason.map(Value::from).unwrap_or(Value::Null),
    );
    let mut event = Map::new();
    event.insert("id", id);
    event.insert("object", "chat.completion.chunk");
    event.insert("created", now_seconds());
    event.insert("model", model.to_string());
    event.insert("choices", Value::Array(vec![Value::Object(choice)]));
    Value::Object(event).to_string()
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Result<Option<Request>, RelayError> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).map_err(io_error)? == 0 {
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
        if reader.read_line(&mut line).map_err(io_error)? == 0 {
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
        reader.read_exact(&mut body).map_err(io_error)?;
    }

    Ok(Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }))
}

fn write_error(
    writer: &mut TcpStream,
    status: u16,
    code: &str,
    message: &str,
    config: &ServerConfig,
    request: &Request,
) -> Result<u16, RelayError> {
    let mut body = Map::new();
    let mut error = Map::new();
    error.insert("message", message);
    error.insert("type", code);
    error.insert("code", code);
    body.insert("error", Value::Object(error));
    write_response(
        writer,
        status,
        "application/json",
        &Value::Object(body).to_string(),
        config,
        request,
    )
}

fn write_response(
    writer: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    config: &ServerConfig,
    request: &Request,
) -> Result<u16, RelayError> {
    write_head(
        writer,
        status,
        content_type,
        Some(body.len()),
        config,
        request,
    )?;
    writer.write_all(body.as_bytes()).map_err(io_error)?;
    writer.flush().map_err(io_error)?;
    Ok(status)
}

fn write_head(
    writer: &mut TcpStream,
    status: u16,
    content_type: &str,
    content_length: Option<usize>,
    config: &ServerConfig,
    request: &Request,
) -> Result<(), RelayError> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    head.push_str(&format!("Content-Type: {content_type}\r\n"));
    if let Some(length) = content_length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str("Connection: close\r\n");
    if content_type == "text/event-stream" {
        head.push_str("Cache-Control: no-cache\r\n");
    }
    if let Some(origin) = cors_origin(config, request) {
        head.push_str(&format!("Access-Control-Allow-Origin: {origin}\r\n"));
        head.push_str("Access-Control-Allow-Headers: authorization, content-type\r\n");
        head.push_str("Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n");
        head.push_str("Vary: Origin\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes()).map_err(io_error)?;
    writer.flush().map_err(io_error)?;
    Ok(())
}

/// Returns the allowed origin for a request, if cross origin access is
/// configured and the request origin is allowed.
fn cors_origin(config: &ServerConfig, request: &Request) -> Option<String> {
    if config.cors_allow_origins.is_empty() {
        return None;
    }
    let origin = request.header("origin")?;
    if config
        .cors_allow_origins
        .iter()
        .any(|allowed| allowed == "*" || allowed == origin)
    {
        Some(origin.to_owned())
    } else {
        None
    }
}

fn io_error(err: std::io::Error) -> RelayError {
    RelayError::new(ErrorKind::Io, err.to_string())
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn unique_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

/// Builds a message list from a JSON array, used by the tests.
#[allow(dead_code)]
fn messages_from_json(value: &Value) -> Vec<Message> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    Message::new(
                        item.get("role")
                            .and_then(|role| role.as_str())
                            .and_then(Role::parse)
                            .unwrap_or(Role::User),
                        item.get("content")
                            .and_then(|content| content.as_str())
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}
