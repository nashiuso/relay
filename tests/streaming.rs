//! Tests for the streaming behaviour of the providers.

use std::time::Duration;

use relay_rs::error::ErrorKind;
use relay_rs::providers::traits::ChatProvider;
use relay_rs::providers::{
    OllamaOptions, OllamaProvider, OpenAiCompatibleOptions, OpenAiCompatibleProvider,
};
use relay_rs::stream::StreamChunk;
use relay_rs::testing::{MockResponse, MockServer};
use relay_rs::types::ChatRequest;

fn openai(server: &MockServer) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        "openai",
        &format!("http://{}/v1", server.address()),
        OpenAiCompatibleOptions::default(),
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
fn openai_compatible_streams_the_answer_in_chunks() {
    let server = MockServer::start(vec![MockResponse::streaming(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n".to_owned(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n".to_owned(),
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n".to_owned(),
        "data: [DONE]\n\n".to_owned(),
    ])]);
    let provider = openai(&server);

    let mut stream = provider
        .stream(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the stream could not be started");

    let mut text = String::new();
    let mut finished = false;
    for chunk in stream.by_ref() {
        match chunk.expect("the stream failed") {
            StreamChunk::Text(part) => text.push_str(&part),
            StreamChunk::Done { finish_reason, .. } => {
                assert_eq!(
                    finish_reason.map(|reason| reason.as_str().to_owned()),
                    Some("stop".to_owned())
                );
                finished = true;
                break;
            }
        }
    }

    assert_eq!(text, "Hello");
    assert!(finished, "the stream did not report that it finished");

    let body = server.last_request().unwrap().body_json().unwrap();
    assert_eq!(body.get("stream").unwrap().as_bool(), Some(true));
}

#[test]
fn openai_compatible_reports_errors_inside_the_stream() {
    let server = MockServer::start(vec![MockResponse::streaming(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n".to_owned(),
        "data: {\"error\":{\"message\":\"upstream exploded\"}}\n\n".to_owned(),
    ])]);
    let provider = openai(&server);

    let mut stream = provider
        .stream(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the stream could not be started");

    let mut text = String::new();
    let mut error = None;
    for chunk in stream.by_ref() {
        match chunk {
            Ok(StreamChunk::Text(part)) => text.push_str(&part),
            Ok(StreamChunk::Done { .. }) => {}
            Err(err) => {
                error = Some(err);
                break;
            }
        }
    }

    assert_eq!(text, "partial");
    let error = error.expect("the stream did not report the error");
    assert!(error.message().contains("upstream exploded"));
}

#[test]
fn ollama_streams_newline_delimited_json() {
    let server = MockServer::start(vec![MockResponse::streaming(vec![
        "{\"message\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"done\":false}\n".to_owned(),
        "{\"message\":{\"role\":\"assistant\",\"content\":\"lo\"},\"done\":false}\n".to_owned(),
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":4,\"eval_count\":2}\n"
            .to_owned(),
    ])]);
    let provider = ollama(&server);

    let mut stream = provider
        .stream(&ChatRequest::new("ollama/llama3.2").user("hello"))
        .expect("the stream could not be started");

    let mut text = String::new();
    let mut usage = None;
    for chunk in stream.by_ref() {
        match chunk.expect("the stream failed") {
            StreamChunk::Text(part) => text.push_str(&part),
            StreamChunk::Done {
                finish_reason,
                usage: reported,
            } => {
                assert_eq!(
                    finish_reason.map(|reason| reason.as_str().to_owned()),
                    Some("stop".to_owned())
                );
                usage = reported;
                break;
            }
        }
    }

    assert_eq!(text, "Hello");
    let usage = usage.expect("no usage reported");
    assert_eq!(usage.prompt_tokens, Some(4));
    assert_eq!(usage.completion_tokens, Some(2));
}

#[test]
fn a_stream_can_be_collected_into_text() {
    let server = MockServer::start(vec![MockResponse::streaming(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"one \"}}]}\n\n".to_owned(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"two\"}}]}\n\n".to_owned(),
        "data: [DONE]\n\n".to_owned(),
    ])]);
    let provider = openai(&server);
    let mut stream = provider
        .stream(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the stream could not be started");
    assert_eq!(stream.collect_text().unwrap(), "one two");
}

#[test]
fn dropping_a_stream_stops_the_transfer() {
    let server = MockServer::start(vec![MockResponse::streaming(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"one\"}}]}\n\n".to_owned(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"two\"}}]}\n\n".to_owned(),
    ])]);
    let provider = openai(&server);
    let mut stream = provider
        .stream(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the stream could not be started");

    let first = stream.next().expect("no chunk").expect("the stream failed");
    assert_eq!(first.text(), Some("one"));
    drop(stream);
    // The connection is closed with the stream; the server sees the client go
    // away and the test finishes without hanging.
}

#[test]
fn a_stream_that_never_ends_times_out() {
    // The server announces more data than it sends and then stops writing.
    let server = MockServer::start(vec![MockResponse::partial(
        "data: {\"choices\":[{\"delta\":{\"content\":\"one\"}}]}\n\n",
        4096,
    )]);
    let provider = OpenAiCompatibleProvider::new(
        "openai",
        &format!("http://{}/v1", server.address()),
        OpenAiCompatibleOptions {
            timeout: Some(Duration::from_millis(600)),
            ..OpenAiCompatibleOptions::default()
        },
    )
    .expect("the provider could not be created");

    let mut stream = provider
        .stream(&ChatRequest::new("openai/gpt-4o-mini").user("hello"))
        .expect("the stream could not be started");

    let mut error = None;
    for chunk in stream.by_ref() {
        if let Err(err) = chunk {
            error = Some(err);
            break;
        }
    }
    let error = error.expect("the stream did not time out");
    assert_eq!(error.kind(), ErrorKind::Timeout);
}
