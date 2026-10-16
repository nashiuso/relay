//! Helpers shared by the provider adapters.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::{ErrorKind, RelayError};
use crate::http::client::Body;
use crate::json::Value;
use crate::stream::StreamChunk;
use crate::sync::AsyncSender;
use crate::types::Usage;

/// The interval used to check whether a stream was cancelled.
pub(crate) const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Parses the `usage` object of a provider response.
pub(crate) fn parse_usage(value: Option<&Value>) -> Option<Usage> {
    let usage = value?;
    let prompt = usage.get("prompt_tokens").and_then(|v| v.as_i64());
    let completion = usage.get("completion_tokens").and_then(|v| v.as_i64());
    let total = usage.get("total_tokens").and_then(|v| v.as_i64());
    if prompt.is_none() && completion.is_none() && total.is_none() {
        return None;
    }
    Some(Usage::new(
        prompt.map(|v| v as u32),
        completion.map(|v| v as u32),
        total.map(|v| v as u32),
    ))
}

/// Extracts the payload of a server sent event line.
///
/// Returns `None` for comments, empty lines and fields other than `data`.
pub(crate) fn sse_payload(line: &str) -> Option<&str> {
    let line = line.trim_end_matches('\r');
    if line.starts_with(':') {
        return None;
    }
    let (field, value) = line.split_once(':')?;
    if field != "data" {
        return None;
    }
    let value = value.strip_prefix(' ').unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    Some(value)
}

/// Truncates a message so that error output stays readable.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// What the line handler wants the reader to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Keep reading.
    Continue,
    /// Stop reading, the stream is complete.
    Stop,
}

/// Reads a body line by line and forwards the parsed content to the caller.
///
/// The reader runs on its own thread so that the provider can hand a stream
/// back to the caller immediately. Cancelling the stream (dropping it) stops
/// the reader.
pub(crate) fn read_lines(
    mut body: Body,
    overall_timeout: Duration,
    cancel: Arc<AtomicBool>,
    sender: &AsyncSender<Result<StreamChunk, RelayError>>,
    mut handler: impl FnMut(&str) -> Result<Flow, RelayError> + Send,
) {
    body.set_idle_timeout(CANCEL_POLL_INTERVAL);
    let deadline = Instant::now() + overall_timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match body.read_line() {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                match handler(&line) {
                    Ok(Flow::Continue) => {}
                    Ok(Flow::Stop) => break,
                    Err(err) => {
                        let _ = sender.send(Err(err));
                        break;
                    }
                }
            }
            Ok(None) => break,
            Err(err) if err.kind() == ErrorKind::Timeout => {
                if Instant::now() >= deadline {
                    let _ = sender.send(Err(RelayError::new(
                        ErrorKind::Timeout,
                        "the provider stopped sending data before the stream finished",
                    )));
                    break;
                }
                continue;
            }
            Err(err) => {
                let _ = sender.send(Err(err));
                break;
            }
        }
    }
}

/// A guard that signals cancellation when it is dropped.
pub(crate) struct CancelGuard(pub(crate) Arc<AtomicBool>);

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
