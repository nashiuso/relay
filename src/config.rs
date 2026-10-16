//! Configuration of Relay.
//!
//! Configuration comes from three places, in this order of precedence:
//!
//! 1. explicit command line options,
//! 2. environment variables prefixed with `RELAY_`,
//! 3. the user configuration file,
//! 4. built-in defaults.
//!
//! The configuration file is TOML. A minimal file looks like this:
//!
//! ```toml
//! default_model = "ollama/llama3.2"
//!
//! [providers.ollama]
//! type = "ollama"
//! base_url = "http://127.0.0.1:11434"
//! model = "llama3.2"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::error::RelayError;
use crate::http::tls::TlsConfig;
use crate::json::Value;
use crate::providers::traits::ChatProvider;
use crate::providers::{
    OllamaOptions, OllamaProvider, OpenAiCompatibleOptions, OpenAiCompatibleProvider,
};
use crate::secret::SecretString;

/// The default configuration file name.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The default request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// The default number of retries for transient failures.
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// The default backoff between retries.
pub const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// The kind of a configured provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    /// An OpenAI compatible HTTP API.
    OpenAiCompatible,
    /// A local Ollama server.
    Ollama,
}

impl ProviderType {
    /// Parses the `type` key of a provider.
    pub fn parse(value: &str) -> Result<ProviderType, RelayError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "openai_compatible" => {
                Ok(ProviderType::OpenAiCompatible)
            }
            "ollama" => Ok(ProviderType::Ollama),
            other => Err(RelayError::config(format!(
                "unknown provider type `{other}`; expected `openai-compatible` or `ollama`"
            ))),
        }
    }

    /// The canonical name of the provider type.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderType::OpenAiCompatible => "openai-compatible",
            ProviderType::Ollama => "ollama",
        }
    }
}

/// The configuration of a single provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// The kind of the provider.
    pub provider_type: Option<ProviderType>,
    /// The base URL of the API.
    pub base_url: Option<String>,
    /// The API key, when it is stored in the configuration file.
    ///
    /// Prefer [`ProviderConfig::api_key_env`] so that secrets stay out of
    /// files that may be shared.
    pub api_key: Option<SecretString>,
    /// The environment variable that holds the API key.
    pub api_key_env: Option<String>,
    /// The default model of the provider.
    pub model: Option<String>,
    /// The request timeout of the provider.
    pub timeout: Option<Duration>,
    /// Additional headers sent with every request.
    pub headers: Vec<(String, String)>,
    /// Skip TLS certificate verification. Only for local development.
    pub insecure: bool,
}

impl ProviderConfig {
    /// Resolves the API key from the environment when needed.
    pub fn resolve_api_key(&self) -> Option<SecretString> {
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        let variable = self.api_key_env.as_deref()?;
        std::env::var(variable)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(SecretString::new)
    }

    /// Builds the provider described by this configuration.
    ///
    /// `default_timeout` is used when the provider does not define its own.
    pub fn build(
        &self,
        name: &str,
        default_timeout: Duration,
    ) -> Result<Arc<dyn ChatProvider>, RelayError> {
        let provider_type = self.provider_type.ok_or_else(|| {
            RelayError::config(format!(
                "provider `{name}` is missing a `type`; expected `openai-compatible` or `ollama`"
            ))
        })?;

        let timeout = Some(self.timeout.unwrap_or(default_timeout));
        match provider_type {
            ProviderType::OpenAiCompatible => {
                let base_url = self.base_url.as_deref().ok_or_else(|| {
                    RelayError::config(format!(
                        "provider `{name}` is missing a `base_url`, for example `https://api.openai.com/v1`"
                    ))
                })?;
                let options = OpenAiCompatibleOptions {
                    api_key: self.resolve_api_key(),
                    default_model: self.model.clone(),
                    timeout,
                    connect_timeout: None,
                    headers: self.headers.clone(),
                    tls: TlsConfig::new().insecure(self.insecure),
                };
                Ok(Arc::new(OpenAiCompatibleProvider::new(
                    name, base_url, options,
                )?))
            }
            ProviderType::Ollama => {
                let base_url = self
                    .base_url
                    .as_deref()
                    .unwrap_or(crate::providers::DEFAULT_OLLAMA_URL);
                let options = OllamaOptions {
                    default_model: self.model.clone(),
                    timeout,
                    connect_timeout: None,
                    api_key: self.resolve_api_key(),
                    headers: self.headers.clone(),
                    tls: TlsConfig::new().insecure(self.insecure),
                };
                Ok(Arc::new(OllamaProvider::new(name, base_url, options)?))
            }
        }
    }
}

