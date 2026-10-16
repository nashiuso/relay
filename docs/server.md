# The optional server

Relay can expose an OpenAI compatible HTTP server. It is behind the `server`
feature, so the default installation does not carry it.

```bash
cargo install --path . --features server
relay serve
```

```text
relay server listening on http://127.0.0.1:8080
```

## The endpoints

### `POST /v1/chat/completions`

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"ollama/llama3.2","messages":[{"role":"user","content":"Hello"}]}'
```

```json
{
  "id": "chatcmpl-8f2c1d0a",
  "object": "chat.completion",
  "created": 1760000000,
  "model": "ollama/llama3.2",
  "choices": [
    {
      "index": 0,
      "message": {"role": "assistant", "content": "Hello!"},
      "finish_reason": "stop"
    }
  ],
  "usage": {"prompt_tokens": 9, "completion_tokens": 2, "total_tokens": 11}
}
```

Supported fields of the request:

| Field | Meaning |
| --- | --- |
| `model` | Required. `provider/model`, or the plain model of the default provider. |
| `messages` | Required. The conversation, in order. |
| `stream` | `true` answers with Server-Sent Events. |
| `temperature` | Forwarded to the provider. |
| `max_tokens` | Forwarded to the provider. |
| `stop` | Forwarded to the provider. |

With `"stream": true` the answer is a stream of events:

```text
data: {"id":"chatcmpl-8f2c1d0a","object":"chat.completion.chunk","created":1760000000,"model":"ollama/llama3.2","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-8f2c1d0a",...,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

Each event is terminated by a blank line, and the stream ends with
`data: [DONE]`, which is what the OpenAI clients expect.

### `GET /v1/models`

```bash
curl http://127.0.0.1:8080/v1/models
```

```json
{
  "object": "list",
  "data": [
    {"id": "ollama/llama3.2", "object": "model", "owned_by": "ollama"}
  ]
}
```

The list contains the models of every configured provider. A provider that
cannot list its models is skipped instead of failing the whole request.

### `GET /health`

```json
{"status": "ok", "version": "0.1.0-alpha"}
```

The endpoint answers as long as the server runs. It does not call a provider.

## Authentication

The server is meant to be used locally. When it is exposed to other machines
it needs a key:

```toml
[server]
api_key_env = "RELAY_SERVER_KEY"
allow_remote = true
```

```bash
export RELAY_SERVER_KEY=...
relay serve
```

Clients then send:

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H 'Authorization: Bearer ...' \
  -H 'Content-Type: application/json' \
  -d '{"model":"ollama/llama3.2","messages":[{"role":"user","content":"Hello"}]}'
```

A request without the key is answered with `401` and a JSON error.

## Binding

The server binds to `127.0.0.1` by default. Binding to another interface
needs **both** `allow_remote = true` and an API key; without them the server
refuses to start and says why:

```text
refusing to bind to 0.0.0.0:8080: set `allow_remote = true` in the `[server]` section and configure an API key
```

`relay serve --host 0.0.0.0 --allow-remote` does the same from the command
line. The port comes from `--port` or from the configuration.

## Limits

| Limit | Default | Setting |
| --- | --- | --- |
| Request body | 1 MiB | `server.max_body_bytes` |
| Cross origin requests | refused | `server.cors_allow_origins` |

A request that is too large is answered with `413`. A request from an origin
that is not on the list is answered without the CORS headers, so a browser
refuses to hand the answer to the calling page.

## Logging

The server writes one line per request:

```text
POST /v1/chat/completions 200
```

The prompt, the answer and the API key are never written to the log.

## Using the server

With the OpenAI SDKs:

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="not-used")
client.chat.completions.create(
    model="ollama/llama3.2",
    messages=[{"role": "user", "content": "Hello"}],
)
```

With the `relay` binary:

```bash
relay chat -m ollama/llama3.2 -p "Hello"
```

The server is a thin layer: it translates the request, calls the same client
the library uses and translates the answer back. There is no queue, no
rate limiting, no accounting and no multi tenant logic.
