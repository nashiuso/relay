# Troubleshooting

Every error message names the provider and the failure. This page collects
the ones you will see most often.

## The configuration

### `no providers are configured`

Relay found a configuration, but it does not contain a single provider. Write
one with `relay init`, or point `--config` at the right file.

```bash
relay providers        # shows what Relay found
relay doctor           # shows the file it used
```

### `unknown configuration key ...`

The configuration file contains a key that Relay does not know. The key is
named in the message. Compare the file with
[`config.example.toml`](../config.example.toml) and with
[configuration.md](configuration.md).

### `provider `openai` is missing a `base_url``

Every provider needs a `base_url`. For an OpenAI compatible endpoint that is
usually `https://api.example.com/v1` — with the `/v1` at the end, because
Relay appends `/chat/completions` and `/models` to it.

### `provider `openai` is missing a `type`

Add `type = "openai-compatible"` or `type = "ollama"` to the provider.

### `the configuration file is not private`

The file holds a key and can be read by other users on the machine. Fix it:

```bash
chmod 600 ~/.config/relay/config.toml
```

Better: remove the `api_key` from the file and use `api_key_env` instead.

## The credentials

### `authentication failed` / `401`

* Is the key set? `echo $OPENAI_API_KEY` prints it — do not paste that into
  a bug report.
* Does the key belong to the endpoint? A key for one service does not work
  with another.
* Is the key expired or revoked? The provider decides, Relay only forwards.

### `the request was rejected` / `403`

The key is valid but not allowed to use the model. Check the model name and
the permissions of the key with the provider.

### The provider is reachable but never answers

* Raise the timeout: `timeout_secs = 300` in the configuration, or
  `RELAY_TIMEOUT_SECS=300`.
* A local model needs to be pulled first: `ollama pull llama3.2`.
* A cold model can take a while to load. The first request is usually the
  slow one.

## The model

### `unknown provider `ollama`; configured providers: openai`

The model names a provider that is not configured. The message lists the
providers that exist. Either configure the provider or use one of them.

### `the model `llama3.2` is not known by the provider`

The backend does not know the model. List the models it offers:

```bash
relay models ollama
relay models --refresh
```

Relay does not substitute a different model: an unknown model is an error.

### The answer comes from a different model than expected

The response carries the model that answered:

```rust
println!("{} answered with {}", response.provider(), response.model());
```

Relay reports the provider and the model that actually answered. It never
silently switches to another model, except with an enabled fallback list.

## The network

### `could not connect to ...`

* Is the service running? `curl http://127.0.0.1:11434/api/version` answers
  the same question.
* Is the port right? Ollama uses `11434` by default.
* Is a firewall in the way?

### `the TLS handshake failed`

Relay uses the `openssl` command line tool for TLS in this release.

* Is `openssl` installed and in the `PATH`? `openssl version` answers.
* Does the certificate validate? `openssl s_client -connect api.example.com:443
  -servername api.example.com` shows the details.
* For a local development server with a self signed certificate, set
  `insecure = true` on the provider. Never do that for a hosted endpoint.

### `the request timed out`

The deadline passed before the answer arrived. Raise `timeout_secs`, or check
whether the backend is slow on the first request.

## The command line interface

### `unknown command`

`relay <TAB>` lists the commands: `init`, `providers`, `models`, `chat`,
`doctor`, `serve`.

### `no prompt given`

`relay chat` needs `-p "your question"`, or `--interactive` for a
conversation.

### The output has escape codes in it

Relay uses colours when the output is a terminal. Use `--no-color` when the
output is piped, or set `NO_COLOR=1`. The colours are off automatically when
the output is not a terminal.

### The output is empty in a script

`--quiet` suppresses everything but the results and the errors. `--json`
prints a machine readable object instead of the human readable text.

## The server

### `refusing to bind to 0.0.0.0:8080`

The server refuses to listen on a remote interface without an explicit
opt-in. Set `allow_remote = true` **and** configure an API key, or bind to
`127.0.0.1`.

### `401` from the server

The server has an API key configured and the request did not send it:

```bash
-H 'Authorization: Bearer $RELAY_SERVER_KEY'
```

### `413` from the server

The request body is larger than `server.max_body_bytes`. Raise the limit or
send a shorter prompt.

### The browser refuses the answer

Cross origin requests are refused by default. Add the origin of your page to
`server.cors_allow_origins`.

### `address already in use`

Another process uses the port. Choose another one:

```bash
relay serve --port 8090
```

## Getting more information

* `relay doctor --json` prints the configuration, the default model and the
  state of every provider as JSON.
* `relay providers --json` and `relay models --json` print the same
  information for scripts.
* `-v` (verbose) adds the diagnostics of the request that failed.

## Reporting a bug

Open an issue at <https://github.com/nashiuso/relay-rs/issues> with the
version (`relay --version`), the command you ran and the complete error
message. Remove API keys, prompts and answers from the report.
