# Security policy

## Supported versions

Relay is in alpha. Security fixes land on the `main` branch and in the next
release; there is no long term support branch yet.

| Version | Supported |
| --- | --- |
| 0.1.0-alpha | yes |

## Reporting a vulnerability

Please **do not** open a public issue for a security problem.

Report it privately through GitHub's security advisory form:
<https://github.com/nashiuso/relay/security/advisories/new>. Include the
version, a description of the problem and, if possible, a reproduction. You
can expect an acknowledgement within a few days.

If the advisory form is not available to you, open an issue that only says
that you want to report a vulnerability privately and ask for a contact
address — without the details.

## What Relay does and does not do

Relay is a client for AI providers. It is designed so that a mistake in the
configuration does not leak your secrets or expose your machine.

### Secrets

* API keys are read from the environment (`api_key_env`) by default. Storing a
  key in the configuration file is possible, but the file is then a secret
  and Relay warns about it.
* Keys are wrapped in `SecretString`. Its `Debug` and `Display` print
  `<redacted>`, and the redacted form is what appears in configuration dumps,
  error messages and the `--json` output of the command line interface.
* `relay init` creates the configuration file with the permissions `0600` on
  Unix and warns when an existing file is readable by others.
* Relay never writes a key to a log, to a crash message or to a request body
  of its own server.

### Network

* Relay talks to the endpoints that are configured, and to nothing else. There
  is no telemetry, no update check and no phone home.
* Hosted endpoints should use `https`. `http://` is accepted for local
  development servers, and the provider configuration makes the difference
  explicit.
* TLS is provided by the `openssl` command line tool in this release. It is
  the system trust store that decides whether a certificate is valid.

### The optional server

* The server binds to `127.0.0.1`. Binding to another interface requires
  `allow_remote = true` **and** an API key; without both, the server refuses
  to start.
* When an API key is configured, clients must send
  `Authorization: Bearer <key>`.
* Request bodies are bounded (`max_body_bytes`, one megabyte by default) and
  oversized requests are rejected with `413`.
* Cross origin requests are rejected by default. Only the origins in
  `cors_allow_origins` receive the CORS headers.
* Prompts, answers and API keys are never logged. The server writes one line
  per request with the method, the path and the status.

### Error handling

* Errors name the provider and the failure, but never the prompt, the answer
  or the credentials.
* The crate is compiled with `#![forbid(unsafe_code)]` and does not panic on
  user controlled input.

## Known limitations of this alpha

* The TOML parser accepts a subset of TOML. A configuration file is never
  executed, but it is parsed by code that is not as battle tested as a mature
  parser.
* The `insecure` option disables certificate verification. It exists for
  local development and must not be used against a hosted endpoint.
* The `openssl` tunnel inherits the environment of the Relay process. A
  hostile `PATH` could point at a different `openssl`; run Relay with a
  trusted environment.
