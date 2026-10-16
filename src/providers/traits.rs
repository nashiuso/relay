//! The provider abstraction.
//!
//! Every backend that can answer a chat request implements [`ChatProvider`].
//! The trait is deliberately small: it covers the operations that Relay
//! actually supports, so new providers can be added without touching the
//! client, the CLI or the HTTP server.

use std::fmt;

use crate::error::RelayError;
use crate::model::ModelInfo;
use crate::stream::ChatStream;
use crate::types::{ChatRequest, ChatResponse};

/// What a provider is able to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// The provider can stream answers.
    pub streaming: bool,
    /// The provider can list its models.
    pub model_listing: bool,
    /// The provider understands system messages.
    pub system_messages: bool,
    /// The provider accepts tool definitions.
    pub tools: bool,
}

impl Capabilities {
    /// A provider that supports every capability Relay knows about.
    pub fn all() -> Self {
        Self {
            streaming: true,
            model_listing: true,
            system_messages: true,
            tools: true,
        }
    }

    /// A provider that supports nothing beyond plain completions.
    pub fn none() -> Self {
        Self {
            streaming: false,
            model_listing: false,
            system_messages: true,
            tools: false,
        }
    }
}

/// The family a provider belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// An OpenAI compatible HTTP API.
    OpenAiCompatible,
    /// A local Ollama server.
    Ollama,
    /// A provider implemented by the user of the crate.
    Custom,
}

impl ProviderKind {
    /// The wire name of the provider kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::Ollama => "ollama",
            ProviderKind::Custom => "custom",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A backend that can answer chat requests.
pub trait ChatProvider: Send + Sync + fmt::Debug {
    /// The name the provider is configured under.
    fn name(&self) -> &str;

    /// The family the provider belongs to.
    fn kind(&self) -> ProviderKind;

    /// The capabilities of the provider.
    fn capabilities(&self) -> Capabilities;

    /// The model the provider uses when a request does not name one.
    fn default_model(&self) -> Option<&str> {
        None
    }

    /// Whether the provider is configured with credentials.
    fn has_credentials(&self) -> bool {
        false
    }

    /// Runs a chat completion.
    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError>;

    /// Starts a streaming chat completion.
    fn stream(&self, request: &ChatRequest) -> Result<ChatStream, RelayError>;

    /// Lists the models the provider offers.
    fn list_models(&self) -> Result<Vec<ModelInfo>, RelayError>;

    /// A one line description of the provider, used by `relay providers`.
    fn describe(&self) -> String {
        format!("{} provider", self.kind())
    }
}
