//! The stop button, as something the loop and the transport can both hold.
//!
//! Replaces the `AbortSignal` the TypeScript passed around. Cloning shares the
//! flag rather than copying it, so a token handed to a request and a token kept
//! by the session are the same switch.
//!
//! The silence deadline that used to be wound into this lives in the transport
//! instead: a read that is given its whole allowance every time it is called
//! measures silence exactly, and needs no timer of its own to reset.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

#[derive(Debug, Default)]
struct State {
    stopped: AtomicBool,
    woken: Notify,
}

#[derive(Debug, Clone, Default)]
pub struct Cancel {
    state: Arc<State>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks whatever holds this to stop. Safe to call more than once.
    pub fn cancel(&self) {
        self.state.stopped.store(true, Ordering::SeqCst);
        // `notify_waiters` would miss anyone who has not reached the await yet,
        // which is exactly the race a stop button loses. This one is remembered.
        self.state.woken.notify_last();
        self.state.woken.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.stopped.load(Ordering::SeqCst)
    }

    /// Resolves once cancelled, and immediately if it already is.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        loop {
            let waiting = self.state.woken.notified();
            if self.is_cancelled() {
                return;
            }
            waiting.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_wakes_a_waiter() {
        let cancel = Cancel::new();
        let watcher = cancel.clone();
        let task = tokio::spawn(async move { watcher.cancelled().await });
        // Yield so the task is parked on the notify before the switch is thrown.
        tokio::task::yield_now().await;
        cancel.cancel();
        task.await.unwrap();
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn a_token_already_cancelled_resolves_at_once() {
        let cancel = Cancel::new();
        cancel.cancel();
        cancel.cancelled().await;
    }

    #[test]
    fn a_clone_is_the_same_switch() {
        let cancel = Cancel::new();
        let other = cancel.clone();
        assert!(!other.is_cancelled());
        cancel.cancel();
        assert!(other.is_cancelled());
    }
}