/// The fallback behaviour of the client.
#[derive(Debug, Clone, Default)]
pub struct FallbackConfig {
    /// Whether failed requests may be retried on another provider.
    pub enabled: bool,
    /// The models to try when the primary model fails.
    pub models: Vec<String>,
}

/// The configuration of the optional HTTP server.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The interface the server binds to.
    pub host: String,
    /// The port the server listens on.
    pub port: u16,
    /// The API key clients must send.
    pub api_key: Option<SecretString>,
    /// The environment variable that holds the API key.
    pub api_key_env: Option<String>,
    /// Whether the server may be exposed to a remote network.
    pub allow_remote: bool,
    /// The maximum size of a request body.
    pub max_body_bytes: usize,
    /// The origins allowed to call the server from a browser.
    pub cors_allow_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 8080,
            api_key: None,
            api_key_env: None,
            allow_remote: false,
            max_body_bytes: 1024 * 1024,
            cors_allow_origins: Vec::new(),
        }
    }
}

impl ServerConfig {
    /// Resolves the API key of the server.
    pub fn resolve_api_key(&self) -> Option<SecretString> {
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        let variable = self.api_key_env.as_deref()?;
        std::env::var(variable)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(SecretString::new)
    }
}

/// The full configuration of Relay.
#[derive(Debug, Clone)]
pub struct Config {
    /// The model used when a request does not name one.
    pub default_model: Option<String>,
    /// The request timeout.
    pub timeout: Duration,
    /// The number of retries for transient failures.
    pub max_retries: u32,
    /// The backoff between retries.
    pub retry_backoff: Duration,
    /// The fallback behaviour.
    pub fallback: FallbackConfig,
    /// The configured providers, keyed by name.
    pub providers: BTreeMap<String, ProviderConfig>,
    /// The configuration of the optional server.
    pub server: ServerConfig,
    /// The file the configuration was loaded from.
    pub path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_model: None,
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_backoff: DEFAULT_RETRY_BACKOFF,
            fallback: FallbackConfig::default(),
            providers: BTreeMap::new(),
            server: ServerConfig::default(),
            path: None,
        }
    }
}

