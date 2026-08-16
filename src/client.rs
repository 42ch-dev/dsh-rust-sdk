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

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, oneshot, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::Error;
use crate::protocol::{
    ContentBlock, IncomingFrame, InitializeParams, InitializeResult, JsonRpcId,
    JsonRpcResponseOutcome, Notification, SessionPromptParams, SessionPromptResult,
};
use crate::transport::{write_frame, JsonRpcLineTransport};

/// How to launch the runtime process (the official
/// `deepseek-harness-sdk-runtime` binary).
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Path to (or name of) the runtime executable.
    pub program: String,
    /// Extra command-line arguments passed to the runtime.
    pub args: Vec<String>,
    /// Environment overrides; the parent environment is inherited and these
    /// entries are layered on top.
    pub envs: HashMap<String, String>,
    /// Working directory for the runtime process, when set.
    pub cwd: Option<PathBuf>,
}

/// Timeout ladder for requests and the close sequence.
///
/// Defaults follow the TypeScript client (`shutdownTimeoutMs` 1000 /
/// `disposeEofGraceMs` 6000 / `disposeGraceMs` 3000). The longer EOF grace
/// gives the runtime time to flush durable state after stdin closes.
#[derive(Debug, Clone, Copy)]
pub struct ClientTimeouts {
    /// Per-request response deadline. `None` waits indefinitely (the Python
    /// SDK default). There is no wire-level cancellation: on timeout the
    /// client abandons the wait and removes the pending entry, while the
    /// server-side work still runs until close.
    pub request_timeout: Option<Duration>,
    /// Bound for the cooperative `shutdown` request during [`HarnessClient::close`].
    pub shutdown_timeout: Duration,
    /// How long to wait for the runtime to exit after stdin EOF before
    /// escalating to SIGTERM.
    pub eof_grace: Duration,
    /// How long to wait for the runtime to exit after SIGTERM before
    /// escalating to SIGKILL.
    pub term_grace: Duration,
}

impl Default for ClientTimeouts {
    fn default() -> Self {
        Self {
            request_timeout: None,
            shutdown_timeout: Duration::from_secs(1),
            eof_grace: Duration::from_secs(6),
            term_grace: Duration::from_secs(3),
        }
    }
}

/// Default capacity of the notification broadcast channel. When a slow
/// receiver falls more than this many notifications behind, the oldest are
/// dropped and the receiver observes `Lagged(n)` — documented drop-oldest
/// behavior, matching the bounded queues of the reference clients.
const DEFAULT_BROADCAST_CAPACITY: usize = 4096;

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

/// A live, filtered subscription to one session tree's notifications.
///
/// Created by [`HarnessClient::subscribe_session_tree`]. Wraps a
/// `broadcast::Receiver<Notification>`; dropping the handle unsubscribes it
/// (the receiver detaches from the broadcast channel). The filter consults
/// the client-side `subagent.started` parent→child edge map **live**, so
/// descendants discovered mid-stream pass the filter from then on.
///
/// A subscription created after close (or after runtime death) is
/// born-failed: [`NotificationSubscription::recv`] rejects immediately.
#[derive(Debug)]
pub struct NotificationSubscription {
    receiver: Option<broadcast::Receiver<Notification>>,
    parent_map: Arc<Mutex<ParentMap>>,
    state: Arc<Mutex<SharedState>>,
    root: String,
}

impl NotificationSubscription {
    /// Wait for the next notification belonging to the subscribed tree.
    ///
    /// Already-delivered notifications are drained first, so a queue built up
    /// before close/runtime death remains readable (reference parity); once
    /// the channel (or the client) is closed, [`Error::TransportClosed`] is
    /// returned with the process diagnostics. A receiver that falls behind
    /// the broadcast capacity logs the drop and continues (documented
    /// drop-oldest behavior).
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
                    tracing::debug!(
                        skipped,
                        "notification subscription fell behind; dropped oldest \
                         notifications (documented drop-oldest behavior)"
                    );
                }
            }
        }
    }
}

enum DrainOutcome {
    Matched(Notification),
    Empty,
    Closed,
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
            }
        }
    }
}

