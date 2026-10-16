//! The core request and response types of Relay.

use std::fmt;
use std::time::Duration;

use crate::json::Value;
use crate::model::ModelRef;

/// The role of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Instructions that steer the whole conversation.
    System,
    /// A message from the user.
    User,
    /// A message from the model.
    Assistant,
    /// The result of a tool call.
    Tool,
}

impl Role {
    /// The wire representation of the role.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    /// Parses a role from its wire representation.
    pub fn parse(value: &str) -> Option<Role> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" | "developer" => Some(Role::System),
            "user" | "human" => Some(Role::User),
            "assistant" | "model" | "ai" => Some(Role::Assistant),
            "tool" | "function" => Some(Role::Tool),
            _ => None,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    role: Role,
    content: String,
    name: Option<String>,
}

impl Message {
    /// Creates a message.
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            name: None,
        }
    }

    /// Creates a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    /// Creates a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    /// Creates an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    /// Creates a tool message.
    pub fn tool(content: impl Into<String>) -> Self {
        Self::new(Role::Tool, content)
    }

    /// Sets an optional participant name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The role of the message.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The content of the message.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// The participant name, when set.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// The reason a model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    /// The model completed the answer.
    Stop,
    /// The token budget was exhausted.
    Length,
    /// The model wants to call a tool.
    ToolCalls,
    /// The provider filtered the answer.
    ContentFilter,
    /// A reason reported by the provider that Relay does not know.
    Other(String),
}

impl FinishReason {
    /// Parses a finish reason from its wire representation.
    pub fn from_wire(value: &str) -> FinishReason {
        match value.trim().to_ascii_lowercase().as_str() {
            "stop" | "end_turn" | "stop_sequence" => FinishReason::Stop,
            "length" | "max_tokens" | "model_length" => FinishReason::Length,
            "tool_calls" | "function_call" | "tool_use" => FinishReason::ToolCalls,
            "content_filter" | "safety" => FinishReason::ContentFilter,
            _ => FinishReason::Other(value.to_owned()),
        }
    }

    /// The wire representation of the finish reason.
    pub fn as_str(&self) -> &str {
        match self {
            FinishReason::Stop => "stop",
            FinishReason::Length => "length",
            FinishReason::ToolCalls => "tool_calls",
            FinishReason::ContentFilter => "content_filter",
            FinishReason::Other(value) => value,
        }
    }
}

impl fmt::Display for FinishReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Token accounting reported by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    pub prompt_tokens: Option<u32>,
    /// Tokens produced by the model.
    pub completion_tokens: Option<u32>,
    /// Total tokens used by the request.
    pub total_tokens: Option<u32>,
}

impl Usage {
    /// Creates a usage record.
    pub fn new(
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
        total_tokens: Option<u32>,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        }
    }
}

/// A chat request.
///
/// Requests are built with a fluent API:
///
/// ```no_run
/// use relay_rs::{ChatRequest, Role};
///
/// let request = ChatRequest::new("ollama/llama3.2")
///     .message(Role::System, "You are terse.")
///     .message(Role::User, "Explain DNS in one sentence.");
/// assert_eq!(request.messages().len(), 2);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    model: ModelRef,
    messages: Vec<Message>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    max_tokens: Option<u32>,
    stop: Vec<String>,
    seed: Option<u64>,
    extra: Vec<(String, Value)>,
}

