//! An adapter for local models served by Ollama.
//!
//! The adapter uses the `/api/chat` endpoint for completions, `/api/tags` for
//! the model list and `/api/version` for connectivity checks. Streaming
//! responses are newline delimited JSON, not server sent events.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::error::{ErrorKind, RelayError};
use crate::http::client::{HttpClient, HttpClientBuilder, HttpRequest, HttpResponse};
use crate::http::tls::TlsConfig;
use crate::http::url::Url;
use crate::json::{Map, Value};
use crate::model::ModelInfo;
use crate::providers::common::{CancelGuard, Flow, read_lines, truncate};
use crate::providers::traits::{Capabilities, ChatProvider, ProviderKind};
use crate::secret::SecretString;
use crate::stream::{ChatStream, StreamChunk};
use crate::types::{ChatRequest, ChatResponse, FinishReason, Message, Usage};

/// The default address of a local Ollama server.
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Options for [`OllamaProvider`].
#[derive(Debug, Clone, Default)]
pub struct OllamaOptions {
    /// The model used when a request does not name one.
    pub default_model: Option<String>,
    /// The request timeout.
    pub timeout: Option<Duration>,
    /// The connection timeout.
    pub connect_timeout: Option<Duration>,
    /// An API key, for Ollama deployments that require one.
    pub api_key: Option<SecretString>,
    /// Additional headers sent with every request.
    pub headers: Vec<(String, String)>,
    /// The TLS configuration.
    pub tls: TlsConfig,
}

/// A provider that talks to a local Ollama server.
#[derive(Debug)]
pub struct OllamaProvider {
    name: String,
    base_url: Url,
    default_model: Option<String>,
    api_key: Option<SecretString>,
    headers: Vec<(String, String)>,
    http: HttpClient,
}

impl OllamaProvider {
    /// Creates a provider for the given base URL.
    pub fn new(
        name: impl Into<String>,
        base_url: &str,
        options: OllamaOptions,
    ) -> Result<Self, RelayError> {
        let base_url = Url::parse_lenient(base_url)
            .map_err(|err| RelayError::config(format!("invalid base_url `{base_url}`: {err}")))?;
        let mut builder: HttpClientBuilder = HttpClient::builder()
            .request_timeout(options.timeout.unwrap_or(Duration::from_secs(300)));
        if let Some(connect_timeout) = options.connect_timeout {
            builder = builder.connect_timeout(connect_timeout);
        }
        builder = builder.tls(options.tls);

        Ok(Self {
            name: name.into(),
            base_url,
            default_model: options.default_model,
            api_key: options.api_key,
            headers: options.headers,
            http: builder.build(),
        })
    }

