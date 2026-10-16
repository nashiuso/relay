# Roadmap

This page says what Relay does today, what is planned and what will not be
built. The rule is simple: a capability is only listed as available when it
works.

## Now — 0.1.0-alpha

* One OpenAI compatible adapter, one Ollama adapter.
* A client with explicit model resolution, bounded retries, opt-in fallback
  and a model cache.
* Streaming with cancellation, deadlines and partial output.
* A configuration file, environment variables and a command line interface.
* An optional OpenAI compatible server behind the `server` feature.
* No third party dependencies, no telemetry, no unsafe code.

## Next

The order is a judgement call, not a promise.

1. **More providers.** The trait is the seam. An adapter for a provider is a
   few hundred lines plus tests against the mock server. Every new adapter
   has to bring its own documentation of the wire format.
2. **Real asynchronous functions.** The asynchronous API is a facade over the
   blocking core today. The plan is to make the providers `async` and to drop
   the worker thread, while keeping the blocking API for the command line
   interface.
3. **The dependency question.** The crate works without third party crates,
   but the transport, the TLS stack and the parsers would be better with
   `reqwest` and `rustls`, `serde` and `toml`. The migration is described in
   [architecture.md](architecture.md#the-migration-path). The public API does
   not have to change for it.
4. **Tool calling.** The message type has a tool role, but no adapter sends
   tool definitions and no client resolves tool calls. It needs a provider
   that supports it and a decision about the shape of the API.
5. **A `relay bench` command.** Measure the latency of the configured
   providers against a fixed set of prompts. Only useful if the numbers are
   reproducible and clearly labelled as a measurement of *your* setup.

## Later

* **Image, audio and embedding models.** The request type is text only.
  These need their own request and response types, not a flag on the chat
  request.
* **Web search.** A provider that searches and cites sources. It needs its
  own type because the answer is more than text, and because a search engine
  has different privacy rules than a chat model.
* **A richer configuration.** Named profiles, per model defaults, a
  configuration that can be merged from several files.

## Not planned

* **A hosted relay service.** Relay is a client. It does not proxy traffic
  for other people.
* **Unlimited or free inference.** The providers decide what they offer.
  Relay does not resell access and makes no claim about free tiers.
* **Workarounds for authentication walls, captchas or anti bot measures.**
* **Automatic provider switching.** A request goes to the model that was
  asked for, or it fails. The fallback list is opt-in.
* **A plugin system.** Adding a provider means adding a Rust file.
* **A GUI.** The command line interface and the library are the interfaces.

## Known limitations of the alpha

* The TOML parser supports a subset of TOML.
* The HTTP client speaks HTTP/1.1 only and does not decode compressed
  answers.
* TLS needs the `openssl` command line tool.
* The model cache lives in the process; it is not shared between the command
  line invocations.
* The server has no rate limiting and no accounting.
* The public API may still change: `0.1.0-alpha` means exactly that.
