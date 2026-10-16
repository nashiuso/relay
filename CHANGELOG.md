# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha] - 2026-10-16

The first public alpha. The API may still change.

### Added

- Core types: `ChatRequest`, `Message`, `Role`, `ChatResponse`, `Usage`,
  `FinishReason`, `ModelRef` and `ModelInfo`.
- `Client` with explicit model resolution (`provider/model`), bounded retries
  for transient failures, opt-in fallback to configured alternatives, a
  per-provider model cache with an explicit refresh path, and a report of the
  provider and model that actually answered.
- Providers:
  - `openai_compatible` — chat completions, streaming and model listing for
    any OpenAI compatible HTTP endpoint.
  - `ollama` — `/api/chat`, `/api/tags` and `/api/version` for local models.
- Streaming with incremental delivery, cancellation on drop, idle and overall
  deadlines, and a clear distinction between a completed and a failed stream.
- Configuration from a TOML file, `RELAY_*` environment variables and command
  line options, with the precedence `command line > environment > file >
  defaults`.
- `relay` command line interface: `init`, `providers`, `models`, `chat`
  (one shot and `--interactive`), `doctor` and `serve`, with `--json`,
  `--quiet` and `--no-color`.
- Optional OpenAI compatible server behind the `server` feature:
  `POST /v1/chat/completions`, `GET /v1/models` and `GET /health`.
- `SecretString`, a wrapper that keeps API keys out of logs and error
  messages.
- A dependency free core: JSON, TOML subset, URL, HTTP/1.1 client and server,
  command line parser and asynchronous primitives are part of the crate.

### Security

- API keys are read from the environment by default and are never written to
  the debug output of a configuration or an error.
- The HTTP server binds to `127.0.0.1` and refuses to start on a remote
  interface without `allow_remote` and an API key.
- Request bodies are bounded, cross origin requests are rejected by default
  and prompts are never logged.

[Unreleased]: https://github.com/nashiuso/relay-rs/compare/v0.1.0-alpha...HEAD
[0.1.0-alpha]: https://github.com/nashiuso/relay-rs/releases/tag/v0.1.0-alpha