    /// The base URL of the provider.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// The default model of the provider, when configured.
    pub fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    /// Returns the version reported by the server.
    ///
    /// Used by `relay doctor` to check that a local server is reachable.
    pub fn version(&self) -> Result<String, RelayError> {
        let request = HttpRequest::get(self.base_url.join_path("api/version"));
        let mut response = self.request(request)?;
        if !response.is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(self.error_from_status(status, &body));
        }
        let text = response.text()?;
        let value = crate::json::parse(&text).map_err(|err| {
            RelayError::provider(
                ErrorKind::Decode,
                &self.name,
                format!("the server returned invalid JSON: {err}"),
            )
        })?;
        Ok(value
            .get("version")
            .and_then(|version| version.as_str())
            .unwrap_or("unknown")
            .to_owned())
    }

    fn resolve_model<'a>(&'a self, request: &'a ChatRequest) -> &'a str {
        if request.model().model().is_empty() {
            self.default_model.as_deref().unwrap_or("")
        } else {
            request.model().model()
        }
    }

    fn build_payload(&self, request: &ChatRequest, stream: bool) -> Value {
        let mut payload = Map::new();
        payload.insert("model", self.resolve_model(request));
        payload.insert("messages", messages_to_json(request.messages()));
        payload.insert("stream", Value::Bool(stream));

        let mut options = Map::new();
        if let Some(temperature) = request.temperature_value() {
            options.insert("temperature", Value::from_f32(temperature));
        }
        if let Some(top_p) = request.top_p_value() {
            options.insert("top_p", Value::from_f32(top_p));
        }
        if let Some(max_tokens) = request.max_tokens_value() {
            options.insert("num_predict", Value::from(max_tokens as u64));
        }
        if !request.stop_sequences().is_empty() {
            options.insert(
                "stop",
                Value::Array(
                    request
                        .stop_sequences()
                        .iter()
                        .map(|value| Value::string(value.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(seed) = request.seed_value() {
            options.insert("seed", Value::from(seed));
        }
        if !options.is_empty() {
            payload.insert("options", Value::Object(options));
        }

        for (key, value) in request.options() {
            payload.insert(key.clone(), value.clone());
        }
        Value::Object(payload)
    }

    fn request(&self, request: HttpRequest) -> Result<HttpResponse, RelayError> {
        let mut request = request;
        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", key.expose()));
        }
        for (name, value) in &self.headers {
            request = request.header(name.clone(), value.clone());
        }
        self.http.send(&request)
    }

    fn error_from_status(&self, status: u16, body: &str) -> RelayError {
        let kind = match status {
            401 | 403 => ErrorKind::Auth,
            429 => ErrorKind::RateLimited,
            408 | 504 => ErrorKind::Timeout,
            404 => ErrorKind::InvalidRequest,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::InvalidRequest,
        };
        let message = crate::providers::openai_compatible::extract_error_message(body)
            .unwrap_or_else(|| format!("the server answered with HTTP {status}"));
        RelayError::provider(kind, &self.name, message)
    }
}

impl ChatProvider for OllamaProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            model_listing: true,
            system_messages: true,
            tools: false,
        }
    }

    fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    fn has_credentials(&self) -> bool {
        self.api_key.as_ref().is_some_and(|key| !key.is_empty())
    }

    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError> {
        let started = Instant::now();
        let payload = self.build_payload(request, false);
        let http_request = HttpRequest::post(self.base_url.join_path("api/chat")).json(&payload);
        let mut response = self.request(http_request)?;

        if !response.is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(self.error_from_status(status, &body));
        }

        let text = response.text()?;
        let value = crate::json::parse(&text).map_err(|err| {
            RelayError::provider(
                ErrorKind::Decode,
                &self.name,
                format!("the server returned invalid JSON: {err}"),
            )
        })?;
        let content = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .unwrap_or_default();
        let finish_reason = value
            .get("done_reason")
            .and_then(|reason| reason.as_str())
            .map(FinishReason::from_wire);
        let model = value
            .get("model")
            .and_then(|model| model.as_str())
            .unwrap_or_else(|| self.resolve_model(request));
        let usage = parse_ollama_usage(&value);

        Ok(ChatResponse::new(&self.name, model, content)
            .with_finish_reason(finish_reason)
            .with_usage(usage)
            .with_latency(started.elapsed())
            .with_raw(Some(value)))
    }

    fn stream(&self, request: &ChatRequest) -> Result<ChatStream, RelayError> {
        let payload = self.build_payload(request, true);
        let http_request = HttpRequest::post(self.base_url.join_path("api/chat")).json(&payload);
        let response = self.request(http_request)?;

        if !response.is_success() {
            let status = response.status();
            let mut response = response;
            let body = response.text().unwrap_or_default();
            return Err(self.error_from_status(status, &body));
        }

        let body = response.into_body();
        let (sender, receiver) = crate::sync::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = Box::new(CancelGuard(Arc::clone(&cancel)));
        let name = self.name.clone();
        let timeout = self.http.request_timeout();

        std::thread::spawn(move || {
            let mut usage: Option<Usage> = None;
            let mut finish_reason: Option<FinishReason> = None;
            read_lines(body, timeout, Arc::clone(&cancel), &sender, |line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return Ok(Flow::Continue);
                }
                let value = crate::json::parse(trimmed).map_err(|err| {
                    RelayError::provider(
                        ErrorKind::Decode,
                        &name,
                        format!("the server sent invalid JSON: {err}"),
                    )
                })?;
                if let Some(error) = value.get("error") {
                    let message = error
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("the server reported an error");
                    return Err(RelayError::provider(
                        ErrorKind::Server,
                        &name,
                        message.to_owned(),
                    ));
                }
                if let Some(content) = value
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .and_then(|content| content.as_str())
                {
                    if !content.is_empty() {
                        let _ = sender.send(Ok(StreamChunk::Text(content.to_owned())));
                    }
                }
                if let Some(reason) = value.get("done_reason").and_then(|reason| reason.as_str()) {
                    finish_reason = Some(FinishReason::from_wire(reason));
                }
                if let Some(reported) = parse_ollama_usage(&value) {
                    usage = Some(reported);
                }
                if value.get("done").and_then(|done| done.as_bool()) == Some(true) {
                    return Ok(Flow::Stop);
                }
                Ok(Flow::Continue)
            });
            let _ = sender.send(Ok(StreamChunk::Done {
                finish_reason,
                usage,
            }));
        });

        Ok(ChatStream::new(receiver, Some(guard)))
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, RelayError> {
        let request = HttpRequest::get(self.base_url.join_path("api/tags"));
        let mut response = self.request(request)?;
        if !response.is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(self.error_from_status(status, &body));
        }
        let text = response.text()?;
        let value = crate::json::parse(&text).map_err(|err| {
            RelayError::provider(
                ErrorKind::Decode,
                &self.name,
                format!("the server returned invalid JSON: {err}"),
            )
        })?;
        let models = value
            .get("models")
            .and_then(|models| models.as_array())
            .ok_or_else(|| {
                RelayError::provider(
                    ErrorKind::Decode,
                    &self.name,
                    "the model list did not contain a `models` array",
                )
            })?;

        Ok(models
            .iter()
            .filter_map(|model| {
                let id = model.get("name").or_else(|| model.get("model"))?.as_str()?;
                Some(ModelInfo::new(id, &self.name))
            })
            .collect())
    }

    fn describe(&self) -> String {
        self.base_url.to_string()
    }
}

