//! A minimal executor for the asynchronous facade.
//!
//! Relay does not depend on an async runtime, but its asynchronous methods
//! return ordinary futures. This module provides a tiny [`block_on`] that
//! drives such a future to completion on the current thread, which is enough
//! for examples, scripts and small tools.
//!
//! Applications that already use Tokio, async-std or smol can simply await
//! the futures of the crate inside their own runtime; Relay never requires
//! one.

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};

/// The waker that wakes the thread blocked in [`block_on`].
struct ThreadWaker {
    notified: Mutex<bool>,
    condition: Condvar,
}

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let mut notified = match self.notified.lock() {
            Ok(notified) => notified,
            // A poisoned lock only means that another thread panicked while
            // holding it. The notification is still worth sending.
            Err(poisoned) => poisoned.into_inner(),
        };
        *notified = true;
        self.condition.notify_one();
    }
}

/// Runs a future to completion on the current thread.
///
/// The future is polled first, and then again every time it wakes the
/// executor. Panics inside the future are propagated to the caller.
///
/// ```no_run
/// use relay_rs::block_on;
/// use relay_rs::{ChatRequest, Client};
///
/// let answer = block_on(async {
///     let client = Client::from_config()?;
///     let response = client.complete(ChatRequest::new("provider/model").user("Hi")).await?;
///     Ok::<_, relay_rs::RelayError>(response.text().to_owned())
/// });
/// println!("{answer:?}");
/// # Ok::<_, relay_rs::RelayError>(())
/// ```
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let state = Arc::new(ThreadWaker {
        notified: Mutex::new(false),
        condition: Condvar::new(),
    });
    let waker = Waker::from(Arc::clone(&state));
    let mut context = Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                let mut notified = match state.notified.lock() {
                    Ok(notified) => notified,
                    Err(poisoned) => poisoned.into_inner(),
                };
                while !*notified {
                    notified = match state.condition.wait(notified) {
                        Ok(notified) => notified,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                }
                *notified = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::block_on;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn runs_a_ready_future() {
        assert_eq!(block_on(async { 1 + 1 }), 2);
    }

    #[test]
    fn waits_for_a_wake_up() {
        let counter = Arc::new(AtomicUsize::new(0));
        let inner = Arc::clone(&counter);
        let value = block_on(async move {
            inner.fetch_add(1, Ordering::SeqCst);
            42
        });
        assert_eq!(value, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
