//! A minimal URL parser for the HTTP transports.
//!
//! Relay only needs to talk to HTTP endpoints, so the parser understands
//! `scheme://host[:port][/path][?query][#fragment]` and nothing more.

use std::fmt;

/// The transport scheme of a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    /// Plaintext HTTP.
    Http,
    /// HTTP over TLS.
    Https,
}

impl Scheme {
    /// The canonical name of the scheme.
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }

    /// The default port for the scheme.
    pub fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parsed URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    scheme: Scheme,
    host: String,
    port: u16,
    path: String,
    query: Option<String>,
}

/// A URL parsing failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlError {
    message: String,
}

impl UrlError {
    /// The human readable description of the failure.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UrlError {}

impl Url {
    /// Parses an absolute URL.
    ///
    /// The scheme is required; use [`Url::parse_lenient`] to fall back to
    /// sensible defaults for configuration values.
    pub fn parse(input: &str) -> Result<Url, UrlError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(UrlError {
                message: "URL is empty".to_owned(),
            });
        }
        let (scheme, rest) = input.split_once("://").ok_or_else(|| UrlError {
            message: format!(
                "URL `{input}` is missing a scheme (expected `http://` or `https://`)"
            ),
        })?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            other => {
                return Err(UrlError {
                    message: format!("unsupported URL scheme `{other}`"),
                });
            }
        };

        let (authority, remainder) = match rest.find(['/', '?', '#']) {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err(UrlError {
                message: format!("URL `{input}` is missing a host"),
            });
        }

        let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
            // IPv6 literal.
            let (host, rest) = stripped.split_once(']').ok_or_else(|| UrlError {
                message: format!("URL `{input}` has an unterminated IPv6 host"),
            })?;
            let port = match rest.strip_prefix(':') {
                Some(port) => parse_port(port, input)?,
                None => scheme.default_port(),
            };
            (format!("[{host}]"), port)
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host.to_owned(), parse_port(port, input)?),
                None => (authority.to_owned(), scheme.default_port()),
            }
        };
        if host.is_empty() {
            return Err(UrlError {
                message: format!("URL `{input}` is missing a host"),
            });
        }

        let (before_fragment, _) = match remainder.split_once('#') {
            Some((head, _)) => (head, ""),
            None => (remainder, ""),
        };
        let (path, query) = match before_fragment.split_once('?') {
            Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
            None => (before_fragment.to_owned(), None),
        };
        let path = if path.is_empty() {
            "/".to_owned()
        } else if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        };

        Ok(Url {
            scheme,
            host,
            port,
            path,
            query,
        })
    }

    /// Parses a URL, defaulting the scheme when it is missing.
    ///
    /// Loopback hosts default to `http` so that local development endpoints
    /// keep working, every other host defaults to `https`.
    pub fn parse_lenient(input: &str) -> Result<Url, UrlError> {
        let trimmed = input.trim();
        if trimmed.contains("://") {
            return Url::parse(trimmed);
        }
        let host = trimmed
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(trimmed);
        let scheme = if is_loopback_host(host) {
            "http"
        } else {
            "https"
        };
        Url::parse(&format!("{scheme}://{trimmed}"))
    }

    /// The scheme of the URL.
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The host of the URL, without brackets for IPv6 literals.
    pub fn host(&self) -> &str {
        self.host.trim_start_matches('[').trim_end_matches(']')
    }

    /// The port of the URL.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The path of the URL, always starting with `/`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The query string of the URL, without the leading `?`.
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// The `Host` header value for the URL.
    pub fn authority(&self) -> String {
        let default = self.scheme.default_port();
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The request target (path and query) used on the request line.
    pub fn request_target(&self) -> String {
        match &self.query {
            Some(query) => format!("{}?{}", self.path, query),
            None => self.path.clone(),
        }
    }

    /// Returns a new URL with `suffix` appended to the current path.
    ///
    /// A leading `/` on the suffix is optional and duplicate slashes are
    /// collapsed.
    pub fn join_path(&self, suffix: &str) -> Url {
        let base = self.path.trim_end_matches('/');
        let suffix = suffix.trim_start_matches('/');
        let path = if suffix.is_empty() {
            if base.is_empty() {
                "/".to_owned()
            } else {
                base.to_owned()
            }
        } else {
            format!("{base}/{suffix}")
        };
        Url {
            scheme: self.scheme,
            host: self.host.clone(),
            port: self.port,
            path,
            query: self.query.clone(),
        }
    }

    /// Returns `true` when the host refers to the local machine.
    pub fn is_loopback(&self) -> bool {
        is_loopback_host(self.host())
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}{}", self.scheme, self.authority(), self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        Ok(())
    }
}

fn parse_port(raw: &str, input: &str) -> Result<u16, UrlError> {
    raw.parse::<u16>().map_err(|_| UrlError {
        message: format!("URL `{input}` has an invalid port `{raw}`"),
    })
}

/// Returns `true` for hosts that always point at the local machine.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if host == "::1" {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(addr)) => addr.is_loopback(),
        Ok(std::net::IpAddr::V6(addr)) => addr.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_url() {
        let url = Url::parse("https://api.example.com:8443/v1/chat?x=1").unwrap();
        assert_eq!(url.scheme(), Scheme::Https);
        assert_eq!(url.host(), "api.example.com");
        assert_eq!(url.port(), 8443);
        assert_eq!(url.path(), "/v1/chat");
        assert_eq!(url.query(), Some("x=1"));
        assert_eq!(url.request_target(), "/v1/chat?x=1");
        assert_eq!(url.authority(), "api.example.com:8443");
    }

    #[test]
    fn applies_default_ports() {
        let url = Url::parse("http://example.com").unwrap();
        assert_eq!(url.port(), 80);
        assert_eq!(url.path(), "/");
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(url.port(), 443);
        assert_eq!(url.authority(), "example.com");
    }

    #[test]
    fn parses_ipv6_hosts() {
        let url = Url::parse("http://[::1]:11434/api/chat").unwrap();
        assert_eq!(url.host(), "::1");
        assert_eq!(url.port(), 11434);
        assert_eq!(url.path(), "/api/chat");
    }

    #[test]
    fn rejects_invalid_urls() {
        assert!(Url::parse("").is_err());
        assert!(Url::parse("example.com").is_err());
        assert!(Url::parse("ftp://example.com").is_err());
        assert!(Url::parse("https://example.com:notaport").is_err());
    }

    #[test]
    fn lenient_parsing_defaults_scheme() {
        let url = Url::parse_lenient("api.example.com/v1").unwrap();
        assert_eq!(url.scheme(), Scheme::Https);

        let url = Url::parse_lenient("localhost:11434").unwrap();
        assert_eq!(url.scheme(), Scheme::Http);
        assert_eq!(url.port(), 11434);

        let url = Url::parse_lenient("http://example.com").unwrap();
        assert_eq!(url.scheme(), Scheme::Http);
    }

    #[test]
    fn joins_paths() {
        let url = Url::parse("https://api.example.com/v1").unwrap();
        let joined = url.join_path("/chat/completions");
        assert_eq!(joined.path(), "/v1/chat/completions");
        assert_eq!(url.join_path("models").path(), "/v1/models");
    }

    #[test]
    fn detects_loopback_hosts() {
        assert!(Url::parse("http://127.0.0.1:8080").unwrap().is_loopback());
        assert!(Url::parse("http://localhost:8080").unwrap().is_loopback());
        assert!(Url::parse("http://[::1]:8080").unwrap().is_loopback());
        assert!(!Url::parse("https://api.example.com").unwrap().is_loopback());
    }
}
