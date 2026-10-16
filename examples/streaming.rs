//! A streaming chat request.
//!
//! The answer is printed while it is generated. The example works with any
//! provider that supports streaming, for example a local Ollama server:
//!
//! ```text
//! export RELAY_DEFAULT_MODEL=ollama/llama3.2
//! cargo run --example streaming
//! ```
//!
//! The stream is closed when it is dropped, so cancelling the program stops
//! the transfer.

use std::io::Write;

use relay_rs::{ChatRequest, Client, Role, StreamChunk, block_on};

fn main() {
    let client = match Client::from_config() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("relay: {error}");
            eprintln!("hint: run `relay init` and fill in a provider first");
            std::process::exit(1);
        }
    };

    let request = ChatRequest::new("ollama/llama3.2")
        .message(Role::User, "Describe the water cycle in three sentences.");

    let result = block_on(async move { client.stream(request).await });

    let mut stream = match result {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("relay: the stream could not be started: {error}");
            std::process::exit(1);
        }
    };

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let mut finished = false;

    for chunk in stream.by_ref() {
        match chunk {
            Ok(StreamChunk::Text(text)) => {
                // The text is written as it arrives; nothing is buffered.
                let _ = handle.write_all(text.as_bytes());
                let _ = handle.flush();
            }
            Ok(StreamChunk::Done {
                finish_reason,
                usage,
            }) => {
                finished = true;
                eprintln!(
                    "\nfinished: {} tokens: {}",
                    finish_reason
                        .map(|reason| reason.as_str().to_owned())
                        .unwrap_or_else(|| "unknown".to_owned()),
                    usage
                        .and_then(|usage| usage.total_tokens)
                        .map(|total| total.to_string())
                        .unwrap_or_else(|| "unknown".to_owned())
                );
            }
            Err(error) => {
                eprintln!("\nrelay: the stream failed: {error}");
                std::process::exit(1);
            }
        }
    }

    if !finished {
        eprintln!("\nrelay: the stream ended before the answer was complete");
        std::process::exit(1);
    }
}
