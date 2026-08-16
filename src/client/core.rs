//! The spawn/request surface: [`HarnessClient`], [`LaunchSpec`],
//! [`ClientTimeouts`], and the public request helpers.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::Error;
use crate::protocol::{
    ContentBlock, InitializeParams, InitializeResult, Notification, SessionPromptParams,
    SessionPromptResult,
};
use crate::transport::{write_frame, JsonRpcLineTransport};

use super::read_loop::{read_loop, stderr_loop, ReadContext};
use super::session_tree::ParentMap;
use super::subscription::NotificationSubscription;
use super::{closed_error, lock, PendingRequests, SharedState, DEFAULT_BROADCAST_CAPACITY};

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
    pub(super) child: Option<Arc<tokio::sync::Mutex<Child>>>,
    /// Shared stdin write half. The read loop holds only a [`Weak`] reference
    /// so dropping this (stdin EOF) actually closes the runtime's stdin.
    pub(super) stdin: Option<Arc<tokio::sync::Mutex<ChildStdin>>>,
    /// In-flight requests by request id (uuid-v4 string).
    pub(super) pending: PendingRequests,
    /// `subagent.started` parent→child session edges (client-side tree).
    pub(super) parent_map: Arc<Mutex<ParentMap>>,
    /// Shared client state (exit code, closed flag, stderr tail).
    pub(super) state: Arc<Mutex<SharedState>>,
    /// Notification producer; `None` after close (subscriptions then drain
    /// their queues and see the channel close).
    pub(super) notifications: Option<broadcast::Sender<Notification>>,
    /// The stdout read-loop task, joined by [`HarnessClient::close`].
    pub(super) read_task: Option<JoinHandle<()>>,
    /// The stderr-capture task.
    pub(super) stderr_task: Option<JoinHandle<()>>,
    /// The configured timeout ladder.
    pub(super) timeouts: ClientTimeouts,
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
            lagged: false,
        }
    }
}
