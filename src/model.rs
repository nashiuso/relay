//! Model and provider identifiers.

use std::fmt;
use std::str::FromStr;

use crate::error::RelayError;

/// A reference to a model, optionally qualified by a provider.
///
/// The canonical form is `provider/model`, for example `ollama/llama3.2` or
/// `openai/gpt-4o-mini`. A reference without a provider part uses the
/// default provider of the active configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef {
    provider: String,
    model: String,
}

impl ModelRef {
    /// Creates a model reference from a provider and a model name.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Parses a `provider/model` reference.
    ///
    /// The reference must contain a non-empty model name; the provider part
    /// may be empty, in which case the default provider is used later on.
    pub fn parse(value: &str) -> Result<Self, RelayError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(RelayError::usage(
                "a model reference cannot be empty; expected `provider/model`",
            ));
        }
        let (provider, model) = match value.split_once('/') {
            Some((provider, model)) => (provider.trim(), model.trim()),
            None => ("", value),
        };
        if model.is_empty() {
            return Err(RelayError::usage(format!(
                "`{value}` is missing a model name; expected `provider/model`"
            )));
        }
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
        })
    }

    /// The provider part of the reference, possibly empty.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The model name of the reference.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns `true` when the reference does not name a provider.
    pub fn has_provider(&self) -> bool {
        !self.provider.is_empty()
    }

    /// Returns a copy of the reference bound to a provider.
    pub fn with_provider(&self, provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: self.model.clone(),
        }
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.provider.is_empty() {
            f.write_str(&self.model)
        } else {
            write!(f, "{}/{}", self.provider, self.model)
        }
    }
}

impl FromStr for ModelRef {
    type Err = RelayError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        ModelRef::parse(value)
    }
}

impl From<&str> for ModelRef {
    fn from(value: &str) -> Self {
        match value.split_once('/') {
            Some((provider, model)) => ModelRef::new(provider.trim(), model.trim()),
            None => ModelRef::new("", value.trim()),
        }
    }
}

impl From<String> for ModelRef {
    fn from(value: String) -> Self {
        ModelRef::from(value.as_str())
    }
}

/// A model offered by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    id: String,
    provider: String,
    owned_by: Option<String>,
    created: Option<u64>,
}

impl ModelInfo {
    /// Creates a model description.
    pub fn new(id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            owned_by: None,
            created: None,
        }
    }

    /// Sets the owner reported by the provider.
    pub fn with_owned_by(mut self, owned_by: Option<String>) -> Self {
        self.owned_by = owned_by;
        self
    }

    /// Sets the creation timestamp reported by the provider.
    pub fn with_created(mut self, created: Option<u64>) -> Self {
        self.created = created;
        self
    }

    /// The identifier of the model.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The provider that serves the model.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The owner of the model, when the provider reports one.
    pub fn owned_by(&self) -> Option<&str> {
        self.owned_by.as_deref()
    }

    /// The creation timestamp of the model, when the provider reports one.
    pub fn created(&self) -> Option<u64> {
        self.created
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_qualified_references() {
        let model = ModelRef::parse("openai/gpt-4o-mini").unwrap();
        assert_eq!(model.provider(), "openai");
        assert_eq!(model.model(), "gpt-4o-mini");
        assert_eq!(model.to_string(), "openai/gpt-4o-mini");
    }

    #[test]
    fn parses_bare_model_names() {
        let model = ModelRef::parse("llama3.2").unwrap();
        assert!(!model.has_provider());
        assert_eq!(model.model(), "llama3.2");
        assert_eq!(model.to_string(), "llama3.2");
    }

    #[test]
    fn rejects_empty_references() {
        assert!(ModelRef::parse("").is_err());
        assert!(ModelRef::parse("openai/").is_err());
    }

    #[test]
    fn keeps_model_names_with_slashes() {
        let model = ModelRef::parse("ollama/llama3:latest").unwrap();
        assert_eq!(model.provider(), "ollama");
        assert_eq!(model.model(), "llama3:latest");
    }
}
