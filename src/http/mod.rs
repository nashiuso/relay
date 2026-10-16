//! Internal HTTP transport used by the provider adapters.
//!
//! The transport is intentionally private to the crate: the public API of
//! Relay speaks in terms of providers, requests and responses.

pub mod client;
pub mod tls;
pub mod url;

pub use client::{Body, Headers, HttpClient, HttpClientBuilder, HttpRequest, HttpResponse};
pub use tls::TlsConfig;
pub use url::{Scheme, Url, UrlError};
