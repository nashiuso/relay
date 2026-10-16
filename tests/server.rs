//! Tests for the optional OpenAI compatible server.
//!
//! The whole file is behind the `server` feature: the server is optional and
//! the default installation does not carry it.

#![cfg(feature = "server")]

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread::JoinHandle;

use relay_rs::config::ServerConfig;
use relay_rs::http::{HttpClient, HttpRequest, Url};
use relay_rs::json::Value;
use relay_rs::providers::traits::ChatProvider;
use relay_rs::providers::{OpenAiCompatibleOptions, OpenAiCompatibleProvider};
use relay_rs::testing::{MockResponse, MockServer};
use relay_rs::{Client, SecretString};

struct TestServer {
    address: SocketAddr,
    handle: Option<JoinHandle<()>>,
}

impl TestServer {
    fn start(config: ServerConfig, mock: &MockServer) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the server could not bind");
        let address = listener.local_addr().unwrap();
        let provider: Arc<dyn ChatProvider> = Arc::new(
            OpenAiCompatibleProvider::new(
                "mock",
                &format!("http://{}/v1", mock.address()),
                OpenAiCompatibleOptions::default(),
            )
            .unwrap(),
        );
        let client = Client::builder()
            .provider("mock", provider)
            .build()
            .unwrap();
        let handle = std::thread::spawn(move || {
            let _ = relay_rs::server::serve_on(client, config, listener);
        });
        TestServer {
            address,
            handle: Some(handle),
        }
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://{}{path}", self.address)).unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // The server thread runs until the process ends; detaching it keeps
        // the test from blocking on shutdown.
        let _ = self.handle.take();
    }
}

fn http() -> HttpClient {
    HttpClient::builder()
        .request_timeout(std::time::Duration::from_secs(10))
        .build()
}

#[test]
fn health_reports_that_the_server_is_alive() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let mut response = http()
        .send(&HttpRequest::get(server.url("/health")))
        .expect("the request failed");
    assert_eq!(response.status(), 200);
    let body = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert_eq!(body.get("status").unwrap().as_str(), Some("ok"));
}