/// Low-level JSON-RPC client for the DeepSeek Harness SDK runtime over
/// subprocess stdio.
///
/// A spawned client owns a read-loop task that parses stdout frames into
/// responses (resolving the matching pending request), notifications (fanned
/// out to subscriptions), and client-directed server requests (answered with
/// `-32601`, mirroring the reference transport — the DSH server currently
/// sends none, so this path is defensive).
#[derive(Debug)]
pub struct HarnessClient {
    /// The runtime process. Locked for non-blocking exit polls and the close
    /// ladder; `None` once closed.
    child: Option<Arc<tokio::sync::Mutex<Child>>>,
    /// Shared stdin write half. The read loop holds only a [`Weak`] reference
    /// so dropping this (stdin EOF) actually closes the runtime's stdin.
    stdin: Option<Arc<tokio::sync::Mutex<ChildStdin>>>,
    /// In-flight requests by request id (uuid-v4 string).
    pending: PendingRequests,
    /// `subagent.started` parent→child session edges (client-side tree).
    parent_map: Arc<Mutex<ParentMap>>,
    /// Shared client state (exit code, closed flag, stderr tail).
    state: Arc<Mutex<SharedState>>,
    /// Notification producer; `None` after close (subscriptions then drain
    /// their queues and see the channel close).
    notifications: Option<broadcast::Sender<Notification>>,
    /// The stdout read-loop task, joined by [`HarnessClient::close`].
    read_task: Option<JoinHandle<()>>,
    /// The stderr-capture task.
    stderr_task: Option<JoinHandle<()>>,
    /// The configured timeout ladder.
    timeouts: ClientTimeouts,
}

impl HarnessClient {
    /// Spawn the runtime process with a default 4096-notification broadcast
    /// capacity and start reading its stdout.
    ///
    /// # Tokio runtime requirement
    ///
    /// This function starts background Tokio tasks ([`tokio::spawn`]) and
    /// takes the subprocess's stdio halves, so it MUST be called from within
    /// an active Tokio runtime — typically a `#[tokio::main]` function or a
    /// `#[tokio::test]`. Called from outside a runtime it panics (no reactor
    /// is running) and the returned client is unusable.
    ///
    /// The runtime's stderr is captured to a bounded 400-line tail (not
    /// inherited) and embedded in transport/close diagnostics.
    pub fn spawn(spec: LaunchSpec, timeouts: ClientTimeouts) -> Result<Self, Error> {
        Self::spawn_with_broadcast_capacity(spec, timeouts, DEFAULT_BROADCAST_CAPACITY)
    }

