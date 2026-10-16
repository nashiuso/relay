//! Tests for the provider adapters against a local mock HTTP server.

use relay_rs::error::ErrorKind;
use relay_rs::providers::traits::ChatProvider;
use relay_rs::providers::{
    OllamaOptions, OllamaProvider, OpenAiCompatibleOptions, OpenAiCompatibleProvider,
};
use relay_rs::testing::{MockResponse, MockServer};
use relay_rs::types::{ChatRequest, Role};

fn openai(server: &MockServer, key: Option<&str>) -> OpenAiCompatibleProvider {
    let options = OpenAiCompatibleOptions {
        api_key: key.map(relay_rs::SecretString::new),
        default_model: Some("default-model".to_owned()),
        ..OpenAiCompatibleOptions::default()
    };
    OpenAiCompatibleProvider::new(
        "openai",
        &format!("http://{}/v1", server.address()),
        options,
    )
    .expect("the provider could not be created")
}

fn ollama(server: &MockServer) -> OllamaProvider {
    OllamaProvider::new(
        "ollama",
        &format!("http://{}", server.address()),
        OllamaOptions::default(),
    )
    .expect("the provider could not be created")
}

#[test]
fn openai_compatible_posts_to_the_chat_completions_route() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"id":"1","model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#,
    )]);
    let provider = openai(&server, Some("sk-test"));

    let response = provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the request failed");

    assert_eq!(response.text(), "hi");
    assert_eq!(response.provider(), "openai");
    assert_eq!(response.model(), "gpt-4o-mini");
    assert_eq!(response.usage().unwrap().total_tokens, Some(6));

    let request = server.last_request().expect("no request was recorded");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(request.header("content-type"), Some("application/json"));

    let body = request.body_json().expect("the body is not JSON");
    assert_eq!(body.get("model").unwrap().as_str(), Some("gpt-4o-mini"));
    assert_eq!(body.get("stream").unwrap().as_bool(), Some(false));
    let messages = body.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages[0].get("role").unwrap().as_str(), Some("user"));
    assert_eq!(messages[0].get("content").unwrap().as_str(), Some("hello"));
}

#[test]
fn openai_compatible_omits_the_authorization_header_without_a_key() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"choices":[{"message":{"content":"hi"}}]}"#,
    )]);
    let provider = openai(&server, None);
    provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the request failed");
    let request = server.last_request().unwrap();
    assert!(request.header("authorization").is_none());
}

#[test]
fn openai_compatible_maps_the_status_code_to_an_error() {
    let server = MockServer::start(vec![MockResponse::new(
        401,
        r#"{"error":{"message":"invalid api key"}}"#,
    )]);
    let provider = openai(&server, Some("sk-wrong"));
    let error = provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Auth);
    assert!(error.message().contains("invalid api key"));
    assert_eq!(error.provider_name(), Some("openai"));
}

#[test]
fn openai_compatible_reports_rate_limits() {
    let server = MockServer::start(vec![MockResponse::new(429, "slow down")]);
    let provider = openai(&server, None);
    let error = provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::RateLimited);
}

#[test]
fn openai_compatible_reports_server_errors() {
    let server = MockServer::start(vec![MockResponse::new(503, "unavailable")]);
    let provider = openai(&server, None);
    let error = provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Server);
}

#[test]
fn openai_compatible_rejects_malformed_responses() {
    let server = MockServer::start(vec![MockResponse::json("not json at all")]);
    let provider = openai(&server, None);
    let error = provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Decode);
}

#[test]
fn openai_compatible_reports_missing_fields() {
    let server = MockServer::start(vec![MockResponse::json(r#"{"unexpected":true}"#)]);
    let provider = openai(&server, None);
    let error = provider
        .complete(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Decode);
    assert!(error.message().contains("choices"));
}

#[test]
fn openai_compatible_lists_the_models() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"data":[{"id":"gpt-4o-mini","owned_by":"openai"},{"id":"gpt-4o"}]}"#,
    )]);
    let provider = openai(&server, None);
    let models = provider.list_models().expect("the model list failed");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id(), "gpt-4o-mini");
    assert_eq!(models[0].owned_by(), Some("openai"));

    let request = server.last_request().unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/v1/models");
}

#[test]
fn ollama_posts_to_the_chat_route() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"model":"llama3.2","message":{"role":"assistant","content":"hi"},"done":true,"prompt_eval_count":7,"eval_count":3}"#,
    )]);
    let provider = ollama(&server);

    let response = provider
        .complete(&ChatRequest::new("ollama/llama3.2").user("hello"))
        .expect("the request failed");

    assert_eq!(response.text(), "hi");
    assert_eq!(response.provider(), "ollama");
    let usage = response.usage().expect("no usage reported");
    assert_eq!(usage.prompt_tokens, Some(7));
    assert_eq!(usage.completion_tokens, Some(3));
    assert_eq!(usage.total_tokens, Some(10));

    let request = server.last_request().unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/api/chat");
    let body = request.body_json().unwrap();
    assert_eq!(body.get("model").unwrap().as_str(), Some("llama3.2"));
    assert_eq!(body.get("stream").unwrap().as_bool(), Some(false));
}

#[test]
fn ollama_lists_the_models() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"models":[{"name":"llama3.2:latest"},{"name":"qwen2.5:7b"}]}"#,
    )]);
    let provider = ollama(&server);
    let models = provider.list_models().expect("the model list failed");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id(), "llama3.2:latest");
    let request = server.last_request().unwrap();
    assert_eq!(request.path, "/api/tags");
}

#[test]
fn ollama_reports_the_server_version() {
    let server = MockServer::start(vec![MockResponse::json(r#"{"version":"0.5.7"}"#)]);
    let provider = ollama(&server);
    assert_eq!(provider.version().unwrap(), "0.5.7");
}

#[test]
fn ollama_maps_errors() {
    let server = MockServer::start(vec![MockResponse::new(404, "model not found")]);
    let provider = ollama(&server);
    let error = provider
        .complete(&ChatRequest::new("ollama/missing").user("hello"))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert_eq!(error.provider_name(), Some("ollama"));
}

#[test]
fn system_messages_are_forwarded() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"choices":[{"message":{"content":"hi"}}]}"#,
    )]);
    let provider = openai(&server, None);
    provider
        .complete(
            &ChatRequest::new("openai/gpt-4o-mini")
                .message(Role::System, "be brief")
                .user("hello"),
        )
        .expect("the request failed");

    let body = server.last_request().unwrap().body_json().unwrap();
    let messages = body.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].get("role").unwrap().as_str(), Some("system"));
}

#[test]
fn request_options_are_forwarded() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"choices":[{"message":{"content":"hi"}}]}"#,
    )]);
    let provider = openai(&server, None);
    provider
        .complete(
            &ChatRequest::new("openai/gpt-4o-mini")
                .user("hello")
                .temperature(0.25)
                .max_tokens(64)
                .stop("END"),
        )
        .expect("the request failed");

    let body = server.last_request().unwrap().body_json().unwrap();
    assert_eq!(body.get("temperature").unwrap().as_f64(), Some(0.25));
    assert_eq!(body.get("max_tokens").unwrap().as_i64(), Some(64));
    let stop = body.get("stop").unwrap().as_array().unwrap();
    assert_eq!(stop[0].as_str(), Some("END"));
}
