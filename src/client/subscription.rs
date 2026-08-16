//! The filtered session-tree notification subscription.

use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::error::Error;
use crate::protocol::Notification;

use super::session_tree::{notification_in_tree, ParentMap};
use super::{closed_error, lock, SharedState};

/// A live, filtered subscription to one session tree's notifications.
///
/// Created by [`super::HarnessClient::subscribe_session_tree`]. Wraps a
/// `broadcast::Receiver<Notification>`; dropping the handle unsubscribes it
/// (the receiver detaches from the broadcast channel). The filter consults
/// the client-side `subagent.started` parent→child edge map **live**, so
/// descendants discovered mid-stream pass the filter from then on.
///
/// A subscription created after close (or after runtime death) is
/// born-failed: [`NotificationSubscription::recv`] rejects immediately.
#[derive(Debug)]
pub struct NotificationSubscription {
    pub(super) receiver: Option<broadcast::Receiver<Notification>>,
    pub(super) parent_map: Arc<Mutex<ParentMap>>,
    pub(super) state: Arc<Mutex<SharedState>>,
    pub(super) root: String,
    /// Set whenever the receiver has fallen behind the broadcast capacity
    /// (dropped-oldest notifications are irrecoverable). Consumed by
    /// `NotificationSubscription::take_lagged` so callers whose protocol
    /// depends on every notification (e.g. the `Session::run` activity
    /// interval) can fail fast instead of trusting a truncated stream.
    pub(super) lagged: bool,
}

impl NotificationSubscription {
    /// Wait for the next notification belonging to the subscribed tree.
    ///
    /// Already-delivered notifications are drained first, so a queue built up
    /// before close/runtime death remains readable (reference parity); once
    /// the channel (or the client) is closed, [`Error::TransportClosed`] is
    /// returned with the process diagnostics. A receiver that falls behind
    /// the broadcast capacity logs the drop and continues (documented
    /// drop-oldest behavior), but records the lag — see
    /// `NotificationSubscription::take_lagged` — so callers whose protocol
    /// depends on a lossless stream can fail fast.
    pub async fn recv(&mut self) -> Result<Notification, Error> {
        let Some(receiver) = self.receiver.as_mut() else {
            return Err(closed_error(
                &lock(&self.state),
                "DeepSeek Harness runtime closed",
            ));
        };
        loop {
            match drain_queued(receiver, &self.parent_map, &self.root) {
                DrainOutcome::Matched(notification) => return Ok(notification),
                DrainOutcome::Closed => {
                    return Err(closed_error(
                        &lock(&self.state),
                        "DeepSeek Harness runtime closed",
                    ));
                }
                DrainOutcome::Lagged => self.lagged = true,
                DrainOutcome::Empty => {}
            }
            let closed = lock(&self.state).closed;
            if closed {
                // Final drain: a notification may have landed between the
                // drain above and the closed check.
                if let DrainOutcome::Matched(notification) =
                    drain_queued(receiver, &self.parent_map, &self.root)
                {
                    return Ok(notification);
                }
                return Err(closed_error(
                    &lock(&self.state),
                    "DeepSeek Harness runtime closed",
                ));
            }
            match receiver.recv().await {
                Ok(notification) => {
                    let map = lock(&self.parent_map);
                    if notification_in_tree(&map, &notification, &self.root) {
                        return Ok(notification);
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(closed_error(
                        &lock(&self.state),
                        "DeepSeek Harness runtime closed",
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    self.lagged = true;
                    tracing::debug!(
                        skipped,
                        "notification subscription fell behind; dropped oldest \
                         notifications (documented drop-oldest behavior)"
                    );
                }
            }
        }
    }

    /// Whether the receiver has fallen behind the broadcast capacity at
    /// least once since the last call — any dropped notification is
    /// irrecoverable, so the stream may be missing notifications. Consumed
    /// on read (returns `false` on subsequent calls until the next lag).
    pub(crate) fn take_lagged(&mut self) -> bool {
        std::mem::take(&mut self.lagged)
    }
}

enum DrainOutcome {
    Matched(Notification),
    Empty,
    Closed,
    Lagged,
}

/// Pop one matching notification from the receiver's queue without waiting.
fn drain_queued(
    receiver: &mut broadcast::Receiver<Notification>,
    parent_map: &Arc<Mutex<ParentMap>>,
    root: &str,
) -> DrainOutcome {
    loop {
        match receiver.try_recv() {
            Ok(notification) => {
                let map = lock(parent_map);
                if notification_in_tree(&map, &notification, root) {
                    return DrainOutcome::Matched(notification);
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => return DrainOutcome::Empty,
            Err(broadcast::error::TryRecvError::Closed) => return DrainOutcome::Closed,
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                tracing::debug!(
                    skipped,
                    "notification subscription fell behind; dropped oldest \
                     notifications (documented drop-oldest behavior)"
                );
                return DrainOutcome::Lagged;
            }
        }
    }
}
