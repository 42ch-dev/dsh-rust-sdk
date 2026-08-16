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

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};
    use tokio::sync::broadcast;

    use super::*;

    /// A root-session `session.event` notification payload.
    fn event_notification(text: &str) -> Notification {
        let mut payload = Map::new();
        payload.insert("sessionId".to_string(), json!("root"));
        payload.insert("event".to_string(), json!({"type": "test", "text": text}));
        Notification {
            method: "session.event".to_string(),
            payload,
        }
    }

    /// A subscription over a fresh broadcast channel of `capacity`, with an
    /// empty tree (the root always passes the filter) and a live client
    /// state.
    fn subscription_with_capacity(
        capacity: usize,
    ) -> (broadcast::Sender<Notification>, NotificationSubscription) {
        let (tx, rx) = broadcast::channel(capacity);
        let subscription = NotificationSubscription {
            receiver: Some(rx),
            parent_map: Arc::new(Mutex::new(ParentMap::default())),
            state: Arc::new(Mutex::new(SharedState::default())),
            root: "root".to_string(),
            lagged: false,
        };
        (tx, subscription)
    }

    #[tokio::test]
    async fn overflow_sets_lagged_and_take_lagged_consumes_it() {
        // Capacity 1: the second send evicts the first, so the receiver
        // falls behind by exactly one notification — the low-level overflow
        // `Session::run`'s `ensure_no_lag` fail-fast protects against.
        let (tx, mut subscription) = subscription_with_capacity(1);

        tx.send(event_notification("first")).expect("send");
        tx.send(event_notification("second")).expect("send");

        // recv() surfaces the Lagged (recording it on the flag) and still
        // delivers the retained notification.
        let notification = subscription
            .recv()
            .await
            .expect("the retained notification is delivered");
        assert_eq!(
            notification
                .payload
                .get("event")
                .and_then(|e| e.get("text"))
                .and_then(Value::as_str),
            Some("second"),
            "the first notification was dropped; the second is delivered"
        );
        assert!(subscription.lagged, "an overflow must set the lag flag");
        assert!(
            subscription.take_lagged(),
            "take_lagged reports the recorded lag"
        );
        assert!(
            !subscription.take_lagged(),
            "take_lagged is consumed on read until the next lag"
        );

        // A stream that keeps up after the lag leaves the flag clear.
        tx.send(event_notification("third")).expect("send");
        subscription
            .recv()
            .await
            .expect("the third notification is delivered");
        assert!(
            !subscription.take_lagged(),
            "no overflow since the last read → the flag stays clear"
        );
    }
}
