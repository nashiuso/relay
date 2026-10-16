# Security

This page describes what Relay does with your secrets, what it sends over the
network and how the optional server is protected. The short version: Relay
talks to the providers you configure, to nothing else, and it keeps your keys
out of its output.

## Secrets

API keys are wrapped in `SecretString`:

```rust
pub struct SecretString(String);
```

Its `Debug` and `Display` print `<redacted>`. That is what ends up in error
messages, in the `Debug` output of a configuration and in the `--json` output
of the command line interface. A key is never part of a request body that
Relay builds itself, and it is never written to a log.

The recommended way to configure a key is the environment:

```toml
[providers.openai]
api_key_env = "OPENAI_API_KEY"
```

The alternative, `api_key = "..."` in the file, works but makes the file a
secret. Relay warns about it and creates new files with the permissions
`0600`.

## The network

* Relay opens connections to the `base_url` of the providers you configured
  and to nothing else. There is no telemetry, no update check and no crash
  report.
* A hosted endpoint should use `https`. The configuration makes the scheme
  explicit, and there is no fallback from `https` to `http`.
* The server of the optional feature binds to the loopback interface by
  default and refuses to bind anywhere else without an explicit opt-in and
  an API key.

## TLS

In this release TLS is provided by the `openssl` command line tool, which
Relay drives as a subprocess. The system trust store decides whether a
certificate is valid, exactly as it does for a browser.

Consequences to be aware of:

* `openssl` has to be installed and in the `PATH` of the Relay process.
* A hostile `PATH` could point at a different `openssl`. Run Relay with an
  environment you trust.
* The `insecure` option disables the certificate check. It exists for local
  development servers with a self signed certificate and must not be used
  against a hosted endpoint.

The planned replacement is `rustls` inside the HTTP client, which removes
the subprocess and the dependency on the system tool.

## The optional server

| Protection | Default | Setting |
| --- | --- | --- |
| Binds to the loopback interface | `127.0.0.1` | `server.host` |
| Requires an API key | no key needed locally | `server.api_key_env` |
| Remote binding needs an opt-in and a key | refused | `server.allow_remote` |
| Bounds the request body | 1 MiB | `server.max_body_bytes` |
| Restricts cross origin requests | refused | `server.cors_allow_origins` |

* A request without the key is answered with `401`.
* A request that is too large is answered with `413`.
* A request to an unknown route is answered with `404`.
* The log contains the method, the path and the status — never the prompt,
  the answer or a key.

The server has no rate limiting, no accounting and no multi tenant logic. It
is a translation layer in front of your own providers; do not expose it to
the internet without putting a reverse proxy with a rate limit in front of
it.

## Input handling

* Configuration files, command line arguments and provider answers are
  untrusted input. They are parsed into typed values, and a problem becomes
  a `RelayError` instead of a panic.
* The crate is compiled with `#![forbid(unsafe_code)]`.
* The JSON and TOML parsers do not evaluate anything: there is no include,
  no substitution and no interpolation.
* The HTTP client does not follow redirects. A redirect is a configuration
  mistake, not something to follow silently.

## What Relay does not do

* It does not log prompts or answers.
* It does not send telemetry.
* It does not call a service that you did not configure.
* It does not work around authentication walls, captchas or rate limit pages.
* It does not switch to another provider without being told to.
* It does not contain workarounds for any particular service, and it is not
  a bundle of copied code.

## Reporting a vulnerability

See [SECURITY.md](../SECURITY.md) in the repository root.