impl Config {
    /// Loads the configuration from the default location.
    pub fn load() -> Result<Config, RelayError> {
        let path = std::env::var_os("RELAY_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(Config::default_path);
        let mut config = if path.exists() {
            Config::from_path(&path)?
        } else {
            Config::default()
        };
        config.path = Some(path);
        config.apply_env(|key| std::env::var(key).ok())?;
        Ok(config)
    }

    /// Loads the configuration from a file.
    pub fn from_path(path: &Path) -> Result<Config, RelayError> {
        let text = std::fs::read_to_string(path).map_err(|err| {
            RelayError::config(format!("could not read `{}`: {err}", path.display()))
        })?;
        let mut config = Config::parse(&text)?;
        config.path = Some(path.to_path_buf());
        Ok(config)
    }

    /// Parses a configuration from TOML text.
    pub fn parse(text: &str) -> Result<Config, RelayError> {
        let document = crate::toml::parse(text)?;
        let mut config = Config::default();
        let object = document
            .as_object()
            .ok_or_else(|| RelayError::config("the configuration must be a table"))?;

        for (key, value) in object.iter() {
            match key {
                "default_model" => {
                    config.default_model = Some(expect_string(key, value)?);
                }
                "timeout_secs" => {
                    config.timeout = Duration::from_secs(expect_u64(key, value)?);
                }
                "max_retries" => {
                    config.max_retries = expect_u64(key, value)?.min(10) as u32;
                }
                "retry_backoff_ms" => {
                    config.retry_backoff =
                        Duration::from_millis(expect_u64(key, value)?.min(60_000));
                }
                "fallback" => {
                    config.fallback = parse_fallback(value)?;
                }
                "providers" => {
                    config.providers = parse_providers(value)?;
                }
                "server" => {
                    config.server = parse_server(&config.server, value)?;
                }
                other => {
                    return Err(RelayError::config(format!(
                        "unknown configuration key `{other}`"
                    )));
                }
            }
        }
        Ok(config)
    }

    /// Applies environment variable overrides.
    ///
    /// The lookup function is injected so that the behaviour can be tested
    /// without touching the process environment.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<(), RelayError> {
        if let Some(value) = get("RELAY_DEFAULT_MODEL") {
            if !value.trim().is_empty() {
                self.default_model = Some(value.trim().to_owned());
            }
        }
        if let Some(value) = get("RELAY_TIMEOUT_SECS") {
            self.timeout = Duration::from_secs(parse_u64_env("RELAY_TIMEOUT_SECS", &value)?);
        }
        if let Some(value) = get("RELAY_MAX_RETRIES") {
            self.max_retries = parse_u64_env("RELAY_MAX_RETRIES", &value)?.min(10) as u32;
        }
        if let Some(value) = get("RELAY_RETRY_BACKOFF_MS") {
            self.retry_backoff =
                Duration::from_millis(parse_u64_env("RELAY_RETRY_BACKOFF_MS", &value)?.min(60_000));
        }
        if let Some(value) = get("RELAY_FALLBACK_ENABLED") {
            self.fallback.enabled = parse_bool_env("RELAY_FALLBACK_ENABLED", &value)?;
        }
        if let Some(value) = get("RELAY_FALLBACK_MODELS") {
            self.fallback.models = value
                .split(',')
                .map(|item| item.trim().to_owned())
                .filter(|item| !item.is_empty())
                .collect();
        }
        if let Some(value) = get("RELAY_SERVER_HOST") {
            self.server.host = value.trim().to_owned();
        }
        if let Some(value) = get("RELAY_SERVER_PORT") {
            self.server.port = parse_u64_env("RELAY_SERVER_PORT", &value)? as u16;
        }
        if let Some(value) = get("RELAY_SERVER_API_KEY") {
            self.server.api_key = Some(SecretString::new(value.trim()));
        }
        if let Some(value) = get("RELAY_SERVER_ALLOW_REMOTE") {
            self.server.allow_remote = parse_bool_env("RELAY_SERVER_ALLOW_REMOTE", &value)?;
        }
        if let Some(value) = get("RELAY_SERVER_MAX_BODY_BYTES") {
            self.server.max_body_bytes =
                parse_u64_env("RELAY_SERVER_MAX_BODY_BYTES", &value)? as usize;
        }

        for name in self.providers.keys().cloned().collect::<Vec<_>>() {
            let prefix = format!("RELAY_PROVIDER_{}", env_key(&name));
            let provider = self
                .providers
                .get_mut(&name)
                .expect("the provider was collected from the map");
            if let Some(value) = get(&format!("{prefix}_TYPE")) {
                provider.provider_type = Some(ProviderType::parse(&value)?);
            }
            if let Some(value) = get(&format!("{prefix}_BASE_URL")) {
                provider.base_url = Some(value.trim().to_owned());
            }
            if let Some(value) = get(&format!("{prefix}_API_KEY")) {
                provider.api_key = Some(SecretString::new(value.trim()));
            }
            if let Some(value) = get(&format!("{prefix}_API_KEY_ENV")) {
                provider.api_key_env = Some(value.trim().to_owned());
            }
            if let Some(value) = get(&format!("{prefix}_MODEL")) {
                provider.model = Some(value.trim().to_owned());
            }
            if let Some(value) = get(&format!("{prefix}_TIMEOUT_SECS")) {
                provider.timeout = Some(Duration::from_secs(parse_u64_env(
                    &format!("{prefix}_TIMEOUT_SECS"),
                    &value,
                )?));
            }
        }
        Ok(())
    }

    /// The default location of the configuration file.
    pub fn default_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(dir).join("relay").join(CONFIG_FILE_NAME);
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("relay").join(CONFIG_FILE_NAME)
    }

    /// The names of the configured providers.
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}

fn env_key(name: &str) -> String {
    name.to_ascii_uppercase().replace(['-', '.'], "_")
}

