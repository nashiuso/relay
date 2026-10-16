# Contributing to Relay

Thanks for taking the time to contribute. Relay is a small crate with a big
surface area, so a few conventions keep it reviewable.

## Getting started

```bash
git clone https://github.com/nashiuso/relay-rs
cd relay-rs
cargo test --all-features
```

Relay builds without third party crates, so a plain stable toolchain is
enough. Rust 1.85 or newer is required (edition 2024).

## Before you open a pull request

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps --all-features
```

CI runs exactly these commands. Warnings are not suppressed to make the
pipeline green — a warning that cannot be fixed is worth a discussion in the
pull request.

## Rules of the project

* **No third party dependencies without a discussion.** The crate currently
  builds with the standard library only. A dependency has to earn its place:
  it needs to solve a problem that the standard library cannot, and it needs a
  maintenance story. Open an issue first.
* **No `unsafe`.** The crate is compiled with `#![forbid(unsafe_code)]`. Do
  not remove that.
* **No panics on user controlled paths.** Configuration files, server
  requests and provider answers are all untrusted input. Return a
  `RelayError` instead.
* **No secrets in output.** API keys must never appear in an error message,
  in a `Debug` implementation or in the machine readable output of the CLI.
  Use `SecretString`.
* **No telemetry and no hidden requests.** Relay talks to the provider that
  the user configured, and to nothing else.
* **No silent provider switching.** A request either goes to the model that
  was asked for, or it fails. Fallback has to be enabled explicitly.
* **Tests are hermetic.** Use the mock server in `relay_rs::testing` instead
  of the network. No test may need an API key.
* **Claims follow the implementation.** If a capability is not implemented, it
  is not advertised in the README, the help text or the documentation.

## Adding a provider

1. Implement `ChatProvider` in `src/providers/`.
2. Report the capability set truthfully in `Capabilities`.
3. Map the answers of the backend to `ChatResponse` and `StreamChunk`, and
   map its failures to `ErrorKind`.
4. Add an integration test against the mock server in
   `tests/provider_compatibility.rs`.
5. Document the provider in `docs/providers.md`.

## Commit messages and pull requests

Use short, imperative subjects (`Add the mistral provider`), keep one logical
change per pull request and describe how you tested it. A pull request that
changes the public API should update `CHANGELOG.md` and the documentation in
the same commit.

## Reporting bugs

Open an issue with the version of Relay, the command you ran, the output you
got and the output you expected. Please remove API keys and prompts from the
report.

## Code of conduct

By taking part in this project you agree to abide by the
[Code of Conduct](CODE_OF_CONDUCT.md).