/// Parses the token counters reported by Ollama.
fn parse_ollama_usage(value: &Value) -> Option<Usage> {
    let prompt = value.get("prompt_eval_count").and_then(|v| v.as_i64());
    let completion = value.get("eval_count").and_then(|v| v.as_i64());
    if prompt.is_none() && completion.is_none() {
        return None;
    }
    let total = match (prompt, completion) {
        (Some(prompt), Some(completion)) => Some(prompt + completion),
        _ => None,
    };
    Some(Usage::new(
        prompt.map(|v| v as u32),
        completion.map(|v| v as u32),
        total.map(|v| v as u32),
    ))
}

/// Converts the messages of a request into the Ollama wire format.
fn messages_to_json(messages: &[Message]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                let mut object = Map::new();
                object.insert("role", message.role().as_str());
                object.insert("content", message.content());
                Value::Object(object)
            })
            .collect(),
    )
}

/// Truncates a provider message.
#[allow(dead_code)]
fn truncate_message(text: &str) -> String {
    truncate(text, 400)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OllamaProvider {
        OllamaProvider::new("ollama", DEFAULT_OLLAMA_URL, OllamaOptions::default()).unwrap()
    }

    #[test]
    fn builds_the_chat_payload() {
        let provider = provider();
        let request = ChatRequest::new("ollama/llama3.2")
            .user("hello")
            .max_tokens(64)
            .temperature(0.1);
        let payload = provider.build_payload(&request, false);

        assert_eq!(payload.get("model").unwrap().as_str(), Some("llama3.2"));
        assert_eq!(payload.get("stream").unwrap().as_bool(), Some(false));
        let options = payload.get("options").unwrap();
        assert_eq!(options.get("num_predict").unwrap().as_i64(), Some(64));
        assert_eq!(options.get("temperature").unwrap().as_f64(), Some(0.1));
    }

    #[test]
    fn parses_token_counters() {
        let value = crate::json::parse(r#"{"prompt_eval_count":10,"eval_count":5}"#).unwrap();
        let usage = parse_ollama_usage(&value).unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(15));
    }

    #[test]
    fn uses_the_default_model_when_the_request_has_none() {
        let provider = OllamaProvider::new(
            "ollama",
            DEFAULT_OLLAMA_URL,
            OllamaOptions {
                default_model: Some("llama3.2".to_owned()),
                ..OllamaOptions::default()
            },
        )
        .unwrap();
        let request = ChatRequest::new("ollama/").user("hi");
        let payload = provider.build_payload(&request, false);
        assert_eq!(payload.get("model").unwrap().as_str(), Some("llama3.2"));
    }
}
