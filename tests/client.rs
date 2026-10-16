//! Tests for the central client: model resolution, retries and fallback.

use std::sync::Arc;
use std::time::Duration;

use relay_rs::error::ErrorKind;
use relay_rs::providers::traits::ChatProvider;
use relay_rs::providers::{OpenAiCompatibleOptions, OpenAiCompatibleProvider};
use relay_rs::testing::{MockResponse, MockServer};
use relay_rs::types::ChatRequest;
use relay_rs::{Client, ClientBuilder};

fn provider(server: &MockServer, name: &str) -> Arc<dyn ChatProvider> {
    Arc::new(
        OpenAiCompatibleProvider::new(
            name,
            &format!("http://{}/v1", server.address()),
            OpenAiCompatibleOptions::default(),
        )
        .expect("the provider could not be created"),
    )
}

fn builder() -> ClientBuilder {
    Client::builder()
        .max_retries(0)
        .retry_backoff(Duration::from_millis(1))
}

#[test]
fn sends_the_request_to_the_selected_provider() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"choices":[{"message":{"content":"hi"}}]}"#,
    )]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .build()
        .unwrap();

    let response = client
        .complete_blocking(ChatRequest::new("alpha/model").user("hello"))
        .expect("the request failed");

    assert_eq!(response.text(), "hi");
    assert_eq!(response.provider(), "alpha");
    assert_eq!(server.request_count(), 1);
}

#[test]
fn uses_the_default_provider_for_a_bare_model_name() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"choices":[{"message":{"content":"hi"}}]}"#,
    )]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .default_model("alpha/default-model")
        .build()
        .unwrap();

    let response = client
        .complete_blocking(ChatRequest::new("default-model").user("hello"))
        .expect("the request failed");

    assert_eq!(response.provider(), "alpha");
    let body = server.last_request().unwrap().body_json().unwrap();
    assert_eq!(body.get("model").unwrap().as_str(), Some("default-model"));
}

#[test]
fn reports_an_unknown_provider() {
    let server = MockServer::start(vec![MockResponse::json("{}")]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .build()
        .unwrap();

    let error = client
        .complete_blocking(ChatRequest::new("beta/model").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UnknownProvider);
    assert!(error.message().contains("alpha"));
}

#[test]
fn retries_transient_failures_a_bounded_number_of_times() {
    let server = MockServer::start(vec![MockResponse::new(500, "boom")]);
    let client = Client::builder()
        .max_retries(2)
        .retry_backoff(Duration::from_millis(1))
        .provider("alpha", provider(&server, "alpha"))
        .build()
        .unwrap();

    let error = client
        .complete_blocking(ChatRequest::new("alpha/model").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Server);
    assert_eq!(server.request_count(), 3, "one attempt and two retries");
}

#[test]
fn does_not_retry_permanent_failures() {
    let server = MockServer::start(vec![MockResponse::new(401, "no key")]);
    let client = Client::builder()
        .max_retries(3)
        .retry_backoff(Duration::from_millis(1))
        .provider("alpha", provider(&server, "alpha"))
        .build()
        .unwrap();

    let error = client
        .complete_blocking(ChatRequest::new("alpha/model").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Auth);
    assert_eq!(server.request_count(), 1);
}

#[test]
fn does_not_fall_back_by_default() {
    let server = MockServer::start(vec![MockResponse::new(500, "boom")]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .provider("beta", provider(&server, "beta"))
        .build()
        .unwrap();

    let error = client
        .complete_blocking(ChatRequest::new("alpha/model").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Server);
    assert_eq!(server.request_count(), 1);
}

#[test]
fn falls_back_to_a_configured_alternative() {
    let server = MockServer::start(vec![
        MockResponse::new(500, "boom"),
        MockResponse::json(r#"{"choices":[{"message":{"content":"from beta"}}]}"#),
    ]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .provider("beta", provider(&server, "beta"))
        .fallback(true, vec!["beta/model".to_owned()])
        .build()
        .unwrap();

    let response = client
        .complete_blocking(ChatRequest::new("alpha/model").user("hello"))
        .expect("the request failed");

    assert_eq!(response.provider(), "beta");
    assert_eq!(response.text(), "from beta");
    assert_eq!(server.request_count(), 2);
}

#[test]
fn never_falls_back_for_permanent_failures() {
    let server = MockServer::start(vec![MockResponse::new(401, "no key")]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .provider("beta", provider(&server, "beta"))
        .fallback(true, vec!["beta/model".to_owned()])
        .build()
        .unwrap();

    let error = client
        .complete_blocking(ChatRequest::new("alpha/model").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Auth);
    assert_eq!(server.request_count(), 1);
}

#[test]
fn lists_models_and_caches_them() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"data":[{"id":"model-a"},{"id":"model-b"}]}"#,
    )]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .build()
        .unwrap();

    let models = client.models("alpha").expect("the model list failed");
    assert_eq!(models.len(), 2);
    let models = client.models("alpha").expect("the model list failed");
    assert_eq!(models.len(), 2);
    assert_eq!(server.request_count(), 1, "the second call used the cache");

    client
        .refresh_models("alpha")
        .expect("the model list failed");
    assert_eq!(server.request_count(), 2, "the refresh bypassed the cache");
}

#[test]
fn a_client_without_providers_is_rejected() {
    let error = Client::builder().build().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config);
}

#[test]
fn reports_the_configured_providers() {
    let server = MockServer::start(vec![MockResponse::json("{}")]);
    let client = builder()
        .provider("alpha", provider(&server, "alpha"))
        .build()
        .unwrap();

    let providers = client.providers();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].name, "alpha");
    assert_eq!(providers[0].kind, relay_rs::ProviderKind::OpenAiCompatible);
}
