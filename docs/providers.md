# Providers

A provider is the adapter between Relay and a backend that answers chat
requests. Two ship with Relay; more can be added without touching the client,
the command line interface or the server.

## The `ChatProvider` trait

```rust
pub trait ChatProvider: Send + Sync + fmt::Debug {
    fn name(&self) -> &str;
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> Capabilities;
    fn default_model(&self) -> Option<&str> { None }
    fn has_credentials(&self) -> bool { false }
    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError>;
    fn stream(&self, request: &ChatRequest) -> Result<ChatStream, RelayError>;
    fn list_models(&self) -> Result<Vec<ModelInfo>, RelayError>;
    fn describe(&self) -> String { /* ... */ }
}
```

* `complete` answers with the whole text at once.
* `stream` returns a [`ChatStream`](https://docs.rs/relay-rs) that yields the
  answer piece by piece.
* `list_models` may return an error when the backend has no catalogue; the
  client turns that into a readable message.
* `capabilities` describes what the backend can do. Relay uses it to decide
  what it may offer, and it never claims more than the backend can do.

The request that reaches a provider carries the model, the messages and the
options that the caller set. The provider translates them into the wire
format of its backend.

## OpenAI compatible providers

```toml
[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o-mini"
```

The adapter talks to the endpoints that every OpenAI compatible service
implements:

| Operation | Request |
| --- | --- |
| Chat | `POST {base_url}/chat/completions` |
| Models | `GET {base_url}/models` |
| Stream | `POST {base_url}/chat/completions` with `"stream": true` |

The body of a chat request is the OpenAI format:

```json
{
  "model": "gpt-4o-mini",
  "messages": [{"role": "user", "content": "Hello"}],
  "temperature": 0.7,
  "max_tokens": 512,
  "stream": false
}
```

* The key is sent as `Authorization: Bearer <key>`. Without a key the header
  is left out entirely, which is what local gateways expect.
* The answer is read from `choices[0].message.content`, the reason from
  `choices[0].finish_reason` and the usage from `usage`.
* A stream is read as Server-Sent Events: every `data:` line carries one
  `choices[0].delta.content` fragment, and `data: [DONE]` ends the stream.
* The answer may be a plain JSON object or an error object; both are mapped
  to a typed error.

Status codes are mapped like this:

| Status | Error |
| --- | --- |
| `400`, `404`, `405`, `409`, `413`, `422` | `ErrorKind::InvalidRequest` |
| `401`, `403` | `ErrorKind::Auth` |
| `408`, `504` | `ErrorKind::Timeout` |
| `429` | `ErrorKind::RateLimited` |
| `5xx` | `ErrorKind::Server` |
| anything else | `ErrorKind::InvalidRequest` |

Only `429`, `5xx`, timeouts and connection errors are retried. A `401` is
permanent: repeating it would only lock the account.

## Ollama

```toml
[providers.ollama]
type = "ollama"
base_url = "http://127.0.0.1:11434"
model = "llama3.2"
```

The adapter uses the native Ollama API:

| Operation | Request |
| --- | --- |
| Chat | `POST {base_url}/api/chat` |
| Models | `GET {base_url}/api/tags` |
| Version | `GET {base_url}/api/version` |
| Stream | `POST {base_url}/api/chat` with `"stream": true` |

* The body uses the Ollama format: `model`, `messages`, `stream` and the
  options `temperature`, `top_p`, `num_predict`, `seed` and `stop`.
* A stream is newline delimited JSON. Every line carries
  `message.content`, and the last line has `"done": true` together with
  `prompt_eval_count` and `eval_count`, which become the usage.
* Ollama runs on the machine, so no key is needed. The adapter never sends
  an `Authorization` header.

## Selecting a provider

A model is written as `provider/model`:

```rust
let request = ChatRequest::new("ollama/llama3.2").user("Hello!");
```

The part before the slash names the provider, the part after it is the model
that the provider receives. If the request names no model, the
`default_model` of the configuration is used.

Relay never substitutes a model on its own. If a provider does not know the
model, the answer is an error, not a different model. The only exception is
the fallback list, which has to be enabled explicitly.

## Writing your own provider

Implement the trait, then register the provider with the client:

```rust
use std::sync::Arc;
use relay_rs::{ChatProvider, Client};

let provider: Arc<dyn ChatProvider> = Arc::new(MyProvider::new()?);
let client = Client::builder()
    .provider("mine", provider)
    .default_model("mine/my-model")
    .build()?;
```

A complete example, including streaming, is in
[`examples/custom_provider.rs`](../examples/custom_provider.rs). It shows the
three things every provider needs:

* build the request body,
* parse the answer into a `ChatResponse`,
* turn the fragments of a stream into `StreamChunk::Text` and a final
  `StreamChunk::Done`.

For a streaming provider, create the stream with `ChatStream::channel()`,
move the returned sender into a worker thread and publish the fragments. The
sender reports the cancellation when the consumer drops the stream, so the
worker can stop early.

When the provider is good enough, add it to `src/providers/`, give it a
section in this document and add tests against the mock server in
`tests/provider_compatibility.rs`.

## Providers Relay does not ship

There is no adapter for any hosted service other than the OpenAI compatible
one, and none for image, audio or embedding models. The interfaces for those
exist in the type system — a provider can report that it does not support
streaming or model listing — but no implementation is included. See
[roadmap.md](roadmap.md).

Relay does not include workarounds for authentication walls, captchas or
rate limit pages, and it does not bundle access to any third party service.
