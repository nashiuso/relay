# Configuration

Relay reads its configuration from four places. They are applied in this
order, and the later ones win:

1. the built-in defaults,
2. the configuration file,
3. the `RELAY_*` environment variables,
4. the command line options.

The configuration file is TOML. The default location is
`$XDG_CONFIG_HOME/relay/config.toml`, which is usually
`~/.config/relay/config.toml`. `RELAY_CONFIG` and `relay --config <path>`
point at a different file.

A complete, commented example is in [`config.example.toml`](../config.example.toml).

## The configuration file

### Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `default_model` | string | none | The model used when a request does not name one, as `provider/model`. |
| `timeout_secs` | integer | `120` | Request timeout for providers that do not set their own. |
| `max_retries` | integer | `2` | Retries for transient failures. Capped at `10`. |
| `retry_backoff_ms` | integer | `250` | The first wait between two retries. The wait grows with every attempt. Capped at `60000`. |

```toml
default_model = "ollama/llama3.2"
timeout_secs = 120
max_retries = 2
retry_backoff_ms = 250
```

### `[fallback]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | boolean | `false` | Whether a failed request may be retried on another model. |
| `models` | array of strings | empty | The alternatives, in order, as `provider/model`. |

```toml
[fallback]
enabled = true
models = ["openai/gpt-4o-mini", "ollama/llama3.2"]
```

Fallback is off by default and only ever happens for transient failures. A
request that fails with `401`, `403`, `404` or `400` is not repeated
somewhere else — that would hide a configuration mistake.

### `[providers.<name>]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `type` | string | required | `openai-compatible` or `ollama`. |
| `base_url` | string | required | The base URL of the API. |
| `model` | string | none | The default model of this provider. |
| `api_key` | string | none | The key, in the file. Prefer `api_key_env`. |
| `api_key_env` | string | none | The environment variable that holds the key. |
| `timeout_secs` | integer | `timeout_secs` | The timeout of this provider. |
| `headers` | table | empty | Extra headers for every request of this provider. |
| `insecure` | boolean | `false` | Skip certificate verification. Local development only. |

```toml
[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o-mini"
timeout_secs = 60

[providers.openai.headers]
"X-Tenant" = "team-a"
```

The name of the provider is the key of the table. It is the first part of
`provider/model`, so it may not contain a slash.

### `[server]`

Only used by `relay serve`, which needs the `server` feature.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `host` | string | `127.0.0.1` | The interface to bind to. |
| `port` | integer | `8080` | The port to listen on. Between `1` and `65535`. |
| `api_key` | string | none | The key clients must send. Prefer `api_key_env`. |
| `api_key_env` | string | none | The environment variable that holds the key. |
| `allow_remote` | boolean | `false` | Whether the server may listen on a remote interface. |
| `max_body_bytes` | integer | `1048576` | The largest accepted request body. |
| `cors_allow_origins` | array of strings | empty | The origins that may call the server from a browser. |

```toml
[server]
host = "127.0.0.1"
port = 8080
api_key_env = "RELAY_SERVER_KEY"
max_body_bytes = 1048576
cors_allow_origins = ["https://my-app.example.com"]
```

## Environment variables

### Global

| Variable | Meaning |
| --- | --- |
| `RELAY_CONFIG` | The configuration file to use. |
| `RELAY_DEFAULT_MODEL` | The default model, `provider/model`. |
| `RELAY_TIMEOUT_SECS` | The request timeout in seconds. |
| `RELAY_MAX_RETRIES` | The number of retries. |
| `RELAY_RETRY_BACKOFF_MS` | The first wait between two retries. |
| `RELAY_FALLBACK_ENABLED` | `true` or `false`. |
| `RELAY_FALLBACK_MODELS` | A comma separated list of alternatives. |
| `RELAY_SERVER_HOST` | The interface of the server. |
| `RELAY_SERVER_PORT` | The port of the server. |
| `RELAY_SERVER_API_KEY` | The key the server requires. |
| `RELAY_SERVER_ALLOW_REMOTE` | `true` or `false`. |
| `RELAY_SERVER_MAX_BODY_BYTES` | The largest accepted request body. |

### Per provider

For every provider `name` in the configuration:

| Variable | Meaning |
| --- | --- |
| `RELAY_PROVIDER_<NAME>_TYPE` | The type of the provider. |
| `RELAY_PROVIDER_<NAME>_BASE_URL` | The base URL. |
| `RELAY_PROVIDER_<NAME>_MODEL` | The default model. |
| `RELAY_PROVIDER_<NAME>_API_KEY` | The key itself. |
| `RELAY_PROVIDER_<NAME>_API_KEY_ENV` | The variable that holds the key. |
| `RELAY_PROVIDER_<NAME>_TIMEOUT_SECS` | The timeout of the provider. |

`<NAME>` is the upper case name of the provider with every character that is
not a letter or a digit replaced by an underscore. The provider `openai` is
`RELAY_PROVIDER_OPENAI_*`, the provider `my-gateway` is
`RELAY_PROVIDER_MY_GATEWAY_*`.

### Values that are read from the environment

The environment variables of the providers that Relay does not know about are
left alone. A key that is meant for the backend, such as `OPENAI_API_KEY`,
belongs to the backend and is named in `api_key_env`.

## Secrets

* Prefer `api_key_env` over `api_key`. A key in the file can end up in a
  backup, in a screenshot or in a bug report.
* `relay init` creates the file with the permissions `0600` and warns when an
  existing file is readable by others.
* Keys are wrapped in `SecretString`. Their `Debug` and `Display` print
  `<redacted>`, and that is also what the `--json` output and the error
  messages contain.
* Relay never writes a key to a log, and never sends it to a provider it was
  not configured for.

## The TOML subset

The parser accepts tables, strings, integers, floats, booleans, arrays and
inline tables. Comments start with `#`. Not supported are dates, dotted keys,
multi line basic strings and arrays of tables. The parser is strict: an
unknown key is an error, not a warning, so a typo does not silently change
the behaviour.

```toml
# a comment
key = "value"
number = 42
ratio = 0.5
flag = true
list = ["a", "b"]
inline = { key = "value" }

[table]
nested = "value"

[table.sub]
key = "value"
```

## Command line options

The global options work with every command and may appear before or after the
command name:

| Option | Meaning |
| --- | --- |
| `-c`, `--config <PATH>` | Use a specific configuration file. |
| `-v`, `--verbose` | Print more diagnostics. |
| `-q`, `--quiet` | Print only results and errors. |
| `--no-color` | Never use colours. |
| `--json` | Print machine readable JSON. |
| `-h`, `--help` | Show the help. |
| `-V`, `--version` | Show the version. |

Colours are disabled automatically when the output is not a terminal or when
`NO_COLOR` is set.

## Precedence in practice

```bash
# the file says ollama, the environment says openai
RELAY_DEFAULT_MODEL=openai/gpt-4o-mini relay chat -p "hi"

# the command line wins over both
RELAY_DEFAULT_MODEL=openai/gpt-4o-mini relay chat -m ollama/llama3.2 -p "hi"
```

The second command talks to Ollama: the model on the command line replaces
the model from the environment. The provider of the model decides where the
request goes, so the environment cannot silently redirect it.
