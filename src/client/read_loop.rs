//! The stdout read loop, frame dispatch, and the stderr-capture task.

use std::io;
use std::sync::{Arc, Mutex, Weak};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{broadcast, Notify};

use crate::error::Error;
use crate::protocol::{IncomingFrame, JsonRpcId, JsonRpcResponseOutcome, Notification};
use crate::transport::{write_frame, JsonRpcLineTransport};

use super::session_tree::{record_session_relationship, ParentMap};
use super::{
    fail_all_pending, lock, PendingMap, PendingRequests, SharedState, MAX_STDERR_LINE,
    STDERR_TAIL_LIMIT, TASK_JOIN_GRACE,
};

/// The read loop's shared context: every client handle it needs to dispatch
/// frames and to resolve the pending map (and its own state) on EOF.
pub(super) struct ReadContext {
    /// Weak stdin handle — the read loop answers client-directed requests;
    /// the client's owned handle keeps the pipe open.
    pub(super) stdin: Weak<tokio::sync::Mutex<ChildStdin>>,
    /// Weak child handle — polled once on EOF for the exit code.
    pub(super) child: Weak<tokio::sync::Mutex<Child>>,
    /// In-flight requests by request id.
    pub(super) pending: PendingRequests,
    /// The client-side session-tree edge map.
    pub(super) parent_map: Arc<Mutex<ParentMap>>,
    /// Notification producer fanned out to subscriptions.
    pub(super) notifications: broadcast::Sender<Notification>,
    /// Shared client state (exit code, closed flag, stderr tail).
    pub(super) state: Arc<Mutex<SharedState>>,
    /// Signalled when the captured stderr stream has fully drained.
    pub(super) stderr_done: Arc<Notify>,
}

/// The read loop: owns stdout, dispatches frames, and terminates the pending
/// map when the transport closes.
pub(super) async fn read_loop(
    mut transport: JsonRpcLineTransport<ChildStdout, tokio::io::Sink>,
    ctx: ReadContext,
) {
    loop {
        match transport.read_frame().await {
            Ok(Some(frame)) => {
                dispatch_frame(
                    frame,
                    &ctx.pending,
                    &ctx.parent_map,
                    &ctx.notifications,
                    &ctx.stdin,
                )
                .await
            }
            Ok(None) => break, // EOF: the runtime's stdout closed.
            Err(err) => {
                tracing::warn!(error = %err, "runtime stdout read failed; closing transport");
                break;
            }
        }
    }
    // Best-effort: poll the child once so EOF-path diagnostics carry the
    // exit code (plan contract: EOF resolves pending with exit code +
    // stderr tail). The child lock may be held by the close ladder or the
    // child may already have been waited on — skip silently either way.
    if let Some(child) = ctx.child.upgrade() {
        if let Ok(mut guard) = child.try_lock() {
            if let Ok(Some(status)) = guard.try_wait() {
                lock(&ctx.state).exit_code = status.code();
            }
        }
    }
    // Give the stderr task a bounded moment to drain its pipe so the
    // EOF-path error embeds the complete captured tail. When the process
    // died (the common EOF case) the pipe closes immediately; the bound
    // only guards a grandchild keeping stderr open after stdout closed.
    tokio::time::timeout(TASK_JOIN_GRACE, ctx.stderr_done.notified())
        .await
        .ok();
    fail_all_pending(
        &ctx.pending,
        &ctx.state,
        "DeepSeek Harness runtime stdout closed",
    );
    lock(&ctx.state).closed = true;
}

/// Dispatch one parsed frame: response / client-directed request /
/// notification.
async fn dispatch_frame(
    frame: Value,
    pending: &PendingMap,
    parent_map: &Mutex<ParentMap>,
    notifications: &broadcast::Sender<Notification>,
    stdin: &Weak<tokio::sync::Mutex<ChildStdin>>,
) {
    let frame = match serde_json::from_value::<IncomingFrame>(frame) {
        Ok(frame) => frame,
        Err(err) => {
            // A JSON line that matches none of the JSON-RPC frame shapes is
            // ignored (the line transport already skips non-JSON lines).
            tracing::debug!(error = %err, "ignoring frame that matches no JSON-RPC shape");
            return;
        }
    };
    match frame {
        IncomingFrame::Response(response) => {
            let key = match &response.id {
                JsonRpcId::String(id) => id.clone(),
                JsonRpcId::Number(id) => id.to_string(),
            };
            let waiter = lock(pending).remove(&key);
            if let Some(tx) = waiter {
                let outcome = match response.outcome {
                    JsonRpcResponseOutcome::Success { result } => Ok(result),
                    JsonRpcResponseOutcome::Error { error } => Err(Error::JsonRpc {
                        code: error.code,
                        message: error.message,
                        data: error.data,
                    }),
                };
                let _ = tx.send(outcome);
            }
            // A response for an unknown id (late response, or a response for
            // an abandoned request) is dropped.
        }
        IncomingFrame::Request(request) => {
            // The DSH server currently sends no client-directed requests; the
            // answer mirrors the reference transport's method-not-found.
            let Some(stdin) = stdin.upgrade() else {
                return; // stdin already closed; nothing to answer with
            };
            let frame = json!({
                "jsonrpc": "2.0",
                "id": request.id,
                "error": { "code": -32601, "message": format!("method not found: {}", request.method) },
            });
            let mut stdin = stdin.lock().await;
            if let Err(err) = write_frame(&mut *stdin, &frame).await {
                tracing::debug!(error = %err, "failed to answer client-directed request; runtime stdin is closed");
            }
        }
        IncomingFrame::Notification(notification) => {
            // Update the parent map before fan-out so the tree filter sees
            // the fresh edge (descendants discovered mid-stream match from
            // their first event onward).
            record_session_relationship(&mut lock(parent_map), &notification);
            let _ = notifications.send(notification);
        }
    }
}

