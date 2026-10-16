//! An adapter for OpenAI compatible chat completion APIs.
//!
//! The adapter talks to any endpoint that implements the OpenAI chat
//! completion protocol: `POST {base}/chat/completions` for completions and
//! `GET {base}/models` for the model list. Because the base URL is
//! configurable, the same adapter serves OpenAI, Groq, Together, LM Studio,
//! vLLM, llama.cpp's server and other compatible services.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::error::{ErrorKind, RelayError};
use crate::http::client::{HttpClient, HttpClientBuilder, HttpRequest, HttpResponse};
use crate::http::tls::TlsConfig;
use crate::http::url::Url;
use crate::json::{Map, Value};
use crate::model::ModelInfo;
use crate::providers::common::{CancelGuard, Flow, parse_usage, read_lines, sse_payload, truncate};
use crate::providers::traits::{Capabilities, ChatProvider, ProviderKind};
use crate::secret::SecretString;
use crate::stream::{ChatStream, StreamChunk};
use crate::types::{ChatRequest, ChatResponse, FinishReason, Message};

/// Options for [`OpenAiCompatibleProvider`].
#[derive(Debug, Clone, Default)]
pub struct OpenAiCompatibleOptions {
    /// The API key sent as a bearer token.
    pub api_key: Option<SecretString>,
    /// The model used when a request does not name one.
    pub default_model: Option<String>,
    /// The request timeout.
    pub timeout: Option<Duration>,
    /// The connection timeout.
    pub connect_timeout: Option<Duration>,
    /// Additional headers sent with every request.
    pub headers: Vec<(String, String)>,
    /// The TLS configuration.
    pub tls: TlsConfig,
}

/// A provider that speaks the OpenAI chat completion protocol.
#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    name: String,
    base_url: Url,
    api_key: Option<SecretString>,
    default_model: Option<String>,
    headers: Vec<(String, String)>,
    http: HttpClient,
}

