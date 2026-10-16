//! The examples of the documentation, as tests.
//!
//! The documentation of the crate contains Rust examples. They are compiled
//! as doctests, but this file mirrors the important ones so that they are
//! checked by `cargo test` as well.

use std::sync::Arc;

use relay_rs::providers::traits::ChatProvider;
use relay_rs::providers::{OpenAiCompatibleOptions, OpenAiCompatibleProvider};
use relay_rs::testing::{MockResponse, MockServer};
use relay_rs::{ChatRequest, Client, ModelRef, Role, block_on};

fn client_for(server: &MockServer) -> Client {
    let provider: Arc<dyn ChatProvider> = Arc::new(
        OpenAiCompatibleProvider::new(
            "mock",
            &format!("http://{}/v1", server.address()),
            OpenAiCompatibleOptions::default(),
        )
        .expect("the provider could not be created"),
    );
    Client::builder()
        .provider("mock", provider)
        .default_model("mock/model-a")
        .build()
        .expect("the client could not be created")
}

#[test]
fn the_example_of_the_crate_documentation_works() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"choices":[{"message":{"content":"DNS resolves names."}}]}"#,
    )]);
    let client = client_for(&server);

    let answer = block_on(async move {
        let request =
            ChatRequest::new("mock/model-a").message(Role::User, "Explain how DNS works.");

        let response = client.complete(request).await?;
        Ok::<_, relay_rs::RelayError>(response.text().to_owned())
    })
    .expect("the request failed");

    assert_eq!(answer, "DNS resolves names.");
}

#[test]
fn the_example_of_the_request_documentation_works() {
    let request = ChatRequest::new("ollama/llama3.2")
        .message(Role::System, "You are terse.")
        .message(Role::User, "Explain DNS in one sentence.");
    assert_eq!(request.messages().len(), 2);
}

#[test]
fn the_example_of_the_runtime_documentation_works() {
    let value = block_on(async { 1 + 1 });
    assert_eq!(value, 2);
}

#[test]
fn a_model_reference_carries_the_provider() {
    let model = ModelRef::from("ollama/llama3.2");
    assert_eq!(model.provider(), "ollama");
    assert_eq!(model.model(), "llama3.2");
    assert_eq!(model.to_string(), "ollama/llama3.2");
}
