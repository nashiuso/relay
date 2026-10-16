# Getting started

This guide takes you from a fresh checkout to a working answer from a model.
It takes about five minutes and needs no account to try: a local
[Ollama](https://ollama.com) server is enough.

## 1. Install Relay

```bash
git clone https://github.com/nashiuso/relay
cd relay
cargo install --path .
```

Add `--features server` if you also want the optional OpenAI compatible
server:

```bash
cargo install --path . --features server
```

Rust 1.85 or newer is required. Relay itself has no third party
dependencies, so the build does not download anything.

Check the installation:

```bash
relay --version
relay --help
```

## 2. Write a configuration file

```bash
relay init
```

The file is written to `~/.config/relay/config.toml` (or the platform
equivalent) with the permissions `0600`. It contains two providers: a local
Ollama server and an OpenAI compatible endpoint.

Open it and adjust the `default_model`:

```toml
default_model = "ollama/llama3.2"

[providers.ollama]
type = "ollama"
base_url = "http://127.0.0.1:11434"
model = "llama3.2"
```

## 3. Start a local model

```bash
ollama serve          # in one terminal
ollama pull llama3.2  # in another one
```

Skip this step if you want to use a hosted provider instead.

## 4. Check the setup

```bash
relay doctor
```

`doctor` reports the configuration file it used, the default model, and for
every provider whether it can be reached and whether it has credentials. It
exits with `1` when something is wrong, which makes it usable in a script.

## 5. Ask something

```bash
relay chat -m ollama/llama3.2 -p "What is a relay?"
```

The answer arrives while the model writes it. Add `--no-stream` to wait for the
complete answer instead.

For a conversation:

```bash
relay chat -m ollama/llama3.2 --interactive
```

The interactive mode keeps the history of the conversation and sends it with
every new question. It ends with `Ctrl-D` or `/exit`. When the input is
piped instead of typed, the mode reads the questions from the pipe without a
prompt, which makes it usable in a script.

## 6. Use a hosted provider

Keep the key in the environment, never in the file:

```bash
export OPENAI_API_KEY=sk-...
relay chat -m openai/gpt-4o-mini -p "Write a haiku about TCP"
```

The provider reads the key from the variable named in `api_key_env`. You can
also set it directly:

```bash
export RELAY_PROVIDER_OPENAI_API_KEY=sk-...
```

## 7. Look around

```bash
relay providers            # what is configured
relay models               # what the providers offer
relay models --json        # the same, machine readable
relay providers --json     # machine readable
```

`relay models` asks the provider once and caches the answer. Use
`relay models --refresh` to ask again.

## Using Relay as a library

```toml
[dependencies]
relay-rs = "0.1.0-alpha"
```

```rust
use relay_rs::{block_on, ChatRequest, Client, Role};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let answer = block_on(async {
        let client = Client::from_config()?;
        let request = ChatRequest::new("ollama/llama3.2")
            .message(Role::User, "What is a relay?");
        let response = client.complete(request).await?;
        Ok::<_, relay_rs::RelayError>(response.text().to_owned())
    })?;
    println!("{answer}");
    Ok(())
}
```

`client.complete(...)` is asynchronous and works with any runtime.
`client.complete_blocking(...)` is the same call for programs that do not use
one. `relay_rs::block_on` is a tiny executor for the second case.

## Next steps

* [Configuration](configuration.md) — every key and environment variable.
* [Providers](providers.md) — the shipped adapters and how to write your own.
* [Troubleshooting](troubleshooting.md) — when something does not work.