impl OpenAiCompatibleProvider {
    /// Creates a provider for the given base URL.
    ///
    /// The base URL usually ends with the API version, for example
    /// `https://api.openai.com/v1`.
    pub fn new(
        name: impl Into<String>,
        base_url: &str,
        options: OpenAiCompatibleOptions,
    ) -> Result<Self, RelayError> {
        let base_url = Url::parse_lenient(base_url)
            .map_err(|err| RelayError::config(format!("invalid base_url `{base_url}`: {err}")))?;
        let mut builder: HttpClientBuilder = HttpClient::builder()
            .request_timeout(options.timeout.unwrap_or(Duration::from_secs(120)));
        if let Some(connect_timeout) = options.connect_timeout {
            builder = builder.connect_timeout(connect_timeout);
        }
        builder = builder.tls(options.tls);

        Ok(Self {
            name: name.into(),
            base_url,
            api_key: options.api_key,
            default_model: options.default_model,
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

    /// The model used for a request: the requested one, or the default.
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

        if let Some(temperature) = request.temperature_value() {
            payload.insert("temperature", Value::from_f32(temperature));
        }
        if let Some(top_p) = request.top_p_value() {
            payload.insert("top_p", Value::from_f32(top_p));
        }
        if let Some(max_tokens) = request.max_tokens_value() {
            payload.insert("max_tokens", Value::from(max_tokens as u64));
        }
        if !request.stop_sequences().is_empty() {
            payload.insert(
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
            payload.insert("seed", Value::from(seed));
        }
        for (key, value) in request.options() {
            payload.insert(key.clone(), value.clone());
        }
        payload.insert("stream", Value::Bool(stream));
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
            400 | 404 | 405 | 409 | 413 | 422 => ErrorKind::InvalidRequest,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::InvalidRequest,
        };
        let message = extract_error_message(body)
            .unwrap_or_else(|| format!("the provider answered with HTTP {status}"));
        RelayError::provider(kind, &self.name, message)
    }

    fn parse_response(
        &self,
        value: &Value,
        requested_model: &str,
    ) -> Result<ChatResponse, RelayError> {
        let choices = value
            .get("choices")
            .and_then(|choices| choices.as_array())
            .ok_or_else(|| {
                RelayError::provider(
                    ErrorKind::Decode,
                    &self.name,
                    "the response did not contain a `choices` array",
                )
            })?;
        let choice = choices.first().ok_or_else(|| {
            RelayError::provider(
                ErrorKind::Decode,
                &self.name,
                "the response did not contain any choice",
            )
        })?;
        let content = choice
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .unwrap_or_default();
        let finish_reason = choice
            .get("finish_reason")
            .and_then(|reason| reason.as_str())
            .map(FinishReason::from_wire);
        let model = value
            .get("model")
            .and_then(|model| model.as_str())
            .unwrap_or(requested_model);

        Ok(ChatResponse::new(&self.name, model, content)
            .with_finish_reason(finish_reason)
            .with_usage(parse_usage(value.get("usage")))
            .with_raw(Some(value.clone())))
    }
}

impl ChatProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::all()
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
        let http_request =
            HttpRequest::post(self.base_url.join_path("chat/completions")).json(&payload);
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
                format!("the provider returned invalid JSON: {err}"),
            )
        })?;
        let response = self.parse_response(&value, self.resolve_model(request))?;
        Ok(response.with_latency(started.elapsed()))
    }

    fn stream(&self, request: &ChatRequest) -> Result<ChatStream, RelayError> {
        let payload = self.build_payload(request, true);
        let http_request =
            HttpRequest::post(self.base_url.join_path("chat/completions")).json(&payload);
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
            let mut finish_reason: Option<FinishReason> = None;
            let mut usage = None;
            read_lines(body, timeout, Arc::clone(&cancel), &sender, |line| {
                let Some(payload) = sse_payload(line) else {
                    return Ok(Flow::Continue);
                };
                if payload.trim() == "[DONE]" {
                    return Ok(Flow::Stop);
                }
                let value = crate::json::parse(payload).map_err(|err| {
                    RelayError::provider(
                        ErrorKind::Decode,
                        &name,
                        format!("the provider sent invalid JSON: {err}"),
                    )
                })?;
                if let Some(error) = value.get("error") {
                    let message = error
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("the provider reported an error");
                    return Err(RelayError::provider(
                        ErrorKind::Server,
                        &name,
                        message.to_owned(),
                    ));
                }
                if let Some(reason) = value
                    .get("choices")
                    .and_then(|choices| choices.as_array())
                    .and_then(|choices| choices.first())
                    .and_then(|choice| choice.get("finish_reason"))
                    .and_then(|reason| reason.as_str())
                {
                    finish_reason = Some(FinishReason::from_wire(reason));
                }
                if let Some(text) = delta_text(&value) {
                    if !text.is_empty() {
                        let _ = sender.send(Ok(StreamChunk::Text(text)));
                    }
                }
                if let Some(reported) = parse_usage(value.get("usage")) {
                    usage = Some(reported);
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
        let request = HttpRequest::get(self.base_url.join_path("models"));
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
                format!("the provider returned invalid JSON: {err}"),
            )
        })?;
        let models = value
            .get("data")
            .and_then(|data| data.as_array())
            .ok_or_else(|| {
                RelayError::provider(
                    ErrorKind::Decode,
                    &self.name,
                    "the model list did not contain a `data` array",
                )
            })?;

        Ok(models
            .iter()
            .filter_map(|model| {
                let id = model.get("id")?.as_str()?;
                Some(
                    ModelInfo::new(id, &self.name)
                        .with_owned_by(
                            model
                                .get("owned_by")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        )
                        .with_created(
                            model
                                .get("created")
                                .and_then(|v| v.as_i64())
                                .map(|v| v as u64),
                        ),
                )
            })
            .collect())
    }

    fn describe(&self) -> String {
        self.base_url.to_string()
    }
}

