# Architecture

This document describes how Relay is put together, why it looks the way it
does, and where the seams are for the work that is planned.

## The shape of the crate

```
src/
  lib.rs            the public surface
  main.rs           the `relay` binary
  client.rs         the central client: model resolution, retries, fallback
  config.rs         configuration from files, environment and defaults
  error.rs          the typed error
  types.rs          messages, requests, responses, usage
  model.rs          model references and model information
  stream.rs         streaming answers
  secret.rs         the secret wrapper
  json.rs           a JSON reader and writer
  toml.rs           a parser for a subset of TOML
  runtime.rs        a tiny `block_on` for programs without an async runtime
  sync.rs           the asynchronous channel
  http/
    url.rs          a URL parser
    client.rs       the HTTP/1.1 client
    tls.rs          the TLS tunnel
  providers/
    traits.rs       the `ChatProvider` trait
    common.rs       helpers shared by the adapters
    openai_compatible.rs
    ollama.rs
  cli/
    mod.rs          the command dispatch and the help texts
    args.rs         the option parser
    output.rs       colours, quiet mode, JSON output
    commands.rs     the commands themselves
  server/
    mod.rs          the optional OpenAI compatible server
```

## The data flow

```
   CLI  ─┐
         ├─► Config ──► Client ──► ChatProvider ──► HTTP client ──► backend
 library ┘                │             │
                          │             └──► ChatStream ──► StreamChunk
                          │
                          └── model cache, retries, fallback
```

1. The configuration is read from the file, the environment and the command
   line, in that order.
2. The configuration is turned into providers, and the providers into a
   `Client`.
3. A request names a model as `provider/model`. The client resolves the
   provider, applies the retry policy and, if it is enabled, the fallback
   list.
4. The provider builds the wire request, sends it with the HTTP client and
   translates the answer into a `ChatResponse` or a `ChatStream`.

The client never changes the model of a request. The only place where a
different model is used is the fallback list, and only when it is enabled
explicitly.

## The dependency decision

This release of Relay has **no third party dependencies**. That was not the
original plan — the plan was Tokio, Reqwest, Axum, Serde and Clap — but the
crate registry was not reachable in the environment this release was built
in, so everything had to be written against the standard library.

### What that costs

| Area | Consequence |
| --- | --- |
| TLS | The `openssl` command line tool is used as a byte tunnel. It is not available everywhere and it is slower than a native stack. |
| HTTP | Only HTTP/1.1. No HTTP/2, no multiplexing, no compression. |
| TOML | A subset: no dates, no dotted keys, no multi line basic strings. |
| JSON | Complete grammar, but not a streaming parser and without key order. |
| Async | The asynchronous API is a facade over the blocking core; it does not use the reactor of the runtime you call it from. |
| CLI | A hand written parser: no completion, no man page, no shell integration. |

### The migration path

The modules were drawn so that the transport can be replaced without touching
the public API:

| Today | Planned | What changes |
| --- | --- | --- |
| `http::client` | `reqwest` with `rustls` | The provider adapters keep their shape; only the request and response types change. |
| `http::tls` | `rustls` inside Reqwest | The `openssl` dependency disappears. |
| `json` | `serde_json` | `Value` becomes `serde_json::Value`; the public types keep their own fields. |
| `toml` | `toml` | Same. |
| `server` | `axum` | The routes and the answer format stay the same. |
| `sync` | `tokio::sync` | The asynchronous facade becomes a real `async fn` that awaits the provider. |
| `cli::args` | `clap` | The commands and their help texts stay the same. |

The public types (`ChatRequest`, `ChatResponse`, `ChatStream`, `RelayError`,
`ModelRef`) do not have to change at all, and neither do the tests: they run
against the mock server in `src/testing.rs`, which stays.

## Errors

Every fallible operation returns a `RelayError` with an `ErrorKind`:

| Kind | Meaning | Retried |
| --- | --- | --- |
| `Config` | The configuration is wrong. | no |
| `Usage` | The command line was used incorrectly. | no |
| `Auth` | The credentials are missing or wrong. | no |
| `RateLimited` | The provider asked us to slow down. | yes |
| `Timeout` | The deadline passed. | yes |
| `Connection` | The connection failed. | yes |
| `Server` | The provider answered with an error. | yes (5xx) |
| `InvalidRequest` | The provider rejected the request. | no |
| `Decode` | The answer could not be parsed. | no |
| `Stream` | The stream ended unexpectedly. | no |
| `UnknownModel` | The provider does not know the model. | no |
| `UnknownProvider` | The model names a provider that is not configured. | no |
| `Cancelled` | The caller stopped the request. | no |
| `Unsupported` | The provider cannot do this. | no |
| `Io` | A local I/O failure. | no |

The error names the provider and the failure. It never contains the prompt,
the answer or a credential.

## Streaming

A stream is a channel of `StreamChunk` values:

```rust
pub enum StreamChunk {
    Text(String),
    Done { finish_reason: Option<FinishReason>, usage: Option<Usage> },
}
```

* The provider reads the wire format on a worker thread and publishes the
  fragments as they arrive. Nothing is buffered until the end.
* The consumer reads with the iterator interface or with `next_async()`.
* Dropping the stream sets the cancellation flag; the worker notices it and
  stops the transfer. The connection goes back to the pool or is closed.
* Two deadlines apply: an idle timeout between two fragments and the overall
  timeout of the provider. Both end the stream with a `Timeout` error.
* A stream that ends without a `Done` chunk is an interrupted stream. The
  CLI reports that as a failure, not as a complete answer.

## The HTTP client

* HTTP/1.1 over TCP, optionally through TLS.
* Keep-alive: connections are pooled per host and reused.
* Bodies may be `Content-Length` delimited, chunked or closed by the peer.
* Every request has a deadline; the body of a stream has an idle deadline.
* Redirects are not followed. A provider that redirects is a provider that
  needs a different base URL.
* `Content-Encoding` is not decoded: send `Accept-Encoding: identity` or
  configure a backend that does not compress.

## The server

The server is optional and lives behind the `server` feature. It reuses the
client, so a request that arrives at `POST /v1/chat/completions` takes the
same path as a request from the library. See [server.md](server.md).

## Testing

* The unit tests live next to the code they cover.
* The integration tests in `tests/` run against a mock HTTP server
  (`src/testing.rs`) that speaks HTTP/1.1 on a loopback socket. No test
  needs a network connection or an API key.
* The tests are deterministic: the mock server answers with the responses it
  was given, in order, and records the requests it received.
