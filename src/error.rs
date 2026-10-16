//! Typed errors for Relay.
//!
//! Every failure that crosses the public API is a [`RelayError`]. Errors carry
//! the provider that produced them (when known) and a machine readable
//! [`ErrorKind`] so callers can decide whether a request is worth retrying.
//!
//! Credentials and request payloads are never embedded in error messages.

use std::fmt;

/// Convenience alias used throughout Relay.
pub type Result<T, E = RelayError> = std::result::Result<T, E>;

/// Machine readable classification of a [`RelayError`].
///
/// The kind is deliberately coarse: it answers "what should the caller do?"
/// rather than "what exactly went wrong?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The configuration file or environment is invalid.
    Config,
    /// The command line was used incorrectly.
    Usage,
    /// The requested provider is not configured.
    UnknownProvider,
    /// The requested model is not known to the provider.
    UnknownModel,
    /// The provider rejected the credentials.
    Auth,
    /// The provider throttled the request.
    RateLimited,
    /// The provider rejected the request contents.
    InvalidRequest,
    /// The provider does not implement the requested capability.
    Unsupported,
    /// The request or the connection timed out.
    Timeout,
    /// The connection to the provider failed.
    Connection,
    /// The provider failed while handling the request.
    Server,
    /// A response could not be decoded.
    Decode,
    /// A stream ended unexpectedly or was interrupted.
    Stream,
    /// The operation was cancelled by the caller.
    Cancelled,
    /// A local I/O failure.
    Io,
}

impl ErrorKind {
    /// Returns `true` when retrying the same request may succeed.
    ///
    /// Authentication problems, invalid requests and unsupported capabilities
    /// are permanent and are therefore never retried.
    pub fn is_retriable(self) -> bool {
        matches!(
            self,
            ErrorKind::Timeout | ErrorKind::Connection | ErrorKind::RateLimited | ErrorKind::Server
        )
    }

    /// A short, stable, lowercase label for the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Config => "config",
            ErrorKind::Usage => "usage",
            ErrorKind::UnknownProvider => "unknown_provider",
            ErrorKind::UnknownModel => "unknown_model",
            ErrorKind::Auth => "auth",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::InvalidRequest => "invalid_request",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Connection => "connection",
            ErrorKind::Server => "server",
            ErrorKind::Decode => "decode",
            ErrorKind::Stream => "stream",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Io => "io",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error type returned by every fallible Relay operation.
#[derive(Debug)]
pub struct RelayError {
    kind: ErrorKind,
    provider: Option<String>,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl RelayError {
    /// Creates an error with an explicit kind, message and optional provider.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            provider: None,
            message: message.into(),
            source: None,
        }
    }

    /// Creates an error that is attributed to a specific provider.
    pub fn provider(
        kind: ErrorKind,
        provider: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            provider: Some(provider.into()),
            message: message.into(),
            source: None,
        }
    }

    /// Attaches the provider name to an existing error.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Attaches an underlying cause.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The machine readable classification of this error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The provider that produced the error, when known.
    pub fn provider_name(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// The human readable message, without provider prefix or cause.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns `true` when the failure is transient and may be retried.
    pub fn is_retriable(&self) -> bool {
        self.kind.is_retriable()
    }

    pub(crate) fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    pub(crate) fn decode(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Decode, message)
    }

    pub(crate) fn stream(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Stream, message)
    }

    pub(crate) fn io(err: std::io::Error) -> Self {
        let kind = match err.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ErrorKind::Timeout,
            _ => ErrorKind::Io,
        };
        Self::new(kind, err.to_string()).with_source(err)
    }
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider {
            Some(provider) => write!(f, "{provider}: {}: {}", self.kind, self.message),
            None => write!(f, "{}: {}", self.kind, self.message),
        }
    }
}

impl std::error::Error for RelayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<std::io::Error> for RelayError {
    fn from(value: std::io::Error) -> Self {
        RelayError::io(value)
    }
}

impl From<crate::json::JsonError> for RelayError {
    fn from(value: crate::json::JsonError) -> Self {
        RelayError::decode(value.to_string())
    }
}

impl From<crate::toml::TomlError> for RelayError {
    fn from(value: crate::toml::TomlError) -> Self {
        RelayError::config(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_provider_and_kind() {
        let err = RelayError::provider(ErrorKind::RateLimited, "openai", "slow down");
        assert_eq!(err.to_string(), "openai: rate_limited: slow down");
        assert_eq!(err.provider_name(), Some("openai"));
    }

    #[test]
    fn display_without_provider() {
        let err = RelayError::config("missing base_url");
        assert_eq!(err.to_string(), "config: missing base_url");
        assert!(err.provider_name().is_none());
    }

    #[test]
    fn permanent_errors_are_not_retriable() {
        assert!(!ErrorKind::Auth.is_retriable());
        assert!(!ErrorKind::InvalidRequest.is_retriable());
        assert!(!ErrorKind::Unsupported.is_retriable());
        assert!(ErrorKind::Timeout.is_retriable());
        assert!(ErrorKind::RateLimited.is_retriable());
        assert!(ErrorKind::Server.is_retriable());
        assert!(ErrorKind::Connection.is_retriable());
    }

    #[test]
    fn io_timeouts_map_to_timeout_kind() {
        let err = RelayError::io(std::io::Error::new(std::io::ErrorKind::TimedOut, "slow"));
        assert_eq!(err.kind(), ErrorKind::Timeout);
    }
}
