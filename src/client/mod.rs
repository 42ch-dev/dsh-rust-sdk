//! Low-level process client for the DeepSeek Harness runtime.
//!
//! [`HarnessClient`] spawns the official runtime binary
//! ([deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)) and
//! speaks its stdio JSON-RPC 2.0 line protocol: typed request helpers
//! ([`HarnessClient::initialize`], [`HarnessClient::session_prompt`]), a
//! session-tree notification subscription
//! ([`HarnessClient::subscribe_session_tree`]), and the documented close
//! ladder ([`HarnessClient::close`]).
//!
//! The runtime's stdout is protocol-exclusive; its **stderr is captured**
//! into a bounded 400-line tail (never inherited) that is embedded in
//! transport and close diagnostics.
//!
//! The implementation is split into focused submodules (`core` for the
//! spawn/request surface, `read_loop`, `subscription`, `session_tree`,
//! `close_ladder`) that share the state types and diagnostics helpers below;
//! the public surface is re-exported unchanged, so the crate-level API is
//! identical to a single-module layout.

mod close_ladder;
mod core;
mod read_loop;
mod session_tree;
mod subscription;

// The public surface, re-exported so `client::*` paths (and the crate-level
// re-exports in `lib.rs`) are unchanged by the split.
pub use self::core::{ClientTimeouts, HarnessClient, LaunchSpec};
pub use self::subscription::NotificationSubscription;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::error::Error;

/// Default capacity of the notification broadcast channel. When a slow
/// receiver falls more than this many notifications behind, the oldest are
/// dropped and the receiver observes `Lagged(n)` — documented drop-oldest
/// behavior, matching the bounded queues of the reference clients.
///
/// `pub(crate)` so the high-level [`Session::run`](crate::Session::run)
/// activity interval can cite the boundary when it fails fast on a lag
/// (dropped notifications can include the inbox receipt or the root-idle
/// notification this run depends on).
pub(crate) const DEFAULT_BROADCAST_CAPACITY: usize = 4096;

/// Retained stderr lines used to diagnose an unexpected runtime death
/// (Python `deque(maxlen=400)` / TS `STDERR_TAIL_LIMIT = 400` parity).
const STDERR_TAIL_LIMIT: usize = 400;

/// Upper bound for one retained stderr line, in bytes. A local guard so a
/// pathological runtime cannot grow the tail without limit; longer lines are
/// truncated (the reference clients only bound the *line count*).
const MAX_STDERR_LINE: usize = 64 * 1024;

/// Upper bound, in bytes, for the stderr tail embedded in one error string.
///
/// The retained tail can reach `STDERR_TAIL_LIMIT × MAX_STDERR_LINE`
/// (≈ 25 MiB); embedding all of it into every transport error would make a
/// chatty runtime's failures expensive to format and log. Only the newest
/// [`MAX_EMBEDDED_STDERR_BYTES`] are embedded (whole lines, oldest dropped
/// first, newest line truncated if it alone overflows).
const MAX_EMBEDDED_STDERR_BYTES: usize = 8 * 1024;

/// Maximum number of parent→child edges retained in the client-side session
/// tree.
///
/// A long-running client spawning many subagents would otherwise grow the
/// map without bound for its lifetime. When the cap is reached the oldest
/// edges are evicted (drop-oldest). The reference clients retain every edge
/// for the client lifetime; this local bound is a deliberate divergence so
/// memory stays bounded — a subscription created after an edge was evicted
/// can no longer discover that (evicted) descendant, which is acceptable for
/// a defensive bound far beyond realistic subagent counts.
const MAX_PARENT_EDGES: usize = 100_000;

/// Grace for joining the reader/stderr tasks after the runtime process has
/// been reaped (Python joins with 0.5s; a stuck task is aborted so its pipes
/// are released).
const TASK_JOIN_GRACE: Duration = Duration::from_millis(500);

/// Client state shared between the public handle, the read loop, and the
/// stderr-capture task.
#[derive(Debug, Default)]
struct SharedState {
    /// The runtime's exit code, once observed. `None` while it is still
    /// running (or when it died by signal).
    exit_code: Option<i32>,
    /// Set once the client starts closing or the read loop sees stdout EOF.
    closed: bool,
    /// Captured runtime stderr, newest last, bounded to [`STDERR_TAIL_LIMIT`].
    stderr_tail: VecDeque<String>,
}

/// The inner pending map: request id -> response sender.
type PendingMap = Mutex<HashMap<String, oneshot::Sender<Result<Value, Error>>>>;
/// Shared in-flight requests by request id (uuid-v4 string); the read loop
/// resolves the matching sender when a response arrives.
type PendingRequests = Arc<PendingMap>;