    /// Like [`HarnessClient::spawn`], with an explicit broadcast capacity for
    /// the notification channel (the `Lagged(n)` drop-oldest behavior is
    /// documented on [`NotificationSubscription`]).
    ///
    /// Like [`HarnessClient::spawn`], this MUST be called from within an
    /// active Tokio runtime (typically `#[tokio::main]` / `#[tokio::test]`):
    /// the spawn functions start background tasks and panic outside a
    /// runtime.
    pub fn spawn_with_broadcast_capacity(
        spec: LaunchSpec,
        timeouts: ClientTimeouts,
        broadcast_capacity: usize,
    ) -> Result<Self, Error> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(&spec.envs)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Error::RuntimeNotFound(format!("{}: {err}", spec.program)));
            }
            Err(err) => return Err(Error::Io(err)),
        };
        let stdin = child
            .stdin
            .take()
            .expect("stdin was piped; take cannot fail");
        let stdout = child
            .stdout
            .take()
            .expect("stdout was piped; take cannot fail");
        let stderr = child
            .stderr
            .take()
            .expect("stderr was piped; take cannot fail");

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let parent_map = Arc::new(Mutex::new(ParentMap::new()));
        let state = Arc::new(Mutex::new(SharedState::default()));
        let (notifications, _) = broadcast::channel(broadcast_capacity.max(1));

        let stdin_shared = Arc::new(tokio::sync::Mutex::new(stdin));
        let child_shared = Arc::new(tokio::sync::Mutex::new(child));

        let stderr_done = Arc::new(Notify::new());
        let read_ctx = ReadContext {
            stdin: Arc::downgrade(&stdin_shared),
            child: Arc::downgrade(&child_shared),
            pending: Arc::clone(&pending),
            parent_map: Arc::clone(&parent_map),
            notifications: notifications.clone(),
            state: Arc::clone(&state),
            stderr_done: Arc::clone(&stderr_done),
        };
        let read_task = tokio::spawn(async move {
            let transport = JsonRpcLineTransport::new(stdout, tokio::io::sink());
            read_loop(transport, read_ctx).await;
        });

        let stderr_state = Arc::clone(&state);
        let stderr_task =
            tokio::spawn(async move { stderr_loop(stderr, stderr_state, stderr_done).await });

        Ok(Self {
            child: Some(child_shared),
            stdin: Some(stdin_shared),
            pending,
            parent_map,
            state,
            notifications: Some(notifications),
            read_task: Some(read_task),
            stderr_task: Some(stderr_task),
            timeouts,
        })
    }

    /// Send one JSON-RPC request and await its result.
    ///
    /// Allocates a uuid-v4 request id, registers a pending slot, writes the
    /// frame (the pending entry is registered **before** the write so a fast
    /// response cannot be mistaken for an unknown id), and awaits the
    /// response. When [`ClientTimeouts::request_timeout`] is set, the wait is
    /// abandoned on timeout and the pending entry removed — there is no
    /// wire-level cancellation, so server-side work continues. Responses for
    /// unknown ids are dropped. When the runtime is already dead (or spawn
    /// failed), fails fast with the exit code and captured stderr tail.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, Error> {
        // Fast-fail on a closed or dead runtime, with process context.
        {
            let st = lock(&self.state);
            if st.closed {
                return Err(closed_error(&st, "DeepSeek Harness runtime is not running"));
            }
            if st.exit_code.is_some() {
                return Err(closed_error(&st, "DeepSeek Harness runtime is not running"));
            }
        }
        if let Some(child) = &self.child {
            let mut guard = child.lock().await;
            if let Some(status) = guard.try_wait()? {
                let code = status.code();
                lock(&self.state).exit_code = code;
                return Err(closed_error(
                    &lock(&self.state),
                    "DeepSeek Harness runtime is not running",
                ));
            }
        }

        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        // Register before writing so a response that races the write is not
        // dropped as an unknown id (reference parity).
        lock(&self.pending).insert(id.clone(), tx);

        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
        });
        let stdin_arc = match &self.stdin {
            Some(stdin) => stdin.clone(),
            None => {
                lock(&self.pending).remove(&id);
                return Err(closed_error(
                    &lock(&self.state),
                    "DeepSeek Harness runtime is not running",
                ));
            }
        };
        {
            let mut stdin = stdin_arc.lock().await;
            if let Err(err) = write_frame(&mut *stdin, &frame).await {
                drop(stdin);
                lock(&self.pending).remove(&id);
                // The runtime died between the fast-fail check and the write
                // (EPIPE on a closed pipe); surface it with process context.
                return Err(closed_error(
                    &lock(&self.state),
                    &format!("failed to write to DeepSeek Harness runtime: {err}"),
                ));
            }
        }

        let outcome = match self.timeouts.request_timeout {
            Some(duration) => match tokio::time::timeout(duration, rx).await {
                Ok(result) => result,
                Err(elapsed) => {
                    // Timeout abandonment: remove the pending entry so a late
                    // response is dropped; the server-side work continues.
                    lock(&self.pending).remove(&id);
                    return Err(Error::RequestTimeout {
                        method: method.to_string(),
                        source: elapsed,
                    });
                }
            },
            None => rx.await,
        };
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(err),
            Err(_recv) => Err(closed_error(
                &lock(&self.state),
                "DeepSeek Harness runtime is not running",
            )),
        }
    }

    /// Perform the process-wide SDK handshake and validate the server
    /// identity.
    ///
    /// Rejects `max_tokens == 0` (the server requires a positive integer) and
    /// returns [`Error::SdkProtocol`] when `serverInfo.name` is absent or not
    /// `deepseek-harness-sdk-runtime`, or when `version` is absent — the
    /// protocol declares the name wire-stable and has no version negotiation,
    /// so an unexpected identity is a hard protocol error.
    pub async fn initialize(
        &mut self,
        cwd: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        max_tokens: Option<u32>,
    ) -> Result<InitializeResult, Error> {
        if max_tokens == Some(0) {
            return Err(Error::SdkProtocol {
                message: "maxTokens must be a positive integer".into(),
            });
        }
        let params = InitializeParams {
            cwd: cwd.into(),
            provider: provider.into(),
            model: model.into(),
            max_tokens,
        };
        let result = self
            .request("initialize", Some(serde_json::to_value(params)?))
            .await?;
        let init: InitializeResult =
            serde_json::from_value(result).map_err(|err| Error::SdkProtocol {
                message: format!("initialize returned no server identity: {err}"),
            })?;
        let name = init.server_info.name.as_deref();
        let version = init.server_info.version.as_deref();
        if name != Some("deepseek-harness-sdk-runtime") || version.is_none() {
            return Err(Error::SdkProtocol {
                message: format!(
                    "initialize returned unexpected server identity: name={name:?}, version={version:?}"
                ),
            });
        }
        Ok(init)
    }

    /// Queue one prompt on a session and return its durable inbox message id.
    ///
    /// A `session_id` unknown to the runtime lazily creates the agent+session
    /// pair. A response without a string `messageId` is a protocol error.
    pub async fn session_prompt(
        &mut self,
        session_id: impl Into<String>,
        blocks: Vec<ContentBlock>,
    ) -> Result<String, Error> {
        let params = SessionPromptParams {
            session_id: session_id.into(),
            content_blocks: blocks,
        };
        let result = self
            .request("session/prompt", Some(serde_json::to_value(params)?))
            .await?;
        let prompt: SessionPromptResult =
            serde_json::from_value(result).map_err(|err| Error::SdkProtocol {
                message: format!("session/prompt returned no message id: {err}"),
            })?;
        Ok(prompt.message_id)
    }

    /// Subscribe to the notifications of one session and its descendants.
    ///
    /// Descendants are discovered from the client-side `subagent.started`
    /// parent→child edge map: `subagent.started`/`subagent.finished`
    /// notifications pass when their parent session is already in the tree
    /// (or their child session is the root), and session-scoped notifications
    /// pass when their `sessionId` is the root or a discovered descendant.
    /// The filter consults the live edge map, so a child started after the
    /// subscription is matched from its first event onward.
    ///
    /// A subscription created after close/runtime death is born-failed.
    pub fn subscribe_session_tree(&self, root: &str) -> NotificationSubscription {
        NotificationSubscription {
            receiver: self
                .notifications
                .as_ref()
                .map(broadcast::Sender::subscribe),
            parent_map: Arc::clone(&self.parent_map),
            state: Arc::clone(&self.state),
            root: root.to_string(),
        }
    }

    /// Shut the runtime down and reap it, resolving only after it exited.
    ///
    /// The close ladder per [`ClientTimeouts`]: a cooperative `shutdown`
    /// request bounded by `shutdown_timeout` (diagnostic only on failure) →
    /// drop stdin (EOF) → wait `eof_grace` → SIGTERM → wait `term_grace` →
    /// SIGKILL → wait. Pending requests are resolved with
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

