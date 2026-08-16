//! The close ladder, unconditional teardown, and drop-time task abort.

use std::io;

use tokio::process::Child;

use crate::error::Error;

use super::HarnessClient;
use super::{closed_error, ladder_closed_error, lock, STDERR_TAIL_LIMIT, TASK_JOIN_GRACE};

impl HarnessClient {
    /// Shut the runtime down and reap it, resolving only after it exited.
    ///
    /// The close ladder per [`super::ClientTimeouts`]: a cooperative
    /// `shutdown` request bounded by `shutdown_timeout` (diagnostic only on
    /// failure) → drop stdin (EOF) → wait `eof_grace` → SIGTERM → wait
    /// `term_grace` → SIGKILL → wait. Pending requests are resolved with
    /// [`Error::TransportClosed`] and the read loop is joined; failure paths
    /// surface the exit status and captured stderr tail. Idempotent.
    ///
    /// Teardown is unconditional: even when a ladder tier fails, pending
    /// requests are resolved, the read/stderr tasks are joined (or aborted
    /// after the join grace), and the notification producer is dropped. The
    /// child is killed on drop (`kill_on_drop`), so it cannot outlive the
    /// client; the returned error names the failing tier with the exit
    /// status and stderr tail attached, and a later `close()` — seeing the
    /// already-consistent terminal state — returns `Ok(())`.
    pub async fn close(&mut self) -> Result<(), Error> {
        // Idempotent: a second close (or a close of an already-closed client)
        // returns immediately — the terminal state below is consistent.
        let Some(child_arc) = self.child.take() else {
            return Ok(());
        };

        // Tier 1: cooperative `shutdown`, bounded. Failure is diagnostic
        // only — the dispose ladder below is the authoritative teardown.
        let shutdown = tokio::time::timeout(
            self.timeouts.shutdown_timeout,
            self.request("shutdown", None),
        )
        .await;
        match shutdown {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                tracing::debug!(error = %err, "shutdown request failed; dispose ladder is authoritative");
                self.append_diagnostic(format!("shutdown request failed: {err}"));
            }
            Err(_elapsed) => {
                tracing::debug!("shutdown request timed out; dispose ladder is authoritative");
                self.append_diagnostic("shutdown request timed out".to_string());
            }
        }

        // From here on the client is closed: new requests fast-fail and the
        // read loop must not answer client-directed requests.
        lock(&self.state).closed = true;

        // Tier 2: drop stdin -> EOF on the runtime's stdin. The read loop
        // holds only a weak reference, so the pipe truly closes here.
        drop(self.stdin.take());

        // Tiers 3-6: wait for exit after EOF, then escalate through SIGTERM
        // and SIGKILL, waiting at each tier. The child is killed on drop
        // (`kill_on_drop`) when `close` returns, so a ladder failure cannot
        // strand the process; the error carries the failing tier, exit
        // status, and stderr tail (see `reap_with_ladder`).
        let reap_error = {
            let mut child = child_arc.lock().await;
            self.reap_with_ladder(&mut child).await.err()
        };

        // Teardown is unconditional: once the client is closed the runtime
        // can never answer pending requests, so every waiter is resolved
        // here, and the background tasks must not outlive the client.
        self.finish_teardown().await;