/// Build a transport-closed error with the exit status and captured stderr
/// tail appended (TS `closedError` parity).
///
/// Takes the shared state by reference — callers hold the lock — so building
/// the error never re-enters the (non-reentrant) state mutex.
fn closed_error(state: &SharedState, reason: &str) -> Error {
    let mut parts = vec![reason.to_string()];
    if let Some(code) = state.exit_code {
        parts.push(format!("exit code: {code}"));
    }
    if !state.stderr_tail.is_empty() {
        parts.push(format!(
            "stderr tail:\n{}",
            embed_stderr_tail(&state.stderr_tail)
        ));
    }
    Error::TransportClosed(parts.join("\n"))
}

/// Join the retained stderr tail for embedding in an error string, bounded
/// to the newest [`MAX_EMBEDDED_STDERR_BYTES`] bytes so a chatty runtime
/// cannot bloat every transport error. Whole lines are preferred (oldest
/// dropped first); the newest line is kept even when it alone exceeds the
/// budget, truncated to the budget.
fn embed_stderr_tail(tail: &VecDeque<String>) -> String {
    // Pick the newest span of whole lines that fits the budget, always
    // keeping the newest line.
    let mut keep_from = tail.len();
    let mut bytes = 0usize;
    for (i, line) in tail.iter().enumerate().rev() {
        let line_bytes = line.len().saturating_add(1); // + '\n' separator
        if bytes + line_bytes > MAX_EMBEDDED_STDERR_BYTES && keep_from != tail.len() {
            break;
        }
        bytes += line_bytes;
        keep_from = i;
    }
    let mut out = tail
        .iter()
        .skip(keep_from)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    // A single newest line overflowing the budget keeps its tail (the
    // newest context) rather than being dropped entirely.
    if out.len() > MAX_EMBEDDED_STDERR_BYTES {
        let start = out.len() - MAX_EMBEDDED_STDERR_BYTES;
        let cut = out
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| i >= start)
            .unwrap_or(out.len());
        out = out[cut..].to_string();
    }
    out
}

/// Build the transport-closed error for a failed close-ladder tier: the tier
/// and the underlying I/O error as the reason, with the exit status and
/// captured stderr tail appended via [`closed_error`].
///
/// Extracted from [`HarnessClient::ladder_failure`] so the diagnostics
/// contract is testable without a live child process.
fn ladder_closed_error(state: &SharedState, tier: &str, err: &io::Error) -> Error {
    closed_error(state, &format!("close ladder: {tier} failed: {err}"))
}

/// Lock a `std::sync::Mutex`, recovering from poisoning (no panic path holds
/// these locks while panicking, so a poisoned lock still exposes valid data).
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Register a pending request, failing fast when the client closed or died
/// since the caller's fast-fail check.
///
/// The closed-flag check and the insert are one state-lock critical section
/// (state → pending, the same order as the read loop's EOF drain in
/// [`fail_all_pending`]), so a drain that happened between the caller's
/// check and this call cannot strand the request: either the insert lands
/// before the drain (covered by it) or the drain ran first and the check
/// here observes the closed flag and fails fast. Returns `None` when
/// registered, or the transport-closed error to fail fast with.
fn try_register_pending(
    pending: &PendingMap,
    state: &Mutex<SharedState>,
    id: String,
    tx: oneshot::Sender<Result<Value, Error>>,
) -> Option<Error> {
    let st = lock(state);
    if st.closed || st.exit_code.is_some() {
        return Some(closed_error(&st, "DeepSeek Harness runtime is not running"));
    }
    lock(pending).insert(id, tx);
    None
}