impl ChatRequest {
    /// Creates a request for the given model.
    pub fn new(model: impl Into<ModelRef>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop: Vec::new(),
            seed: None,
            extra: Vec::new(),
        }
    }

    /// Appends a message.
    pub fn message(mut self, role: Role, content: impl Into<String>) -> Self {
        self.messages.push(Message::new(role, content));
        self
    }

    /// Appends a system message.
    pub fn system(self, content: impl Into<String>) -> Self {
        self.message(Role::System, content)
    }

    /// Appends a user message.
    pub fn user(self, content: impl Into<String>) -> Self {
        self.message(Role::User, content)
    }

    /// Appends an assistant message.
    pub fn assistant(self, content: impl Into<String>) -> Self {
        self.message(Role::Assistant, content)
    }

    /// Sets the sampling temperature.
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Replaces the model the request is addressed to.
    pub fn with_model(mut self, model: ModelRef) -> Self {
        self.model = model;
        self
    }

    /// Sets the nucleus sampling parameter.
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Sets the maximum number of tokens to generate.
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Adds a stop sequence.
    pub fn stop(mut self, stop: impl Into<String>) -> Self {
        self.stop.push(stop.into());
        self
    }

    /// Sets the random seed, when the provider supports it.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Adds a provider specific option.
    pub fn option(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra.push((key.into(), value.into()));
        self
    }

    /// The model the request is addressed to.
    pub fn model(&self) -> &ModelRef {
        &self.model
    }

    /// The messages of the request.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The sampling temperature, when set.
    pub fn temperature_value(&self) -> Option<f32> {
        self.temperature
    }

    /// The nucleus sampling parameter, when set.
    pub fn top_p_value(&self) -> Option<f32> {
        self.top_p
    }

    /// The maximum number of tokens, when set.
    pub fn max_tokens_value(&self) -> Option<u32> {
        self.max_tokens
    }

    /// The configured stop sequences.
    pub fn stop_sequences(&self) -> &[String] {
        &self.stop
    }

    /// The configured seed, when set.
    pub fn seed_value(&self) -> Option<u64> {
        self.seed
    }

    /// The provider specific options.
    pub fn options(&self) -> &[(String, Value)] {
        &self.extra
    }
}

/// The answer of a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    provider: String,
    model: String,
    content: String,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
    latency: Duration,
    raw: Option<Value>,
}

impl ChatResponse {
    /// Creates a response.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            content: content.into(),
            finish_reason: None,
            usage: None,
            latency: Duration::ZERO,
            raw: None,
        }
    }

    /// The text produced by the model.
    pub fn text(&self) -> &str {
        &self.content
    }
    /// The text produced by the model.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// The provider that answered the request.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The model that answered the request.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Why the model stopped generating, when the provider reports it.
    pub fn finish_reason(&self) -> Option<&FinishReason> {
        self.finish_reason.as_ref()
    }

    /// The token usage, when the provider reports it.
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// How long the request took.
    pub fn latency(&self) -> Duration {
        self.latency
    }

    /// The unmodified provider payload, when it was kept.
    pub fn raw(&self) -> Option<&Value> {
        self.raw.as_ref()
    }

    /// Sets the reason why the model stopped generating.
    pub fn with_finish_reason(mut self, reason: Option<FinishReason>) -> Self {
        self.finish_reason = reason;
        self
    }

    /// Sets the token usage reported by the provider.
    pub fn with_usage(mut self, usage: Option<Usage>) -> Self {
        self.usage = usage;
        self
    }

    pub(crate) fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    pub(crate) fn with_raw(mut self, raw: Option<Value>) -> Self {
        self.raw = raw;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_requests_fluently() {
        let request = ChatRequest::new("ollama/llama3.2")
            .system("be brief")
            .user("hello")
            .temperature(0.2)
            .max_tokens(64)
            .stop("END");

        assert_eq!(request.model().provider(), "ollama");
        assert_eq!(request.messages().len(), 2);
        assert_eq!(request.messages()[0].role(), Role::System);
        assert_eq!(request.temperature_value(), Some(0.2));
        assert_eq!(request.max_tokens_value(), Some(64));
        assert_eq!(request.stop_sequences(), ["END".to_owned()]);
    }

    #[test]
    fn parses_roles() {
        assert_eq!(Role::parse("user"), Some(Role::User));
        assert_eq!(Role::parse("SYSTEM"), Some(Role::System));
        assert_eq!(Role::parse("nonsense"), None);
    }

    #[test]
    fn parses_finish_reasons() {
        assert_eq!(FinishReason::from_wire("stop"), FinishReason::Stop);
        assert_eq!(FinishReason::from_wire("max_tokens"), FinishReason::Length);
        assert_eq!(
            FinishReason::from_wire("something_else"),
            FinishReason::Other("something_else".to_owned())
        );
    }
}