fn parse_fallback(value: &Value) -> Result<FallbackConfig, RelayError> {
    let mut fallback = FallbackConfig::default();
    let object = value
        .as_object()
        .ok_or_else(|| RelayError::config("`[fallback]` must be a table"))?;
    for (key, item) in object.iter() {
        match key {
            "enabled" => fallback.enabled = expect_bool(key, item)?,
            "models" => {
                let models = item.as_array().ok_or_else(|| {
                    RelayError::config("`fallback.models` must be an array of strings")
                })?;
                fallback.models = models
                    .iter()
                    .map(|model| {
                        model.as_str().map(str::to_owned).ok_or_else(|| {
                            RelayError::config("`fallback.models` must be an array of strings")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
            other => {
                return Err(RelayError::config(format!(
                    "unknown configuration key `fallback.{other}`"
                )));
            }
        }
    }
    Ok(fallback)
}

fn parse_providers(value: &Value) -> Result<BTreeMap<String, ProviderConfig>, RelayError> {
    let mut providers = BTreeMap::new();
    let object = value
        .as_object()
        .ok_or_else(|| RelayError::config("`[providers]` must be a table"))?;
    for (name, item) in object.iter() {
        let mut config = ProviderConfig::default();
        let entries = item
            .as_object()
            .ok_or_else(|| RelayError::config(format!("`[providers.{name}]` must be a table")))?;
        for (key, entry) in entries.iter() {
            match key {
                "type" => {
                    config.provider_type =
                        Some(ProviderType::parse(entry.as_str().ok_or_else(|| {
                            RelayError::config(format!("`providers.{name}.type` must be a string"))
                        })?)?)
                }
                "base_url" => {
                    config.base_url = Some(expect_string(key, entry)?);
                }
                "api_key" => {
                    config.api_key = Some(SecretString::new(expect_string(key, entry)?));
                }
                "api_key_env" => {
                    config.api_key_env = Some(expect_string(key, entry)?);
                }
                "model" => {
                    config.model = Some(expect_string(key, entry)?);
                }
                "timeout_secs" => {
                    config.timeout = Some(Duration::from_secs(expect_u64(key, entry)?));
                }
                "insecure" => {
                    config.insecure = expect_bool(key, entry)?;
                }
                "headers" => {
                    let headers = entry.as_object().ok_or_else(|| {
                        RelayError::config(format!("`providers.{name}.headers` must be a table"))
                    })?;
                    config.headers = headers
                        .iter()
                        .map(|(header, value)| {
                            value
                                .as_str()
                                .map(|value| (header.to_owned(), value.to_owned()))
                                .ok_or_else(|| {
                                    RelayError::config(format!(
                                        "`providers.{name}.headers.{header}` must be a string"
                                    ))
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                }
                other => {
                    return Err(RelayError::config(format!(
                        "unknown configuration key `providers.{name}.{other}`"
                    )));
                }
            }
        }
        providers.insert(name.to_owned(), config);
    }
    Ok(providers)
}

fn parse_server(defaults: &ServerConfig, value: &Value) -> Result<ServerConfig, RelayError> {
    let mut server = defaults.clone();
    let object = value
        .as_object()
        .ok_or_else(|| RelayError::config("`[server]` must be a table"))?;
    for (key, item) in object.iter() {
        match key {
            "host" => server.host = expect_string(key, item)?,
            "port" => {
                let port = expect_u64(key, item)?;
                if port == 0 || port > 65535 {
                    return Err(RelayError::config(
                        "`server.port` must be between 1 and 65535".to_owned(),
                    ));
                }
                server.port = port as u16;
            }
            "api_key" => server.api_key = Some(SecretString::new(expect_string(key, item)?)),
            "api_key_env" => server.api_key_env = Some(expect_string(key, item)?),
            "allow_remote" => server.allow_remote = expect_bool(key, item)?,
            "max_body_bytes" => server.max_body_bytes = expect_u64(key, item)? as usize,
            "cors_allow_origins" => {
                let origins = item.as_array().ok_or_else(|| {
                    RelayError::config("`server.cors_allow_origins` must be an array of strings")
                })?;
                server.cors_allow_origins = origins
                    .iter()
                    .map(|origin| {
                        origin.as_str().map(str::to_owned).ok_or_else(|| {
                            RelayError::config(
                                "`server.cors_allow_origins` must be an array of strings",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
            other => {
                return Err(RelayError::config(format!(
                    "unknown configuration key `server.{other}`"
                )));
            }
        }
    }
    Ok(server)
}

fn expect_string(key: &str, value: &Value) -> Result<String, RelayError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| RelayError::config(format!("`{key}` must be a string")))
}

fn expect_bool(key: &str, value: &Value) -> Result<bool, RelayError> {
    value
        .as_bool()
        .ok_or_else(|| RelayError::config(format!("`{key}` must be a boolean")))
}

fn expect_u64(key: &str, value: &Value) -> Result<u64, RelayError> {
    value
        .as_i64()
        .filter(|value| *value >= 0)
        .map(|value| value as u64)
        .ok_or_else(|| RelayError::config(format!("`{key}` must be a non-negative integer")))
}

fn parse_u64_env(name: &str, value: &str) -> Result<u64, RelayError> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| RelayError::config(format!("`{name}` must be a number, got `{value}`")))
}

fn parse_bool_env(name: &str, value: &str) -> Result<bool, RelayError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(RelayError::config(format!(
            "`{name}` must be a boolean, got `{value}`"
        ))),
    }
}

/// The configuration template written by `relay init`.
pub const TEMPLATE: &str = r#"# Relay configuration
#
# Values are read from this file, from RELAY_* environment variables and from
# command line options, in that order of precedence.

# The model used when a request does not name one.
default_model = "ollama/llama3.2"

# Request timeout in seconds.
timeout_secs = 120

# Number of retries for transient failures (network errors, timeouts, 429 and
# 5xx answers). Permanent failures are never retried.
max_retries = 2

[providers.ollama]
type = "ollama"
base_url = "http://127.0.0.1:11434"
model = "llama3.2"

# An OpenAI compatible endpoint. The key is read from the environment so that
# it never ends up in a file you might share.
[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o-mini"

# The optional OpenAI compatible server (`relay serve`).
[server]
host = "127.0.0.1"
port = 8080
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn parses_a_minimal_configuration() {
        let config = Config::parse("default_model = \"ollama/llama3.2\"\n").unwrap();
        assert_eq!(config.default_model.as_deref(), Some("ollama/llama3.2"));
        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
        assert!(config.providers.is_empty());
    }

    #[test]
    fn parses_providers() {
        let config = Config::parse(
            r#"
            [providers.openai]
            type = "openai-compatible"
            base_url = "https://api.example.com/v1"
            api_key_env = "EXAMPLE_KEY"
            model = "gpt-4o-mini"
            timeout_secs = 30
            "#,
        )
        .unwrap();
        let provider = config.providers.get("openai").unwrap();
        assert_eq!(provider.provider_type, Some(ProviderType::OpenAiCompatible));
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(provider.api_key_env.as_deref(), Some("EXAMPLE_KEY"));
        assert_eq!(provider.timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn rejects_unknown_keys() {
        let error = Config::parse("nonsense = 1\n").unwrap_err();
        assert!(error.message().contains("unknown configuration key"));
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(Config::parse("timeout_secs = \"soon\"\n").is_err());
        assert!(Config::parse("[server]\nport = 70000\n").is_err());
        assert!(Config::parse("[providers.x]\ntype = \"nope\"\n").is_err());
    }

    #[test]
    fn environment_overrides_the_file() {
        let mut config =
            Config::parse("default_model = \"ollama/a\"\ntimeout_secs = 10\n").unwrap();
        config
            .apply_env(env(&[
                ("RELAY_DEFAULT_MODEL", "openai/b"),
                ("RELAY_TIMEOUT_SECS", "42"),
            ]))
            .unwrap();
        assert_eq!(config.default_model.as_deref(), Some("openai/b"));
        assert_eq!(config.timeout, Duration::from_secs(42));
    }

    #[test]
    fn environment_overrides_single_providers() {
        let mut config = Config::parse(
            "[providers.openai]\ntype = \"openai-compatible\"\nbase_url = \"https://a/v1\"\n",
        )
        .unwrap();
        config
            .apply_env(env(&[
                ("RELAY_PROVIDER_OPENAI_BASE_URL", "https://b/v1"),
                ("RELAY_PROVIDER_OPENAI_MODEL", "gpt-4o"),
            ]))
            .unwrap();
        let provider = config.providers.get("openai").unwrap();
        assert_eq!(provider.base_url.as_deref(), Some("https://b/v1"));
        assert_eq!(provider.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn rejects_malformed_environment_values() {
        let mut config = Config::default();
        assert!(
            config
                .apply_env(env(&[("RELAY_TIMEOUT_SECS", "soon")]))
                .is_err()
        );
        assert!(
            config
                .apply_env(env(&[("RELAY_FALLBACK_ENABLED", "maybe")]))
                .is_err()
        );
    }

    #[test]
    fn server_defaults_are_safe() {
        let config = Config::default();
        assert_eq!(config.server.host, "127.0.0.1");
        assert!(!config.server.allow_remote);
        assert!(config.server.api_key.is_none());
    }

    #[test]
    fn builds_providers() {
        let config = Config::parse(
            "[providers.ollama]\ntype = \"ollama\"\nbase_url = \"http://127.0.0.1:11434\"\n",
        )
        .unwrap();
        let provider = config
            .providers
            .get("ollama")
            .unwrap()
            .build("ollama", Duration::from_secs(5))
            .unwrap();
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn reports_a_missing_base_url() {
        let config = Config::parse("[providers.openai]\ntype = \"openai-compatible\"\n").unwrap();
        let error = config
            .providers
            .get("openai")
            .unwrap()
            .build("openai", Duration::from_secs(5))
            .unwrap_err();
        assert!(error.message().contains("base_url"));
    }
}
