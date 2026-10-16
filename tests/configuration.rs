//! Tests for the configuration: files, environment variables and secrets.

use std::collections::BTreeMap;
use std::path::PathBuf;

use relay_rs::config::ProviderType;
use relay_rs::error::ErrorKind;
use relay_rs::{Config, SecretString};

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

#[test]
fn reads_a_configuration_file() {
    let config = Config::parse(
        r#"
        default_model = "ollama/llama3.2"
        timeout_secs = 30
        max_retries = 1

        [providers.ollama]
        type = "ollama"
        base_url = "http://127.0.0.1:11434"
        model = "llama3.2"

        [providers.openai]
        type = "openai-compatible"
        base_url = "https://api.example.com/v1"
        api_key_env = "EXAMPLE_KEY"
        model = "example-model"
        "#,
    )
    .expect("the configuration is invalid");

    assert_eq!(config.default_model.as_deref(), Some("ollama/llama3.2"));
    assert_eq!(config.timeout, std::time::Duration::from_secs(30));
    assert_eq!(config.max_retries, 1);
    assert_eq!(config.providers.len(), 2);
    assert_eq!(
        config.providers.get("ollama").unwrap().provider_type,
        Some(ProviderType::Ollama)
    );
}

#[test]
fn the_environment_wins_over_the_file() {
    let mut config = Config::parse(
        r#"
        default_model = "ollama/a"
        timeout_secs = 10

        [providers.ollama]
        type = "ollama"
        model = "a"
        "#,
    )
    .unwrap();

    config
        .apply_env(env(&[
            ("RELAY_DEFAULT_MODEL", "openai/b"),
            ("RELAY_TIMEOUT_SECS", "99"),
            ("RELAY_PROVIDER_OLLAMA_MODEL", "b"),
        ]))
        .unwrap();

    assert_eq!(config.default_model.as_deref(), Some("openai/b"));
    assert_eq!(config.timeout, std::time::Duration::from_secs(99));
    assert_eq!(
        config.providers.get("ollama").unwrap().model.as_deref(),
        Some("b")
    );
}

#[test]
fn invalid_configuration_is_reported() {
    let error = Config::parse("default_model = 5\n").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config);
    assert!(error.message().contains("default_model"));

    let error = Config::parse("[providers.x]\ntype = \"unknown\"\n").unwrap_err();
    assert!(error.message().contains("unknown provider type"));

    let error = Config::parse("[server]\nport = 0\n").unwrap_err();
    assert!(error.message().contains("port"));
}

#[test]
fn invalid_environment_values_are_reported() {
    let mut config = Config::default();
    let error = config
        .apply_env(env(&[("RELAY_MAX_RETRIES", "many")]))
        .unwrap_err();
    assert!(error.message().contains("RELAY_MAX_RETRIES"));
}

#[test]
fn secrets_never_show_up_in_diagnostics() {
    let secret = SecretString::new("sk-super-secret");
    assert_eq!(format!("{secret}"), "<redacted>");
    assert_eq!(format!("{secret:?}"), "<redacted>");

    let config = Config::parse(
        r#"
        [providers.openai]
        type = "openai-compatible"
        base_url = "https://api.example.com/v1"
        api_key = "sk-super-secret"
        "#,
    )
    .unwrap();
    let rendered = format!("{:?}", config);
    assert!(
        !rendered.contains("sk-super-secret"),
        "the secret leaked into the debug output: {rendered}"
    );
    assert!(rendered.contains("redacted"));
}

#[test]
fn api_keys_can_be_read_from_the_environment() {
    let config = Config::parse(
        r#"
        [providers.openai]
        type = "openai-compatible"
        base_url = "https://api.example.com/v1"
        api_key_env = "RELAY_TEST_KEY"
        "#,
    )
    .unwrap();
    let provider = config.providers.get("openai").unwrap();
    assert!(provider.resolve_api_key().is_none());
}

#[test]
fn the_default_configuration_is_safe() {
    let config = Config::default();
    assert_eq!(config.server.host, "127.0.0.1");
    assert!(!config.server.allow_remote);
    assert!(config.server.api_key.is_none());
    assert!(!config.fallback.enabled);
    assert_eq!(config.max_retries, relay_rs::config::DEFAULT_MAX_RETRIES);
}

#[test]
fn the_default_path_follows_the_convention() {
    let path: PathBuf = Config::default_path();
    assert!(path.ends_with("relay/config.toml"));
}

#[test]
fn the_template_is_valid() {
    let config = Config::parse(relay_rs::config::TEMPLATE).expect("the template is invalid");
    assert_eq!(config.providers.len(), 2);
    assert_eq!(
        config.providers.get("ollama").unwrap().provider_type,
        Some(ProviderType::Ollama)
    );
}

#[test]
fn the_example_configuration_is_valid() {
    let text = include_str!("../config.example.toml");
    let config = Config::parse(text).expect("config.example.toml is invalid");
    assert_eq!(config.providers.len(), 2);
    assert_eq!(
        config
            .providers
            .get("openai")
            .unwrap()
            .api_key_env
            .as_deref(),
        Some("OPENAI_API_KEY")
    );
    assert_eq!(config.server.host, "127.0.0.1");
    assert!(!config.server.allow_remote);
}