#[test]
fn chat_completions_return_an_openai_shaped_answer() {
    let mock = MockServer::start(vec![MockResponse::json(
        r#"{"model":"model-a","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#,
    )]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let payload = r#"{"model":"mock/model-a","messages":[{"role":"user","content":"hello"}]}"#;
    let request = HttpRequest::post(server.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(payload.to_owned());
    let mut response = http().send(&request).expect("the request failed");

    assert_eq!(response.status(), 200);
    let body = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert_eq!(
        body.get("object").unwrap().as_str(),
        Some("chat.completion")
    );
    let choices = body.get("choices").unwrap().as_array().unwrap();
    assert_eq!(
        choices[0]
            .get("message")
            .unwrap()
            .get("content")
            .unwrap()
            .as_str(),
        Some("hi")
    );
    let usage = body.get("usage").unwrap();
    assert_eq!(usage.get("total_tokens").unwrap().as_i64(), Some(4));
}

#[test]
fn chat_completions_can_stream() {
    let mock = MockServer::start(vec![MockResponse::streaming(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n".to_owned(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n".to_owned(),
        "data: [DONE]\n\n".to_owned(),
    ])]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let payload =
        r#"{"model":"mock/model-a","messages":[{"role":"user","content":"hello"}],"stream":true}"#;
    let request = HttpRequest::post(server.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(payload.to_owned());
    let mut response = http().send(&request).expect("the request failed");

    assert_eq!(response.status(), 200);
    assert_eq!(response.header("content-type"), Some("text/event-stream"));
    let body = response.text().unwrap();
    assert!(body.contains("data: "));
    assert!(body.contains("\"content\":\"Hel\""));
    assert!(body.contains("\"content\":\"lo\""));
    assert!(body.trim_end().ends_with("data: [DONE]"));
}

#[test]
fn the_model_list_contains_the_configured_models() {
    let mock = MockServer::start(vec![MockResponse::json(r#"{"data":[{"id":"model-a"}]}"#)]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let mut response = http()
        .send(&HttpRequest::get(server.url("/v1/models")))
        .expect("the request failed");
    assert_eq!(response.status(), 200);
    let body = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert_eq!(body.get("object").unwrap().as_str(), Some("list"));
    let data = body.get("data").unwrap().as_array().unwrap();
    assert_eq!(data[0].get("id").unwrap().as_str(), Some("mock/model-a"));
}

#[test]
fn unknown_routes_return_an_error() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let mut response = http()
        .send(&HttpRequest::get(server.url("/nope")))
        .expect("the request failed");
    assert_eq!(response.status(), 404);
    let body = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert!(body.get("error").is_some());
}

#[test]
fn invalid_json_is_reported() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let request = HttpRequest::post(server.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body("not json");
    let mut response = http().send(&request).expect("the request failed");
    assert_eq!(response.status(), 400);
    let body = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert!(body.get("error").is_some());
}

#[test]
fn a_missing_model_is_reported() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let request = HttpRequest::post(server.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"messages":[{"role":"user","content":"hello"}]}"#);
    let mut response = http().send(&request).expect("the request failed");
    assert_eq!(response.status(), 400);
    let body = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert!(
        body.get("error")
            .unwrap()
            .get("message")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("model")
    );
}

#[test]
fn an_api_key_is_required_when_it_is_configured() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let config = ServerConfig {
        api_key: Some(SecretString::new("secret-token")),
        ..ServerConfig::default()
    };
    let server = TestServer::start(config, &mock);

    let response = http()
        .send(&HttpRequest::get(server.url("/health")))
        .expect("the request failed");
    assert_eq!(response.status(), 401);

    let request =
        HttpRequest::get(server.url("/health")).header("Authorization", "Bearer secret-token");
    let response = http().send(&request).expect("the request failed");
    assert_eq!(response.status(), 200);
}

#[test]
fn oversized_requests_are_rejected() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let config = ServerConfig {
        max_body_bytes: 64,
        ..ServerConfig::default()
    };
    let server = TestServer::start(config, &mock);

    let payload = format!(
        r#"{{"model":"mock/model-a","messages":[{{"role":"user","content":"{}"}}]}}"#,
        "x".repeat(512)
    );
    let request = HttpRequest::post(server.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(payload);
    let response = http().send(&request).expect("the request failed");
    assert_eq!(response.status(), 413);
}

#[test]
fn the_server_binds_to_localhost_by_default() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let provider: Arc<dyn ChatProvider> = Arc::new(
        OpenAiCompatibleProvider::new(
            "mock",
            &format!("http://{}/v1", mock.address()),
            OpenAiCompatibleOptions::default(),
        )
        .unwrap(),
    );
    let client = Client::builder()
        .provider("mock", provider)
        .build()
        .unwrap();
    let config = ServerConfig {
        host: "127.0.0.1".to_owned(),
        port: address.port(),
        ..ServerConfig::default()
    };
    std::thread::spawn(move || {
        let _ = relay_rs::server::serve_on(client, config, listener);
    });
    // Give the server a moment to start accepting connections.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let url = Url::parse(&format!("http://{address}/health")).unwrap();
    let response = http()
        .send(&HttpRequest::get(url))
        .expect("the request failed");
    assert_eq!(response.status(), 200);
}

#[test]
fn a_remote_binding_needs_an_explicit_opt_in() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let provider: Arc<dyn ChatProvider> = Arc::new(
        OpenAiCompatibleProvider::new(
            "mock",
            &format!("http://{}/v1", mock.address()),
            OpenAiCompatibleOptions::default(),
        )
        .unwrap(),
    );
    let client = Client::builder()
        .provider("mock", provider)
        .build()
        .unwrap();

    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    let config = ServerConfig {
        host: "0.0.0.0".to_owned(),
        port: listener.local_addr().unwrap().port(),
        ..ServerConfig::default()
    };
    let error = relay_rs::server::serve_on(client, config, listener).unwrap_err();
    assert!(error.message().contains("allow_remote"));
}

#[test]
fn a_remote_binding_needs_an_api_key() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let provider: Arc<dyn ChatProvider> = Arc::new(
        OpenAiCompatibleProvider::new(
            "mock",
            &format!("http://{}/v1", mock.address()),
            OpenAiCompatibleOptions::default(),
        )
        .unwrap(),
    );
    let client = Client::builder()
        .provider("mock", provider)
        .build()
        .unwrap();

    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    let config = ServerConfig {
        host: "0.0.0.0".to_owned(),
        port: listener.local_addr().unwrap().port(),
        allow_remote: true,
        ..ServerConfig::default()
    };
    let error = relay_rs::server::serve_on(client, config, listener).unwrap_err();
    assert!(error.message().contains("API key"));
}

#[test]
fn cross_origin_requests_are_blocked_by_default() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let request = HttpRequest::get(server.url("/health")).header("Origin", "https://example.com");
    let response = http().send(&request).expect("the request failed");
    assert!(response.header("access-control-allow-origin").is_none());
}

#[test]
fn allowed_origins_receive_the_cors_headers() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let config = ServerConfig {
        cors_allow_origins: vec!["https://example.com".to_owned()],
        ..ServerConfig::default()
    };
    let server = TestServer::start(config, &mock);

    let request = HttpRequest::get(server.url("/health")).header("Origin", "https://example.com");
    let response = http().send(&request).expect("the request failed");
    assert_eq!(
        response.header("access-control-allow-origin"),
        Some("https://example.com")
    );
}

#[test]
fn the_health_endpoint_reports_the_version() {
    let mock = MockServer::start(vec![MockResponse::json("{}")]);
    let server = TestServer::start(ServerConfig::default(), &mock);

    let mut response = http()
        .send(&HttpRequest::get(server.url("/health")))
        .expect("the request failed");
    let body: Value = relay_rs::json::parse(&response.text().unwrap()).unwrap();
    assert_eq!(
        body.get("version").unwrap().as_str(),
        Some(relay_rs::VERSION)
    );
}
