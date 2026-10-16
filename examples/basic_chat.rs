//! A minimal chat request.
//!
//! Run it with a configuration that points at a provider:
//!
//! ```text
//! export RELAY_PROVIDER_OPENAI_API_KEY=sk-...
//! export RELAY_DEFAULT_MODEL=openai/gpt-4o-mini
//! cargo run --example basic_chat
//! ```
//!
//! Without a reachable provider the example prints a short explanation
//! instead of a stack trace.

use relay_rs::{ChatRequest, Client, Role, block_on};

fn main() {
    let client = match Client::from_config() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("relay: {error}");
            eprintln!("hint: run `relay init` and fill in a provider first");
            std::process::exit(1);
        }
    };

    let request = ChatRequest::new("openai/gpt-4o-mini")
        .system("You are a helpful assistant. Answer in one sentence.")
        .message(
            Role::User,
            "What is the difference between a process and a thread?",
        );

    let result = block_on(async move { client.complete(request).await });

    match result {
        Ok(response) => {
            println!("{}", response.text());
            eprintln!(
                "\nprovider: {} model: {} tokens: {}",
                response.provider(),
                response.model(),
                response
                    .usage()
                    .and_then(|usage| usage.total_tokens)
                    .map(|total| total.to_string())
                    .unwrap_or_else(|| "unknown".to_owned())
            );
        }
        Err(error) => {
            eprintln!("relay: the request failed: {error}");
            std::process::exit(1);
        }
    }
}