/// The stderr-capture task: keeps the bounded 400-line tail of the runtime's
/// stderr in the shared state for transport/close diagnostics.
///
/// Lines are read through [`read_line_capped`], so a pathological
/// newline-less stream can never grow the in-flight line buffer past
/// [`MAX_STDERR_LINE`] — the byte bound is enforced *during* accumulation,
/// not after (unlike a plain `read_until` + truncate, which would buffer the
/// whole unterminated line first).
pub(super) async fn stderr_loop(
    stderr: ChildStderr,
    state: Arc<Mutex<SharedState>>,
    stderr_done: Arc<Notify>,
) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_line_capped(&mut reader, &mut line, MAX_STDERR_LINE).await {
            Ok(false) => break, // EOF
            Ok(true) => {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end(); // strips the newline and trailing whitespace
                if !text.is_empty() {
                    let mut st = lock(&state);
                    st.stderr_tail.push_back(text.to_string());
                    while st.stderr_tail.len() > STDERR_TAIL_LIMIT {
                        st.stderr_tail.pop_front();
                    }
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "runtime stderr read failed");
                break;
            }
        }
    }
    // Signal the read loop (if it is on the EOF path) that the captured
    // stderr stream has fully drained, so the EOF error can embed the
    // complete tail.
    stderr_done.notify_waiters();
}

/// Read one line into `buf`, retaining at most `max_bytes` bytes.
///
/// Returns `Ok(true)` when a line was read (a partial line at EOF counts as
/// a line, mirroring `readline()`), `Ok(false)` on EOF with nothing
/// buffered. The first `max_bytes` of a longer line are retained; the
/// remainder is drained in-flight and discarded, so the caller's buffer is
/// the only memory a newline-less stream can consume. The same incremental
/// guard the transport applies to stdout framing, applied to the captured
/// stderr stream.
async fn read_line_capped<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<bool, io::Error> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(!buf.is_empty());
        }
        let newline = available.iter().position(|&b| b == b'\n');
        let content = newline.unwrap_or(available.len());
        if buf.len() < max_bytes {
            let head = content.min(max_bytes - buf.len());
            buf.extend_from_slice(&available[..head]);
        }
        let consumed = newline.map_or(available.len(), |pos| pos + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(true);
        }
        if buf.len() >= max_bytes {
            // The retained prefix is capped; drain the rest of the line (or
            // EOF) without buffering, then report the line.
            drain_line(reader).await?;
            return Ok(true);
        }
    }
}

/// Consume input up to (and including) the next `\n`, or EOF, without
/// buffering.
async fn drain_line<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<(), io::Error> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(()); // EOF; the retained prefix is still reported
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                reader.consume(pos + 1);
                return Ok(());
            }
            None => {
                let len = available.len();
                reader.consume(len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_line_capped_bounds_in_flight_allocation_and_preserves_lines() {
        // FIX-1 regression: a newline-less blob must not grow the capture
        // buffer; only the first `max_bytes` are retained, the remainder is
        // drained, and following lines read normally afterwards.
        use tokio::io::{duplex, AsyncWriteExt};

        let (client_rx, mut server_tx) = duplex(64);
        let writer = tokio::spawn(async move {
            let giant = b"x".repeat(100);
            server_tx.write_all(&giant).await.unwrap();
            server_tx.write_all(b"\nshort\n").await.unwrap();
        });

        let mut reader = BufReader::new(client_rx);
        let mut buf = Vec::new();

        assert!(
            read_line_capped(&mut reader, &mut buf, 16).await.unwrap(),
            "the capped prefix of the giant line must be reported as a line"
        );
        assert_eq!(buf, b"xxxxxxxxxxxxxxxx", "only the first 16 bytes retained");

        buf.clear();
        assert!(
            read_line_capped(&mut reader, &mut buf, 16).await.unwrap(),
            "the following short line must read intact"
        );
        assert_eq!(buf, b"short");

        buf.clear();
        assert!(
            !read_line_capped(&mut reader, &mut buf, 16).await.unwrap(),
            "EOF with nothing buffered must report Ok(false)"
        );
        writer.await.unwrap();
    }
}