/// Drain the pending map into transport-closed errors, marking the client
/// closed in the same critical section (used by the read loop when stdout
/// closes).
///
/// The closed flag and the drain are one state-lock critical section, so a
/// request that observed not-closed and registered (its insert takes the
/// state lock first, same order) is always covered: either it inserted
/// before this drain (resolved here) or this drain ran first and its
/// re-check in [`try_register_pending`] observes the closed flag and fails
/// fast. No interleaving strands a request after the drain.
fn fail_all_pending(pending: &PendingMap, state: &Mutex<SharedState>, reason: &str) {
    let senders: Vec<_> = {
        let mut st = lock(state);
        st.closed = true;
        let mut pending = lock(pending);
        pending.drain().map(|(_id, tx)| tx).collect()
    };
    for tx in senders {
        let _ = tx.send(Err(closed_error(&lock(state), reason)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_stderr_tail_is_byte_capped_keeping_newest() {
        // FIX-7: the tail embedded in an error string is bounded to
        // MAX_EMBEDDED_STDERR_BYTES, preferring whole newest lines.
        let short = VecDeque::from([
            "line-1".to_string(),
            "line-2".to_string(),
            "line-3".to_string(),
        ]);
        assert_eq!(
            embed_stderr_tail(&short),
            "line-1\nline-2\nline-3",
            "a tail under the budget is embedded verbatim"
        );

        // Many lines overflowing the budget: the newest lines survive, the
        // oldest are dropped, and the embedded text stays within the cap.
        let wide: VecDeque<String> = (0..1000).map(|i| format!("line-{i}")).collect();
        let embedded = embed_stderr_tail(&wide);
        assert!(
            embedded.len() <= MAX_EMBEDDED_STDERR_BYTES,
            "embedded tail must respect the byte cap, got {} bytes",
            embedded.len()
        );
        assert!(
            embedded.ends_with("line-999"),
            "the newest line must be retained"
        );
        assert!(
            !embedded.contains("line-0"),
            "the oldest lines must be dropped"
        );

        // A single newest line alone overflows the budget: its tail (the
        // newest context) is kept, truncated to the budget.
        let giant = VecDeque::from(["z".repeat(MAX_EMBEDDED_STDERR_BYTES + 100)]);
        let embedded = embed_stderr_tail(&giant);
        assert_eq!(embedded.len(), MAX_EMBEDDED_STDERR_BYTES);
        assert!(
            embedded.ends_with(&"z".repeat(MAX_EMBEDDED_STDERR_BYTES)),
            "the truncated tail must keep the newest bytes"
        );
    }

    #[test]
    fn eof_drain_marks_closed_in_the_same_critical_section_as_the_drain() {
        // The read loop used to drain pending and only then set `closed`, so
        // a request that passed the fast-fail check before the drain could
        // insert *after* it and wait forever (default request_timeout:
        // None). The drain now marks the client closed under the state lock
        // while draining: the in-flight waiter is resolved, and any request
        // that would register after the drain observes the flag.
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(SharedState::default()));

        // A request that is in flight when the runtime's stdout closes.
        let (tx, mut rx) = oneshot::channel();
        lock(&pending).insert("in-flight".to_string(), tx);

        fail_all_pending(&pending, &state, "DeepSeek Harness runtime stdout closed");

        assert!(
            matches!(rx.try_recv(), Ok(Err(Error::TransportClosed(_)))),
            "the in-flight request must be resolved by the drain"
        );
        assert!(
            lock(&state).closed,
            "closed must already be set once the drain completes — a post-drain registration must fail fast, not hang"
        );
    }

    #[test]
    fn request_insert_after_eof_drain_fails_fast_instead_of_hanging() {
        // The exact old race, deterministically: a request passes the
        // fast-fail check while the runtime is healthy, then the read loop
        // hits EOF and drains (nothing pending), then the request registers.
        // With the closed flag + drain now one critical section and the
        // insert paired with a closed re-check, the late registration fails
        // fast instead of inserting a waiter that nothing will ever resolve.
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(SharedState::default()));

        assert!(!lock(&state).closed, "the fast-fail check passes");

        fail_all_pending(&pending, &state, "DeepSeek Harness runtime stdout closed");
        assert!(lock(&state).closed, "EOF drain marks the client closed");

        let (tx, mut rx) = oneshot::channel();
        let err = try_register_pending(&pending, &state, "late".to_string(), tx)
            .expect("a post-drain registration must fail fast, not hang");
        assert!(matches!(err, Error::TransportClosed(_)));
        assert!(
            lock(&pending).is_empty(),
            "no stranded pending entry may be left behind"
        );
        assert!(
            rx.try_recv().is_err(),
            "the sender was dropped unresolved — the request failed fast instead of waiting"
        );
    }

    #[test]
    fn fail_all_pending_drains_every_waiter_with_transport_closed() {
        // Teardown must resolve *every* pending waiter with a transport-
        // closed error carrying the process diagnostics, leaving the pending
        // map empty (a later drain is a no-op — the read loop's own EOF
        // resolution can no longer double-resolve).
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(SharedState {
            exit_code: Some(1),
            closed: true,
            stderr_tail: VecDeque::from(["fatal: exploded".to_string()]),
        }));
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        lock(&pending).insert("request-1".to_string(), tx1);
        lock(&pending).insert("request-2".to_string(), tx2);

        fail_all_pending(&pending, &state, "DeepSeek Harness runtime closed");

        assert!(lock(&pending).is_empty(), "pending map must be drained");
        for mut rx in [rx1, rx2] {
            match rx.try_recv() {
                Ok(Err(Error::TransportClosed(message))) => {
                    assert!(message.contains("DeepSeek Harness runtime closed"));
                    assert!(message.contains("exit code: 1"));
                    assert!(message.contains("fatal: exploded"));
                }
                other => panic!("expected TransportClosed, got {other:?}"),
            }
        }
    }
}
