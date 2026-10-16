//! Minimal asynchronous primitives.
//!
//! Relay exposes both a blocking and an asynchronous API. The asynchronous
//! side needs a channel that can be awaited from any executor, so the crate
//! ships a very small one built on `std::sync` primitives. It supports
//! waiting with a [`std::task::Waker`] and blocking on a condition variable at
//! the same time, which keeps the blocking and the streaming code paths
//! consistent.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

struct State<T> {
    queue: VecDeque<T>,
    closed: bool,
    waker: Option<Waker>,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    signal: Condvar,
}

impl<T> Shared<T> {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                closed: false,
                waker: None,
            }),
            signal: Condvar::new(),
        }
    }
}

/// The sending half of an asynchronous channel.
pub(crate) struct AsyncSender<T> {
    shared: Arc<Shared<T>>,
}

/// The receiving half of an asynchronous channel.
pub(crate) struct AsyncReceiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> AsyncSender<T> {
    /// Queues an item for the receiver.
    ///
    /// Returns the item back when the receiver is gone.
    pub(crate) fn send(&self, item: T) -> Result<(), T> {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.closed {
            return Err(item);
        }
        state.queue.push_back(item);
        let waker = state.waker.take();
        drop(state);
        self.shared.signal.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Closes the channel, waking the receiver.
    pub(crate) fn close(&self) {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.closed = true;
        let waker = state.waker.take();
        drop(state);
        self.shared.signal.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        self.close();
    }
}

impl<T> AsyncReceiver<T> {
    /// Waits for the next item.
    pub(crate) fn recv(&mut self) -> Recv<'_, T> {
        Recv { receiver: self }
    }

    /// Waits for the next item, blocking the current thread.
    ///
    /// Returns `None` once the channel is closed and drained.
    pub(crate) fn recv_blocking(&self) -> Option<T> {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        loop {
            if let Some(item) = state.queue.pop_front() {
                return Some(item);
            }
            if state.closed {
                return None;
            }
            state = match self.shared.signal.wait(state) {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }

    /// Closes the receiving side.
    pub(crate) fn close(&self) {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.closed = true;
        let waker = state.waker.take();
        drop(state);
        self.shared.signal.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_recv(&self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(item) = state.queue.pop_front() {
            return Poll::Ready(Some(item));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        self.close();
    }
}

/// The future returned by [`AsyncReceiver::recv`].
pub(crate) struct Recv<'a, T> {
    receiver: &'a AsyncReceiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.receiver.poll_recv(cx)
    }
}

/// Creates a connected sender and receiver.
pub(crate) fn channel<T>() -> (AsyncSender<T>, AsyncReceiver<T>) {
    let shared = Arc::new(Shared::new());
    (
        AsyncSender {
            shared: Arc::clone(&shared),
        },
        AsyncReceiver { shared },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn delivers_items_in_order() {
        let (sender, receiver) = channel::<i32>();
        sender.send(1).unwrap();
        sender.send(2).unwrap();
        assert_eq!(receiver.recv_blocking(), Some(1));
        assert_eq!(receiver.recv_blocking(), Some(2));
    }

    #[test]
    fn closing_ends_the_stream() {
        let (sender, receiver) = channel::<i32>();
        drop(sender);
        assert_eq!(receiver.recv_blocking(), None);
    }

    #[test]
    fn sending_to_a_closed_receiver_fails() {
        let (sender, receiver) = channel::<i32>();
        receiver.close();
        assert!(sender.send(7).is_err());
    }

    #[test]
    fn blocks_until_an_item_arrives() {
        let (sender, receiver) = channel::<&str>();
        let handle = thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(20));
            sender.send("late").unwrap();
        });
        assert_eq!(receiver.recv_blocking(), Some("late"));
        handle.join().unwrap();
    }
}