        match reap_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Resolve every pending request, join (or abort) the background tasks,
    /// and drop the notification producer — the unconditional teardown tail
    /// of [`HarnessClient::close`].
    ///
    /// Runs no matter how the reap ladder ended: pending requests can never
    /// be answered once the client is closed (the read loop's own EOF
    /// resolution becomes a no-op once the map is empty), and the tasks
    /// must not outlive the client.
    async fn finish_teardown(&mut self) {
        self.fail_all_pending("DeepSeek Harness runtime closed");

        // Join the reader and stderr tasks; abort a task stuck on a pipe a
        // grandchild kept open after the process died.
        for handle in [&mut self.read_task, &mut self.stderr_task] {
            if let Some(mut handle) = handle.take() {
                match tokio::time::timeout(TASK_JOIN_GRACE, &mut handle).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        tracing::debug!(error = %err, "client background task failed");
                    }
                    Err(_elapsed) => handle.abort(),
                }
            }
        }

        // Drop the notification producer: existing subscriptions drain their
        // queues and then see the channel close.
        self.notifications = None;
    }

    /// Run the exit-wait ladder: stdin-EOF grace, then SIGTERM, then SIGKILL.
    ///
    /// Every failure returns [`Error::TransportClosed`] naming the failing
    /// tier and the underlying I/O error, with the exit status (best-effort
    /// poll) and captured stderr tail attached — the same diagnostics the
    /// request fast-fail and EOF paths surface.
    async fn reap_with_ladder(&self, child: &mut Child) -> Result<(), Error> {
        match child.try_wait() {
            Ok(Some(_)) => {
                self.record_exit(child);
                return Ok(());
            }
            Ok(None) => {}
            Err(err) => return Err(self.ladder_failure(child, "poll exit status", err)),
        }
        match tokio::time::timeout(self.timeouts.eof_grace, child.wait()).await {
            Ok(Ok(_status)) => {
                self.record_exit(child);
                return Ok(());
            }
            Ok(Err(err)) => {
                return Err(self.ladder_failure(child, "wait for exit after stdin EOF", err))
            }
            Err(_elapsed) => {}
        }

        // POSIX gets a catchable graceful signal; platforms without it (the
        // non-goal Windows tier) skip straight to forced termination, like
        // the TypeScript client.
        #[cfg(unix)]
        {
            if let Err(err) = signal_child(child, Signal::SIGTERM) {
                // The child may have exited between the wait timeout and now.
                match child.try_wait() {
                    Ok(Some(_)) => {
                        self.record_exit(child);
                        return Ok(());
                    }
                    Ok(None) => {}
                    Err(poll_err) => {
                        return Err(self.ladder_failure(
                            child,
                            "poll exit status after SIGTERM",
                            poll_err,
                        ));
                    }
                }
                return Err(self.ladder_failure(child, "send SIGTERM", err));
            }
            match tokio::time::timeout(self.timeouts.term_grace, child.wait()).await {
                Ok(Ok(_status)) => {
                    self.record_exit(child);
                    return Ok(());
                }
                Ok(Err(err)) => {
                    return Err(self.ladder_failure(child, "wait for exit after SIGTERM", err))
                }
                Err(_elapsed) => {}
            }
        }

        // Forced termination, then wait without a bound.
        if let Err(err) = child.start_kill() {
            // The child may have exited between the wait timeout and now.
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.record_exit(child);
                    return Ok(());
                }
                Ok(None) => {}
                Err(poll_err) => {
                    return Err(self.ladder_failure(
                        child,
                        "poll exit status after SIGKILL",
                        poll_err,
                    ));
                }
            }
            return Err(self.ladder_failure(child, "force-terminate with SIGKILL", err));
        }
        match child.wait().await {
            Ok(_status) => {
                self.record_exit(child);
                Ok(())
            }
            Err(err) => Err(self.ladder_failure(child, "wait for exit after SIGKILL", err)),
        }
    }

    /// Build the transport-closed error for a failed close-ladder tier: the
    /// tier and the underlying I/O error, with the exit status (best-effort
    /// poll) and captured stderr tail appended — the same diagnostics the
    /// request fast-fail and EOF paths surface.
    fn ladder_failure(&self, child: &mut Child, tier: &str, err: io::Error) -> Error {
        self.record_exit(child);
        ladder_closed_error(&lock(&self.state), tier, &err)
    }

    /// Cache the child's exit code in the shared state for diagnostics.
    fn record_exit(&self, child: &mut Child) {
        if let Ok(Some(status)) = child.try_wait() {
            lock(&self.state).exit_code = status.code();
        }
    }

    /// Append one line to the bounded stderr-tail diagnostics.
    fn append_diagnostic(&self, line: String) {
        if line.is_empty() {
            return;
        }
        let mut st = lock(&self.state);
        st.stderr_tail.push_back(line);
        while st.stderr_tail.len() > STDERR_TAIL_LIMIT {
            st.stderr_tail.pop_front();
        }
    }

    /// Resolve every pending request with a transport-closed error carrying
    /// the exit status and captured stderr tail.
    fn fail_all_pending(&self, reason: &str) {
        let senders: Vec<_> = {
            let mut pending = lock(&self.pending);
            pending.drain().map(|(_id, tx)| tx).collect()
        };
        for tx in senders {
            let _ = tx.send(Err(closed_error(&lock(&self.state), reason)));
        }
    }
}

impl Drop for HarnessClient {
    fn drop(&mut self) {
        // Best-effort cleanup for a client dropped without close(): abort the
        // background tasks so they cannot linger on pipes a grandchild kept
        // open. `close()` already joins (or aborts) them via
        // `finish_teardown`, which takes the handles — so this fires only on
        // the drop-without-close path. The child itself is killed on drop
        // (`kill_on_drop`), and aborting the tasks releases their pipe
        // halves.
        if let Some(handle) = self.read_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.stderr_task.take() {
            handle.abort();
        }
    }
}

