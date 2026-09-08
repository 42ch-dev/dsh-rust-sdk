//! The spawn/request surface: [`HarnessClient`], [`LaunchSpec`],
//! [`ClientTimeouts`], and the public request helpers.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::{Error, SelectedProfile, TimeoutSource};
use crate::protocol::{
    ContentBlock, InitializeParams, InitializeResult, Notification, SessionPromptParams,
    SessionPromptResult,
};
use crate::transport::{write_frame, JsonRpcLineTransport};

use super::read_loop::{read_loop, stderr_loop, ReadContext};
use super::session_tree::ParentMap;
use super::subscription::NotificationSubscription;
use super::{
    closed_error, lock, try_register_pending, PendingRequests, SharedState,
    DEFAULT_BROADCAST_CAPACITY,
};

/// The environment keys the crate MUST never write into the child
/// environment, under any configuration (spec §4.2). None has a reader
/// upstream: the bundled `cordis.yml` consuming `DSH_CORDIS_CONFIG` was
/// deleted, sessions live under `$DSH_HOME/sessions`, and the workspace cwd
/// reaches the runtime through `initialize.cwd` (spec §4.2 evidence).
///
/// Enforced at the spawn layer — the child inherits the parent environment
/// wholesale, so the keys are stripped from the inherited env here — and in
/// the compose filter (`crate::runtime::compose_env_with_home`).
pub(crate) const FORBIDDEN_ENV_KEYS: [&str; 3] =
    ["DSH_CORDIS_CONFIG", "DSH_SESSION_ROOT", "DSH_CWD"];

/// How to launch the runtime process (the official
/// `deepseek-harness-sdk-runtime` binary).
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Path to (or name of) the runtime executable.
    pub program: PathBuf,
    /// Extra command-line arguments passed to the runtime.
    pub args: Vec<OsString>,
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
    /// Bound for the `initialize` handshake request only (spec §6.4).
    /// `None` waits indefinitely. The activity interval and
    /// `session/prompt` keep using [`ClientTimeouts::request_timeout`]
    /// (Python parity: `initialize_timeout_seconds` vs
    /// `request_timeout_seconds`). The high-level
    /// [`Config::initialize_timeout`](crate::runtime::Config::initialize_timeout)
    /// carries the Python-parity 30 s default and is copied here by
    /// [`DeepSeekHarness::start`](crate::api::DeepSeekHarness::start).
    pub initialize_timeout: Option<Duration>,
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
            initialize_timeout: None,
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
        // Spec §4.2: the three forbidden keys MUST NOT reach the child under
        // any configuration. The override set filters them from
        // `Config::env`, but the child also inherits the parent environment
        // wholesale — strip them here so a parent-exported
        // `DSH_CORDIS_CONFIG` / `DSH_SESSION_ROOT` / `DSH_CWD` can never
        // leak into the runtime (AC1). The rest of the parent env is
        // inherited untouched (no `env_clear`).
        for key in FORBIDDEN_ENV_KEYS {
            command.env_remove(key);
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        // A spawn failure — including ENOENT for a configured-but-missing
        // program — is an I/O error (spec §7). `Error::RuntimeNotFound` is
        // reserved for "no runtime could be resolved" (spec §8), which
        // `resolve_runtime` reports before spawn.
        let mut child = match command.spawn() {
            Ok(child) => child,
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
        self.request_with_timeout(
            method,
            params,
            self.timeouts.request_timeout,
            SelectedProfile::default(),
        )
        .await
    }

    /// [`HarnessClient::request`] with an explicit response deadline and
    /// timeout-diagnostic profile, so the `initialize` handshake can apply
    /// its own bound ([`ClientTimeouts::initialize_timeout`], spec §6.4)
    /// while every other request keeps the generic
    /// [`ClientTimeouts::request_timeout`].
    async fn request_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
        profile: SelectedProfile,
    ) -> Result<Value, Error> {
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
        // dropped as an unknown id (reference parity). The insert is paired
        // with a closed-flag re-check under the state lock (same lock order
        // as the read loop's EOF drain), so a runtime death between the
        // fast-fail check above and this insert cannot strand the request:
        // either the insert lands before the drain (covered by it) or the
        // drain ran first and the check here fails fast.
        if let Some(err) = try_register_pending(&self.pending, &self.state, id.clone(), tx) {
            return Err(err);
        }

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

        let outcome = match timeout {
            Some(duration) => match tokio::time::timeout(duration, rx).await {
                Ok(result) => result,
                Err(elapsed) => {
                    // Timeout abandonment: remove the pending entry so a late
                    // response is dropped; the server-side work continues.
                    lock(&self.pending).remove(&id);
                    return Err(Error::RequestTimeout {
                        // The method stays the exact wire method name (spec
                        // §7); the selected profile rides in the source
                        // carrier's message, or nothing when the timeout
                        // path has no profile context.
                        method: method.to_string(),
                        source: TimeoutSource::new(elapsed, profile),
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
    ///
    /// `reasoning_effort` is sent as the wire key `reasoningEffort`;
    /// `None`, empty, and whitespace-only values are dropped by the wire
    /// type itself ([`InitializeParams::reasoning_effort`], spec §6.3), so
    /// this low-level path can never send a blank value. The high-level
    /// path ([`DeepSeekHarness::start`](crate::api::DeepSeekHarness::start))
    /// additionally normalizes
    /// [`Config::reasoning_effort`](crate::runtime::Config::reasoning_effort)
    /// through `Config::reasoning_effort_for_wire`, which drops empty and
    /// whitespace-only values before they reach this call.
    ///
    /// The handshake is bounded by [`ClientTimeouts::initialize_timeout`]
    /// (spec §6.4) — the bound applies to the handshake only, never to
    /// `session/prompt` or the activity interval, which keep using
    /// [`ClientTimeouts::request_timeout`]. On expiry the error is
    /// [`Error::RequestTimeout`]; the high-level
    /// [`DeepSeekHarness::start`](crate::api::DeepSeekHarness::start) path
    /// names the selected profile in the message (spec §7).
    pub async fn initialize(
        &mut self,
        cwd: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<&str>,
        max_tokens: Option<u32>,
    ) -> Result<InitializeResult, Error> {
        self.initialize_with_profile(
            cwd,
            provider,
            model,
            reasoning_effort,
            max_tokens,
            SelectedProfile::default(),
        )
        .await
    }

    /// [`HarnessClient::initialize`] with the selected profile threaded to
    /// the timeout diagnostic (spec §7).
    ///
    /// `pub(crate)` so the high-level
    /// [`DeepSeekHarness::start`](crate::api::DeepSeekHarness::start) path
    /// names the profile in the timeout message while the public low-level
    /// signature stays unchanged.
    pub(crate) async fn initialize_with_profile(
        &mut self,
        cwd: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<&str>,
        max_tokens: Option<u32>,
        profile: SelectedProfile,
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
            reasoning_effort: reasoning_effort.map(str::to_string),
            max_tokens,
        };
        let result = self
            .request_with_timeout(
                "initialize",
                Some(serde_json::to_value(params)?),
                self.timeouts.initialize_timeout,
                profile,
            )
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
