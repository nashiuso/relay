//! Provider implementations.

pub mod common;
pub mod ollama;
pub mod openai_compatible;
pub mod traits;

pub use ollama::{DEFAULT_OLLAMA_URL, OllamaOptions, OllamaProvider};
pub use openai_compatible::{OpenAiCompatibleOptions, OpenAiCompatibleProvider};
pub use traits::{Capabilities, ChatProvider, ProviderKind};
