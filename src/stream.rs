//! Streaming support.
//!
//! A [`ChatStream`] yields the answer of a model piece by piece. The stream
//! can be consumed synchronously (it implements [`Iterator`]) or from an
//! asynchronous context with [`ChatStream::next_async`]. Dropping the stream
//! cancels the transfer: the underlying connection is closed and the
//! provider stops producing data.

use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{ErrorKind, RelayError};
use crate::sync::{AsyncReceiver, AsyncSender};
use crate::types::{FinishReason, Usage};

/// The flag that is set when the consumer drops a stream.
struct CancelFlag(Arc<AtomicBool>);

impl Drop for CancelFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The producing half of a [`ChatStream`].
///
/// A provider publishes the chunks of an answer with [`StreamSender::send`].
/// When the consumer drops the stream, the sender reports the cancellation
/// through [`StreamSender::is_cancelled`] and stops the transfer.
pub struct StreamSender {
    sender: AsyncSender<Result<StreamChunk, RelayError>>,
    cancelled: Arc<AtomicBool>,
}

impl StreamSender {
    /// Publishes a chunk of the answer.
    ///
    /// Returns an error when the consumer has gone away.
    pub fn send(&mut self, chunk: StreamChunk) -> Result<(), RelayError> {
        self.sender.send(Ok(chunk)).map_err(|_| {
            RelayError::new(
                ErrorKind::Cancelled,
                "the stream was closed by the consumer",
            )
        })
    }

    /// Publishes an error and ends the stream.
    pub fn fail(&mut self, error: RelayError) -> Result<(), RelayError> {
        self.sender.send(Err(error)).map_err(|_| {
            RelayError::new(
                ErrorKind::Cancelled,
                "the stream was closed by the consumer",
            )
        })
    }

    /// Returns `true` when the consumer dropped the stream.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Closes the stream.
    pub fn close(self) {}
}

impl fmt::Debug for StreamSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamSender")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// A single piece of a streamed answer.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamChunk {
    /// A fragment of the answer text.
    Text(String),
    /// The final chunk, carrying the reason the model stopped and the token
    /// usage when the provider reports them.
    Done {
        /// Why the model stopped generating.
        finish_reason: Option<FinishReason>,
        /// The token usage reported by the provider.
        usage: Option<Usage>,
    },
}

impl StreamChunk {
    /// The text of the chunk, if it carries text.
    pub fn text(&self) -> Option<&str> {
        match self {
            StreamChunk::Text(text) => Some(text),
            StreamChunk::Done { .. } => None,
        }
    }

    /// Returns `true` for the final chunk of a stream.
    pub fn is_done(&self) -> bool {
        matches!(self, StreamChunk::Done { .. })
    }
}

/// A stream of answer fragments.
pub struct ChatStream {
    receiver: AsyncReceiver<Result<StreamChunk, RelayError>>,
    guard: Option<Box<dyn Any + Send>>,
    finished: bool,
}

impl ChatStream {
    pub(crate) fn new(
        receiver: AsyncReceiver<Result<StreamChunk, RelayError>>,
        guard: Option<Box<dyn Any + Send>>,
    ) -> Self {
        Self {
            receiver,
            guard,
            finished: false,
        }
    }

    /// Creates a stream together with the sender that feeds it.
    ///
    /// This is the entry point for custom providers: move the returned
    /// [`StreamSender`] into a worker thread and publish the chunks of the
    /// answer. Dropping the stream sets the cancellation flag, which the
    /// sender reports through [`StreamSender::is_cancelled`].
    pub fn channel() -> (StreamSender, ChatStream) {
        let (sender, receiver) = crate::sync::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = Box::new(CancelFlag(Arc::clone(&cancelled)));
        (
            StreamSender { sender, cancelled },
            ChatStream {
                receiver,
                guard: Some(guard),
                finished: false,
            },
        )
    }

    /// Returns the next chunk, waiting for it if necessary.
    pub async fn next_async(&mut self) -> Option<Result<StreamChunk, RelayError>> {
        let chunk = self.receiver.recv().await;
        if chunk.is_none() {
            self.finished = true;
        }
        chunk
    }

    /// Collects the whole answer into a string.
    pub async fn collect_text_async(&mut self) -> Result<String, RelayError> {
        let mut text = String::new();
        while let Some(chunk) = self.next_async().await {
            match chunk? {
                StreamChunk::Text(part) => text.push_str(&part),
                StreamChunk::Done { .. } => break,
            }
        }
        Ok(text)
    }

    /// Collects the whole answer into a string, blocking the current thread.
    pub fn collect_text(&mut self) -> Result<String, RelayError> {
        let mut text = String::new();
        for chunk in self.by_ref() {
            match chunk? {
                StreamChunk::Text(part) => text.push_str(&part),
                StreamChunk::Done { .. } => break,
            }
        }
        Ok(text)
    }

    /// Returns `true` when the stream has been fully consumed.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Cancels the stream and releases the underlying connection.
    pub fn cancel(&mut self) {
        self.finished = true;
        self.receiver.close();
        self.guard = None;
    }
}

impl Iterator for ChatStream {
    type Item = Result<StreamChunk, RelayError>;

    fn next(&mut self) -> Option<Self::Item> {
        let chunk = self.receiver.recv_blocking();
        if chunk.is_none() {
            self.finished = true;
        }
        chunk
    }
}

impl fmt::Debug for ChatStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatStream")
            .field("finished", &self.finished)
            .finish()
    }
}
