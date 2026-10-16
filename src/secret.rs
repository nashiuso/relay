//! A string that never shows up in logs or error messages.

use std::fmt;

/// A secret value.
///
/// The wrapped value is only accessible through [`SecretString::expose`].
/// `Debug` and `Display` render a placeholder, so secrets cannot leak into
/// logs, error messages or `relay doctor` output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the wrapped value.
    ///
    /// The name is a reminder that the value must not be logged.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Returns `true` when the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_the_value() {
        let secret = SecretString::new("sk-secret");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(secret.expose(), "sk-secret");
    }
}