/// Drain the pending map into transport-closed errors (used by the read loop
/// when stdout closes).
fn fail_all_pending(pending: &PendingMap, state: &Mutex<SharedState>, reason: &str) {
    let senders: Vec<_> = {
        let mut pending = lock(pending);
        pending.drain().map(|(_id, tx)| tx).collect()
    };
    for tx in senders {
        let _ = tx.send(Err(closed_error(&lock(state), reason)));
    }
}

/// The read loop's shared context: every client handle it needs to dispatch
/// frames and to resolve the pending map (and its own state) on EOF.
struct ReadContext {
    /// Weak stdin handle — the read loop answers client-directed requests;
    /// the client's owned handle keeps the pipe open.
    stdin: Weak<tokio::sync::Mutex<ChildStdin>>,
    /// Weak child handle — polled once on EOF for the exit code.
    child: Weak<tokio::sync::Mutex<Child>>,
    /// In-flight requests by request id.
    pending: PendingRequests,
    /// The client-side session-tree edge map.
    parent_map: Arc<Mutex<ParentMap>>,
    /// Notification producer fanned out to subscriptions.
    notifications: broadcast::Sender<Notification>,
    /// Shared client state (exit code, closed flag, stderr tail).
    state: Arc<Mutex<SharedState>>,
    /// Signalled when the captured stderr stream has fully drained.
    stderr_done: Arc<Notify>,
}

