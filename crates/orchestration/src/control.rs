//! Cooperative run-control signals for an active turn: cancel, steer, pause
//! and resume.
//!
//! A turn runs asynchronously inside the daemon while clients send `CancelRun`,
//! `SteerRun`, `PauseRun` and `ResumeRun` over IPC. `RunControl` bridges those
//! commands to the orchestration loop and any waiting approval gate without
//! cancelling a running mutating tool mid-flight (which would leave an
//! ambiguous mutation). Instead:
//!
//! - `cancel` wakes any waiting approval immediately and stops the loop before
//!   the next model call or tool execution, signing the receipt as `cancelled`.
//! - `steer` enqueues operator feedback injected before the next model round.
//! - `pause` / `resume` suspends and resumes the loop between rounds.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Default)]
pub struct RunControl {
    cancelled: AtomicBool,
    cancel_reason: Mutex<Option<String>>,
    paused: AtomicBool,
    pause_reason: Mutex<Option<String>>,
    steers: Mutex<VecDeque<String>>,
    notify: Notify,
}

impl RunControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks this run as cancelled and wakes any waiting pause or approval.
    pub fn cancel(&self, reason: impl Into<String>) {
        let reason = reason.into();
        if let Ok(mut slot) = self.cancel_reason.lock() {
            if slot.is_none() {
                *slot = Some(if reason.trim().is_empty() {
                    "cancelled by operator".to_string()
                } else {
                    reason
                });
            }
        }
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancel_reason(&self) -> Option<String> {
        if !self.is_cancelled() {
            return None;
        }
        self.cancel_reason
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .or_else(|| Some("cancelled by operator".to_string()))
    }

    /// Resolves as soon as this run is cancelled.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Requests that the loop pause before its next round.
    pub fn pause(&self, reason: impl Into<String>) {
        let reason = reason.into();
        if let Ok(mut slot) = self.pause_reason.lock() {
            *slot = Some(if reason.trim().is_empty() {
                "paused by operator".to_string()
            } else {
                reason
            });
        }
        self.paused.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Resumes a paused run. Returns `true` if it had been paused.
    pub fn resume(&self) -> bool {
        let was_paused = self.paused.swap(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.pause_reason.lock() {
            *slot = None;
        }
        self.notify.notify_waiters();
        was_paused
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn pause_reason(&self) -> String {
        self.pause_reason
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| "paused by operator".to_string())
    }

    /// Waits while paused until resumed or cancelled.
    pub async fn wait_while_paused(&self) {
        loop {
            let notified = self.notify.notified();
            if !self.is_paused() || self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Queues operator steering feedback to inject before the next model round.
    pub fn steer(&self, feedback: impl Into<String>) {
        let text = feedback.into();
        if text.trim().is_empty() {
            return;
        }
        if let Ok(mut queue) = self.steers.lock() {
            queue.push_back(text);
        }
        self.notify.notify_waiters();
    }

    /// Drains all queued steering messages in arrival order.
    pub fn take_steers(&self) -> Vec<String> {
        self.steers
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn cancel_wakes_paused_and_cancelled_waiters() {
        let ctrl = Arc::new(RunControl::new());
        ctrl.pause("checking diff");
        assert!(ctrl.is_paused());

        let c2 = Arc::clone(&ctrl);
        let waiter = tokio::spawn(async move {
            c2.wait_while_paused().await;
            c2.is_cancelled()
        });

        tokio::task::yield_now().await;
        ctrl.cancel("stop now");
        assert!(waiter.await.expect("join"));
        assert_eq!(ctrl.cancel_reason().as_deref(), Some("stop now"));
    }

    #[test]
    fn steer_preserves_fifo_order() {
        let ctrl = RunControl::new();
        ctrl.steer("first");
        ctrl.steer("   ");
        ctrl.steer("second");
        assert_eq!(ctrl.take_steers(), vec!["first", "second"]);
        assert!(ctrl.take_steers().is_empty());
    }
}