/// Converts the messages of a request into the OpenAI wire format.
fn messages_to_json(messages: &[Message]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                let mut object = Map::new();
                object.insert("role", message.role().as_str());
                object.insert("content", message.content());
                if let Some(name) = message.name() {
                    object.insert("name", name);
                }
                Value::Object(object)
            })
            .collect(),
    )
}

/// Extracts the text of a streaming delta.
fn delta_text(value: &Value) -> Option<String> {
    let choice = value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())?;
    let delta = choice.get("delta").or_else(|| choice.get("message"))?;
    let text = delta.get("content")?;
    match text {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|text| text.as_str()) {
                    out.push_str(text);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Extracts a human readable error message from an error payload.
pub(crate) fn extract_error_message(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = crate::json::parse(trimmed) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
        {
            return Some(truncate(message, 400));
        }
        if let Some(message) = value.get("error").and_then(|error| error.as_str()) {
            return Some(truncate(message, 400));
        }
        if let Some(message) = value.get("message").and_then(|message| message.as_str()) {
            return Some(truncate(message, 400));
        }
        if let Some(message) = value.get("detail").and_then(|detail| detail.as_str()) {
            return Some(truncate(message, 400));
        }
    }
    Some(truncate(trimmed, 400))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::new(
            "openai",
            "https://api.example.com/v1",
            OpenAiCompatibleOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn builds_the_chat_payload() {
        let provider = provider();
        let request = ChatRequest::new("openai/gpt-4o-mini")
            .system("be brief")
            .user("hello")
            .temperature(0.5)
            .max_tokens(32);
        let payload = provider.build_payload(&request, false);

        assert_eq!(payload.get("model").unwrap().as_str(), Some("gpt-4o-mini"));
        assert_eq!(payload.get("stream").unwrap().as_bool(), Some(false));
        assert_eq!(payload.get("max_tokens").unwrap().as_i64(), Some(32));
        let messages = payload.get("messages").unwrap().as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].get("role").unwrap().as_str(), Some("system"));
        assert_eq!(messages[1].get("content").unwrap().as_str(), Some("hello"));
    }

    #[test]
    fn parses_a_chat_response() {
        let provider = provider();
        let value = crate::json::parse(
            r#"{"model":"gpt-4o-mini","choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#,
        )
        .unwrap();
        let response = provider.parse_response(&value, "gpt-4o-mini").unwrap();
        assert_eq!(response.text(), "hi");
        assert_eq!(response.provider(), "openai");
        assert_eq!(response.finish_reason(), Some(&FinishReason::Stop));
        assert_eq!(response.usage().unwrap().total_tokens, Some(4));
    }

    #[test]
    fn reports_missing_choices() {
        let provider = provider();
        let value = crate::json::parse(r#"{"foo":"bar"}"#).unwrap();
        let error = provider.parse_response(&value, "m").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Decode);
    }

    #[test]
    fn maps_status_codes_to_error_kinds() {
        let provider = provider();
        assert_eq!(provider.error_from_status(401, "").kind(), ErrorKind::Auth);
        assert_eq!(
            provider.error_from_status(429, "").kind(),
            ErrorKind::RateLimited
        );
        assert_eq!(
            provider.error_from_status(500, "").kind(),
            ErrorKind::Server
        );
        assert_eq!(
            provider.error_from_status(400, "").kind(),
            ErrorKind::InvalidRequest
        );
    }

    #[test]
    fn extracts_error_messages() {
        assert_eq!(
            extract_error_message(r#"{"error":{"message":"bad key"}}"#),
            Some("bad key".to_owned())
        );
        assert_eq!(
            extract_error_message(r#"{"error":"nope"}"#),
            Some("nope".to_owned())
        );
        assert_eq!(
            extract_error_message("plain text"),
            Some("plain text".to_owned())
        );
        assert_eq!(extract_error_message("  "), None);
    }

    #[test]
    fn extracts_delta_text() {
        let value = crate::json::parse(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#).unwrap();
        assert_eq!(delta_text(&value).as_deref(), Some("Hel"));
    }
}
