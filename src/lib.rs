//! # Relay
//!
//! One interface. Your models.
//!
//! Relay is a small toolkit for talking to AI providers from Rust. It hides
//! the differences between the backends behind one small API: build a
//! request, send it, read the answer.
//!
//! ```no_run
//! use relay_rs::{block_on, ChatRequest, Client, Role};
//!
//! # fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let answer = block_on(async {
//!     let client = Client::from_config()?;
//!
//!     let request = ChatRequest::new("provider/model")
//!         .message(Role::User, "Explain how DNS works.");
//!
//!     let response = client.complete(request).await?;
//!     Ok::<_, relay_rs::RelayError>(response.text().to_owned())
//! })?;
//!
//! println!("{answer}");
//! # Ok(())
//! # }
//! ```
//!
//! ## What is in the box
//!
//! * [`Client`] – the central entry point: model resolution, bounded retries,
//!   opt-in fallback and cached model listings.
//! * [`providers`] – the provider adapters. Two ship with Relay: an
//!   OpenAI compatible client and an Ollama client for local models.
//! * [`config`] – configuration from a TOML file, `RELAY_*` environment
//!   variables and command line options, in that order of precedence.
//! * [`stream`] – incremental answers that can be consumed synchronously or
//!   asynchronously.
//!
//! ## Design
//!
//! Relay keeps its core free of third party dependencies so that it can be
//! built and audited in isolation. The HTTP transport, the JSON and TOML
//! readers and the command line parser live in the crate and are documented
//! where they matter.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

pub mod cli;
pub mod client;
pub mod config;
pub mod error;
pub mod json;
pub mod model;
pub mod providers;
pub mod runtime;
pub mod secret;
pub mod stream;
pub mod toml;
pub mod types;

#[cfg(feature = "server")]
pub mod server;

#[doc(hidden)]
pub mod http;

#[doc(hidden)]
pub mod testing;

mod sync;

pub use client::{Client, ClientBuilder, ProviderSummary};
pub use config::{Config, ProviderConfig, ProviderType, ServerConfig};
pub use error::{ErrorKind, RelayError, Result};
pub use model::{ModelInfo, ModelRef};
pub use providers::traits::{Capabilities, ChatProvider, ProviderKind};
pub use runtime::block_on;
pub use secret::SecretString;
pub use stream::{ChatStream, StreamChunk, StreamSender};
pub use types::{ChatRequest, ChatResponse, FinishReason, Message, Role, Usage};

/// The version of the Relay crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
