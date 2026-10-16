//! How to add a provider of your own.
//!
//! Relay talks to backends through the [`ChatProvider`] trait. Implementing
//! it is enough to make a new backend available to the client, the command
//! line interface and the HTTP server.
//!
//! The provider in this example speaks a small JSON protocol. The example
//! documents the contract it expects, so that it can be pointed at a real
//! service:
//!
//! ```text
//! POST {base_url}/chat
//! {"model": "demo-model", "messages": [{"role": "user", "content": "..."}]}
//!
//! -> {"reply": "the answer", "usage": {"prompt_tokens": 3, "completion_tokens": 4}}
//! ```
//!
//! With `"stream": true` the backend answers with one JSON object per line:
//!
//! ```text
//! {"text": "the "}
//! {"text": "answer"}
//! {"done": true}
//! ```
//!
//! The HTTP transport of Relay lives in `relay_rs::http`. It is public so
//! that custom providers can use it, but it is not part of the stable API.

use std::sync::Arc;

use relay_rs::error::{ErrorKind, RelayError};
use relay_rs::http::{HttpClient, HttpRequest, Url};
use relay_rs::json::{self, Map, Value};
use relay_rs::model::ModelInfo;
use relay_rs::providers::traits::{Capabilities, ChatProvider, ProviderKind};
use relay_rs::stream::{ChatStream, StreamChunk};
use relay_rs::types::{ChatRequest, ChatResponse, FinishReason, Usage};

/// A provider for a backend that speaks the protocol described above.
#[derive(Debug)]
struct DemoProvider {
    name: String,
    base_url: String,
    http: HttpClient,
}

impl DemoProvider {
    fn new(name: &str, base_url: &str) -> Result<Self, RelayError> {
        Ok(Self {
            name: name.to_owned(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            http: HttpClient::builder()
                .request_timeout(std::time::Duration::from_secs(60))
                .build(),
        })
    }

    /// Builds the request body for a chat request.
    fn payload(request: &ChatRequest, streaming: bool) -> Value {
        let mut messages = Vec::new();
        for message in request.messages() {
            let mut entry = Map::new();
            entry.insert("role", message.role().as_str());
            entry.insert("content", message.content());
            messages.push(Value::Object(entry));
        }
        let mut body = Map::new();
        body.insert("model", request.model().model().to_owned());
        body.insert("messages", Value::Array(messages));
        body.insert("stream", streaming);
        Value::Object(body)
    }

    /// Sends a request to the backend.
    fn post(&self, body: &Value) -> Result<(u16, String), RelayError> {
        let url = Url::parse(&format!("{}/chat", self.base_url))
            .map_err(|err| RelayError::new(ErrorKind::Config, format!("invalid URL: {err}")))?;
        let request = HttpRequest::post(url)
            .header("Content-Type", "application/json")
            .body(body.to_string());
        let mut response = self
            .http
            .send(&request)
            .map_err(|err| err.with_provider(&self.name))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        Ok((status, text))
    }
}

impl ChatProvider for DemoProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Custom
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            model_listing: false,
            system_messages: true,
            tools: false,
        }
    }

    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError> {
        let (status, text) = self.post(&Self::payload(request, false))?;
        if !(200..300).contains(&status) {
            return Err(RelayError::new(
                ErrorKind::Server,
                format!("the backend answered with status {status}"),
            )
            .with_provider(&self.name));
        }
        let value = json::parse(&text).map_err(|err| {
            RelayError::new(ErrorKind::Decode, format!("invalid answer: {err}"))
                .with_provider(&self.name)
        })?;
        let reply = value
            .get("reply")
            .and_then(|reply| reply.as_str())
            .ok_or_else(|| {
                RelayError::new(ErrorKind::Decode, "the answer has no `reply` field")
                    .with_provider(&self.name)
            })?;
        let usage = value.get("usage").map(|usage| {
            let prompt = usage.get("prompt_tokens").and_then(|value| value.as_i64());
            let completion = usage
                .get("completion_tokens")
                .and_then(|value| value.as_i64());
            Usage {
                prompt_tokens: prompt.map(|value| value as u32),
                completion_tokens: completion.map(|value| value as u32),
                total_tokens: Some(
                    prompt.unwrap_or_default() as u32 + completion.unwrap_or_default() as u32,
                ),
            }
        });
        Ok(
            ChatResponse::new(&self.name, request.model().model(), reply)
                .with_finish_reason(Some(FinishReason::Stop))
                .with_usage(usage),
        )
    }

    fn stream(&self, request: &ChatRequest) -> Result<ChatStream, RelayError> {
        let (status, text) = self.post(&Self::payload(request, true))?;
        if !(200..300).contains(&status) {
            return Err(RelayError::new(
                ErrorKind::Server,
                format!("the backend answered with status {status}"),
            )
            .with_provider(&self.name));
        }

        let (mut sender, stream) = ChatStream::channel();
        let provider = self.name.clone();
        std::thread::spawn(move || {
            for line in text.lines() {
                if sender.is_cancelled() {
                    return;
                }
                let Ok(value) = json::parse(line) else {
                    let _ = sender.fail(RelayError::new(
                        ErrorKind::Decode,
                        format!("invalid stream line: {line}"),
                    ));
                    return;
                };
                if value.get("done").and_then(|done| done.as_bool()) == Some(true) {
                    break;
                }
                if let Some(part) = value.get("text").and_then(|part| part.as_str()) {
                    if sender.send(StreamChunk::Text(part.to_owned())).is_err() {
                        return;
                    }
                }
            }
            let _ = sender.send(StreamChunk::Done {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
            });
            let _ = provider;
        });

        Ok(stream)
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, RelayError> {
        Err(RelayError::new(
            ErrorKind::Unsupported,
            "this provider cannot list its models",
        )
        .with_provider(&self.name))
    }

    fn describe(&self) -> String {
        format!("custom provider at {}", self.base_url)
    }
}

fn main() {
    let base_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_owned());

    let provider = match DemoProvider::new("demo", &base_url) {
        Ok(provider) => provider,
        Err(error) => {
            eprintln!("relay: {error}");
            std::process::exit(1);
        }
    };

    let client = match relay_rs::Client::builder()
        .provider("demo", Arc::new(provider))
        .default_model("demo/demo-model")
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("relay: {error}");
            std::process::exit(1);
        }
    };

    let request = ChatRequest::new("demo/demo-model").user("Say hello in three words.");
    match client.complete_blocking(request) {
        Ok(response) => println!("{}", response.text()),
        Err(error) => {
            eprintln!("relay: {error}");
            std::process::exit(1);
        }
    }
}