/// The read loop: owns stdout, dispatches frames, and terminates the pending
/// map when the transport closes.
async fn read_loop(
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
async fn stderr_loop(
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

/// The client-side `subagent.started` parent→child session edge map, bounded
/// to [`MAX_PARENT_EDGES`] entries with drop-oldest eviction.
///
/// Each entry maps a child session id to its parent; the insertion order is
/// tracked so the oldest edges are evicted first once the cap is reached.
#[derive(Debug, Default)]
struct ParentMap {
    /// child session id -> parent session id.
    edges: HashMap<String, String>,
    /// Child ids in insertion order, for drop-oldest eviction.
    order: VecDeque<String>,
}

impl ParentMap {
    fn new() -> Self {
        Self::default()
    }

    fn get(&self, child: &str) -> Option<&String> {
        self.edges.get(child)
    }

    /// Record (or update) a parent→child edge, evicting the oldest edges
    /// once the map exceeds [`MAX_PARENT_EDGES`].
    fn insert(&mut self, child: String, parent: String) {
        if !self.edges.contains_key(&child) {
            self.order.push_back(child.clone());
        }
        self.edges.insert(child, parent);
        while self.order.len() > MAX_PARENT_EDGES {
            let oldest = self.order.pop_front().expect("order mirrors edges");
            self.edges.remove(&oldest);
        }
    }
}

/// Record a parent→child session edge when `notification` is a well-formed
/// `subagent.started` (both ids non-empty strings, parent != child).
///
/// Called by the read loop **before** fan-out so the tree filter sees fresh
/// edges; reference parity with the Python and TypeScript clients.
fn record_session_relationship(map: &mut ParentMap, notification: &Notification) {
    if notification.method != "subagent.started" {
        return;
    }
    let Some(parent) = notification
        .payload
        .get("parentSessionId")
        .and_then(Value::as_str)
        .filter(|parent| !parent.is_empty())
    else {
        return;
    };
    let Some(child) = notification
        .payload
        .get("childSessionId")
        .and_then(Value::as_str)
        .filter(|child| !child.is_empty() && *child != parent)
    else {
        return;
    };
    map.insert(child.to_string(), parent.to_string());
}

/// Whether `session` is `root` itself or reachable by walking parent edges.
///
/// The walk is cycle-guarded (the edge map only ever extends chains upward,
/// so a cycle cannot form; the guard is defensive).
fn is_descendant_of(map: &ParentMap, session: &str, root: &str) -> bool {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut current = session;
    loop {
        if current == root {
            return true;
        }
        if !visited.insert(current) {
            return false;
        }
        match map.get(current) {
            Some(parent) => current = parent.as_str(),
            None => return false,
        }
    }
}

/// Whether a notification belongs to the session tree rooted at `root`.
///
/// `subagent.started`/`subagent.finished` pass when the parent session is
/// already in the tree, or when the child session is the root itself; other
/// notifications pass when their `sessionId` is in the tree.
fn notification_in_tree(map: &ParentMap, notification: &Notification, root: &str) -> bool {
    if matches!(
        notification.method.as_str(),
        "subagent.started" | "subagent.finished"
    ) {
        if let Some(parent) = notification
            .payload
            .get("parentSessionId")
            .and_then(Value::as_str)
        {
            if is_descendant_of(map, parent, root) {
                return true;
            }
        }
        return notification
            .payload
            .get("childSessionId")
            .and_then(Value::as_str)
            == Some(root);
    }
    match notification
        .payload
        .get("sessionId")
        .and_then(Value::as_str)
    {
        Some(session) => is_descendant_of(map, session, root),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn notification(method: &str, fields: &[(&str, Value)]) -> Notification {
        let mut payload = Map::new();
        for (key, value) in fields {
            payload.insert((*key).to_string(), value.clone());
        }
        Notification {
            method: method.to_string(),
            payload,
        }
    }

    fn started(parent: &str, child: &str) -> Notification {
        notification(
            "subagent.started",
            &[
                ("parentSessionId", json!(parent)),
                ("childSessionId", json!(child)),
            ],
        )
    }

    fn event(session: &str) -> Notification {
        notification(
            "session.event",
            &[
                ("sessionId", json!(session)),
                ("event", json!({ "type": "assistant/message" })),
            ],
        )
    }

    #[test]
    fn parent_map_records_valid_started_edges_and_ignores_invalid() {
        let mut map = ParentMap::new();
        record_session_relationship(&mut map, &started("root", "child1"));
        record_session_relationship(&mut map, &started("child1", "child2"));
        // Non-`subagent.started` notifications never record edges.
        record_session_relationship(&mut map, &event("root"));
        record_session_relationship(
            &mut map,
            &notification(
                "subagent.finished",
                &[
                    ("parentSessionId", json!("x")),
                    ("childSessionId", json!("y")),
                ],
            ),
        );
        // Degenerate edges are ignored (reference parity).
        record_session_relationship(&mut map, &started("root", ""));
        record_session_relationship(&mut map, &started("", "child"));
        record_session_relationship(&mut map, &started("same", "same"));
        record_session_relationship(
            &mut map,
            &notification(
                "subagent.started",
                &[
                    ("parentSessionId", json!(1)),
                    ("childSessionId", json!("child")),
                ],
            ),
        );

        assert_eq!(map.edges.len(), 2);
        assert_eq!(map.get("child1").map(String::as_str), Some("root"));
        assert_eq!(map.get("child2").map(String::as_str), Some("child1"));
    }

    #[test]
    fn parent_map_evicts_oldest_edges_past_the_cap() {
        // FIX-3: the edge map is bounded; once MAX_PARENT_EDGES is reached
        // the oldest edges are evicted (drop-oldest), so a long-lived client
        // cannot grow the tree without bound.
        let mut map = ParentMap::new();
        for i in 0..MAX_PARENT_EDGES + 50 {
            map.insert(format!("child-{i}"), format!("parent-{i}"));
        }
        assert_eq!(
            map.edges.len(),
            MAX_PARENT_EDGES,
            "the map must never exceed the cap"
        );
        assert!(
            map.get("child-0").is_none(),
            "the oldest edges must be evicted first"
        );
        assert_eq!(
            map.get("child-50").map(String::as_str),
            Some("parent-50"),
            "the first 50 edges (child-0..child-49) are gone; child-50 is the oldest survivor"
        );
        assert_eq!(
            map.get(&format!("child-{}", MAX_PARENT_EDGES + 49))
                .map(String::as_str),
            Some("parent-100049"),
            "the newest edge must be retained"
        );

        // Re-inserting a known child updates its parent without duplicating
        // the eviction order.
        let mut single = ParentMap::new();
        single.insert("child".into(), "parent-1".into());
        single.insert("child".into(), "parent-2".into());
        assert_eq!(single.edges.len(), 1);
        assert_eq!(single.get("child").map(String::as_str), Some("parent-2"));
    }

    #[test]
    fn descendant_check_walks_edges_and_guards_cycles() {
        let mut map = ParentMap::new();
        record_session_relationship(&mut map, &started("root", "child1"));
        record_session_relationship(&mut map, &started("child1", "child2"));
        record_session_relationship(&mut map, &started("other", "child3"));

        assert!(is_descendant_of(&map, "root", "root")); // the root itself
        assert!(is_descendant_of(&map, "child1", "root"));
        assert!(is_descendant_of(&map, "child2", "root"));
        assert!(!is_descendant_of(&map, "child3", "root"));
        assert!(!is_descendant_of(&map, "child3", "child1"));
        assert!(is_descendant_of(&map, "child3", "other"));

        // A cycle (impossible via the record rule, but the walk must not hang).
        map.insert("a".into(), "b".into());
        map.insert("b".into(), "a".into());
        assert!(!is_descendant_of(&map, "a", "root"));
    }

    #[test]
    fn tree_filter_membership_follows_sequence() {
        // The sequence a client would observe: root starts child1, which
        // starts child2; "other" and "sub" belong to a different tree.
        let mut map = ParentMap::new();
        for edge in [
            started("root", "child1"),
            started("child1", "child2"),
            started("other", "sub"),
        ] {
            record_session_relationship(&mut map, &edge);
        }

        assert!(notification_in_tree(&map, &event("root"), "root"));
        assert!(notification_in_tree(&map, &event("child1"), "root"));
        assert!(notification_in_tree(&map, &event("child2"), "root"));
        assert!(!notification_in_tree(&map, &event("other"), "root"));
        assert!(!notification_in_tree(&map, &event("unrelated"), "root"));

        // Lifecycle edges pass when the *parent* is in the tree...
        assert!(notification_in_tree(
            &map,
            &started("child1", "child3"),
            "root"
        ));
        assert!(!notification_in_tree(
            &map,
            &started("other", "child3"),
            "root"
        ));
        assert!(notification_in_tree(
            &map,
            &notification(
                "subagent.finished",
                &[
                    ("parentSessionId", json!("child2")),
                    ("childSessionId", json!("grandchild")),
                ],
            ),
            "root"
        ));
        // ...or when the child session is the root itself.
        assert!(notification_in_tree(
            &map,
            &notification(
                "subagent.finished",
                &[
                    ("parentSessionId", json!("unrelated")),
                    ("childSessionId", json!("root")),
                ],
            ),
            "root"
        ));

        // Notifications without a session identity never match.
        assert!(!notification_in_tree(
            &map,
            &notification("session.status", &[]),
            "root"
        ));
        assert!(!notification_in_tree(
            &map,
            &notification("subagent.started", &[("parentSessionId", json!(7))],),
            "root"
        ));
    }

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