/// Send a signal to the runtime child process (the close ladder's SIGTERM
/// tier).
#[cfg(unix)]
fn signal_child(child: &Child, signal: Signal) -> io::Result<()> {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child process has no pid"))?;
    kill(Pid::from_raw(pid as i32), signal).map_err(io::Error::other)
}

#[cfg(unix)]
use nix::sys::signal::Signal;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::sync::{broadcast, oneshot};

    use super::super::session_tree::ParentMap;
    use super::super::{ClientTimeouts, PendingRequests, SharedState};

    #[test]
    fn ladder_failures_embed_exit_code_and_stderr_tail() {
        // A tier failure must carry the same diagnostics as the request
        // fast-fail and EOF paths: the tier, the I/O error, the exit status,
        // and the captured stderr tail.
        let state = SharedState {
            exit_code: Some(2),
            closed: true,
            stderr_tail: VecDeque::from(["boom: kaboom".to_string()]),
        };
        let err = io::Error::other("synthetic io failure");
        match ladder_closed_error(&state, "send SIGTERM", &err) {
            Error::TransportClosed(message) => {
                assert!(message.contains("close ladder: send SIGTERM failed"));
                assert!(message.contains("synthetic io failure"));
                assert!(message.contains("exit code: 2"));
                assert!(message.contains("stderr tail:\nboom: kaboom"));
            }
            other => panic!("expected TransportClosed, got {other:?}"),
        }

        // When nothing has been observed, the tier error still names itself
        // without fabricating exit/stderr context.
        let err = io::Error::other("still alive");
        match ladder_closed_error(&SharedState::default(), "wait for exit after SIGKILL", &err) {
            Error::TransportClosed(message) => {
                assert!(message.contains("close ladder: wait for exit after SIGKILL failed"));
                assert!(message.contains("still alive"));
                assert!(!message.contains("exit code"));
                assert!(!message.contains("stderr tail"));
            }
            other => panic!("expected TransportClosed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropping_without_close_aborts_background_tasks() {
        // FIX-11: a client dropped without close() must abort its background
        // tasks (they cannot be awaited inside Drop). The task body installs
        // a Drop guard; aborting drops the future, which runs the guard —
        // the only observable signal from outside the task.
        use std::sync::atomic::{AtomicBool, Ordering};

        let aborted = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&aborted);
        let task = tokio::spawn(async move {
            struct Guard(Arc<AtomicBool>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _guard = Guard(flag);
            // Never completes on its own; each sleep registers a waker so an
            // abort can wake and drop the task.
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        let client = HarnessClient {
            child: None,
            stdin: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            parent_map: Arc::new(Mutex::new(ParentMap::new())),
            state: Arc::new(Mutex::new(SharedState::default())),
            notifications: None,
            read_task: Some(task),
            stderr_task: None,
            timeouts: ClientTimeouts::default(),
        };
        // Let the background task start (a real client's read/stderr tasks
        // are always actively polled on their pipes); aborting a task that
        // has never been polled would not drop it until runtime shutdown.
        tokio::task::yield_now().await;
        drop(client);

        // Abort is processed asynchronously on the runtime; poll briefly.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !aborted.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "read task must be aborted when the client is dropped without close()"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn finish_teardown_resolves_pending_and_joins_tasks() {
        // The unconditional teardown tail of close(): pending requests are
        // resolved, both background tasks are joined (their handles taken),
        // and the notification producer is dropped — the terminal state
        // that makes a second close() after a ladder failure safe.
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let parent_map = Arc::new(Mutex::new(ParentMap::new()));
        let state = Arc::new(Mutex::new(SharedState::default()));
        let (notifications, _receiver) = broadcast::channel(8);
        let mut client = HarnessClient {
            child: None,
            stdin: None,
            pending: Arc::clone(&pending),
            parent_map,
            state,
            notifications: Some(notifications),
            read_task: Some(tokio::spawn(async {})),
            stderr_task: Some(tokio::spawn(async {})),
            timeouts: ClientTimeouts::default(),
        };
        let (tx, mut rx) = oneshot::channel();
        lock(&pending).insert("in-flight".to_string(), tx);

        client.finish_teardown().await;

        assert!(lock(&pending).is_empty(), "pending map must be drained");
        assert!(client.read_task.is_none(), "read task must be joined");
        assert!(client.stderr_task.is_none(), "stderr task must be joined");
        assert!(
            client.notifications.is_none(),
            "notification producer must be dropped"
        );
        match rx.try_recv() {
            Ok(Err(Error::TransportClosed(message))) => {
                assert!(message.contains("DeepSeek Harness runtime closed"));
            }
            other => panic!("expected TransportClosed, got {other:?}"),
        }
    }
}
