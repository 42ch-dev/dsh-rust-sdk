//! High-level Python-parity API: [`DeepSeekHarness`], [`Session`], and
//! [`RunResult`].
//!
//! This module layers the Python SDK's `Session::run` activity-interval
//! algorithm on the low-level
//! [`HarnessClient`]: subscribe to the session
//! tree, send `session/prompt`, wait for the durable `agent/inbox/spliced`
//! receipt of the returned message id, collect every tree notification until
//! the **root** session reports `idle`, then derive
//! [`RunResult::final_response`] and [`RunResult::finish_reason`] exactly as
//! the Python SDK does.
//!
//! [`RunResult`] mirrors the **Python** SDK's five fields exactly
//! (`session_id`, `final_response`, `finish_reason`, `events`,
//! `notifications`; upstream `python/sdk/src/deepseek_harness/api.py:40-46`);
//! the TypeScript SDK's `RunResult` lacks `finish_reason`, and Rust
//! intentionally does not claim TypeScript surface parity.
//!
//! The runtime binary is bring-your-own (Plan A): [`DeepSeekHarness::start`]
//! resolves it from `Config::dsh_bin` or the `DSH_RUNTIME_BIN` environment
//! variable and boots it under the configured `profile` (`dsh --profile
//! <name> [--patch <path>]...`). This crate never downloads or bundles a
//! runtime. The official runtime and its sources live at
//! <https://github.com/deepseek-ai/deepseek-harness>.

use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use crate::client::{
    HarnessClient, LaunchSpec, NotificationSubscription, DEFAULT_BROADCAST_CAPACITY,
};
use crate::error::Error;
use crate::protocol::{ContentBlock, Notification};
use crate::runtime::{
    compose_env_with_home, env_var_non_empty, resolve_dsh_home_with, resolve_runtime, Config,
};

/// A running DeepSeek Harness instance, Python `DeepSeekHarness` parity.
///
/// Owns the spawned runtime child (via the low-level
/// [`HarnessClient`]) behind an async mutex:
/// sessions created by [`DeepSeekHarness::start_session`] may run
/// concurrently, interleaving at the `session/prompt` write and then waiting
/// on their own subscriptions.
#[derive(Debug)]
pub struct DeepSeekHarness {
    client: tokio::sync::Mutex<HarnessClient>,
    /// The resolved absolute harness home this instance was launched with
    /// (spec §3.2.4 observability; the value created at boot and injected
    /// into the child as `DSH_HOME`).
    dsh_home: PathBuf,
}

impl DeepSeekHarness {
    /// Resolve the runtime, compose the env injection set, spawn the
    /// subprocess, and perform the `initialize` handshake.
    ///
    /// `Config::cwd` is resolved absolute (Python `Path(cwd).resolve()`) and
    /// feeds `initialize.cwd`; a nonexistent cwd fails with [`Error::Io`].
    /// The runtime subprocess cwd defaults to the same resolved cwd
    /// (`Config::runtime_cwd` overrides it — Python parity).
    ///
    /// The child env carries the resolved `DSH_HOME`, the caller's
    /// `Config::env` entries, and `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY`
    /// when configured (spec §4). The resolved harness home is created when
    /// absent so a fresh home boots (spec §3.2.5); the selection is
    /// observable through [`Config::resolve_dsh_home`] (spec §3.2.4) and
    /// the instance accessor [`DeepSeekHarness::dsh_home`].
    ///
    /// [`Config::initialize_timeout`] bounds the `initialize` handshake
    /// only (spec §6.4); [`Config::request_timeout`] bounds every other
    /// request, including `session/prompt`; `None` (the default) waits
    /// indefinitely.
    ///
    /// On `initialize` failure the close ladder is run before the error
    /// propagates, so the spawned child is never leaked (Python parity).
    pub async fn start(config: Config) -> Result<Self, Error> {
        let launch = resolve_runtime(&config)?;
        // Resolve the harness home once and create it when absent so a
        // fresh home boots (spec §3.2.5); the resolved value is observable
        // through `Config::resolve_dsh_home` (spec §3.2.4) and the instance
        // accessor [`DeepSeekHarness::dsh_home`]. The same value is passed
        // into the compose path, so the created/logged home and the
        // injected child `DSH_HOME` are identical (spec §3.2.3; F7).
        let dsh_home = resolve_dsh_home_with(&config, &env_var_non_empty);
        std::fs::create_dir_all(&dsh_home).map_err(Error::Io)?;
        tracing::info!(dsh_home = %dsh_home.display(), "resolved DSH_HOME");
        let cwd = match &config.cwd {
            Some(path) => path.canonicalize().map_err(Error::Io)?,
            None => std::env::current_dir().map_err(Error::Io)?,
        };
        let spec = LaunchSpec {
            program: launch.program,
            args: launch.args,
            envs: compose_env_with_home(&config, &dsh_home)?
                .into_iter()
                .collect(),
            cwd: Some(config.runtime_cwd.clone().unwrap_or_else(|| cwd.clone())),
        };
        // `Config::initialize_timeout` bounds the handshake only (spec
        // §6.4); `Config::request_timeout` is the Python-parity request
        // deadline (`None` = wait indefinitely); `Config::timeouts`
        // supplies the close-ladder timings.
        let mut timeouts = config.timeouts;
        timeouts.request_timeout = config.request_timeout;
        timeouts.initialize_timeout = config.initialize_timeout;
        let mut client = HarnessClient::spawn(spec, timeouts)?;
        if let Err(err) = client
            .initialize_with_profile(
                cwd.to_string_lossy().into_owned(),
                &config.provider,
                &config.model,
                config.reasoning_effort_for_wire(),
                config.max_tokens,
                Some(config.profile.clone()),
            )
            .await
        {
            // Python parity: no leaked child. The close ladder is
            // unconditional teardown, so the child is always reaped even
            // when a ladder tier reports a diagnostic error.
            if let Err(close_err) = client.close().await {
                tracing::debug!(
                    error = %close_err,
                    "close after a failed initialize reported a ladder error; \
                     the child is still reaped"
                );
            }
            return Err(err);
        }
        Ok(Self {
            client: tokio::sync::Mutex::new(client),
            dsh_home,
        })
    }

    /// The resolved absolute harness home this instance was launched with
    /// (spec §3.2.4 observability; plan Task 2 accessor). The value is the
    /// home resolved at boot — `Config::dsh_home` → non-empty `DSH_HOME`
    /// (from `Config::env`, then the parent environment) → `~/.dsh`,
    /// normalized absolute with `~` expanded (upstream `resolveDshHome`,
    /// `packages/util/home-paths/src/index.ts:87-91`) — created when
    /// absent and injected into the child as `DSH_HOME` (spec §3.2.3,
    /// §3.2.5).
    pub fn dsh_home(&self) -> &Path {
        &self.dsh_home
    }

    /// Shut the runtime down and reap it (the plan 01 close ladder).
    pub async fn close(&mut self) -> Result<(), Error> {
        self.client.lock().await.close().await
    }

    /// Create a session bound to this harness.
    ///
    /// The default session id is `session-<hex>` — Python parity
    /// (`"session-{uuid4().hex}"`), not a bare uuid.
    pub fn start_session(&self, id: Option<String>) -> Session<'_> {
        let session_id = id.unwrap_or_else(|| format!("session-{}", Uuid::new_v4().simple()));
        Session {
            harness: self,
            session_id,
        }
    }
}

/// One SDK session bound to a [`DeepSeekHarness`], Python `Session` parity.
///
/// A cheap handle; a session id unknown to the runtime lazily creates the
/// agent+session pair on the first [`Session::run`].
#[derive(Debug)]
pub struct Session<'a> {
    harness: &'a DeepSeekHarness,
    session_id: String,
}

impl Session<'_> {
    /// Run one user turn and wait for the agent to go idle.
    ///
    /// Python `Session.run` verbatim:
    ///
    /// 1. Subscribe to the session tree **before** writing the prompt, so no
    ///    notification for this turn can be missed.
    /// 2. Send `session/prompt` (bounded by [`Config::request_timeout`]).
    /// 3. Wait for the durable `agent/inbox/spliced` receipt whose
    ///    `inserted[].id` equals the returned message id (the field is `id`,
    ///    **not** `messageId`); notifications before the receipt are dropped
    ///    from both `events` and `notifications`.
    /// 4. Collect — from the receipt **inclusive** — every tree notification
    ///    until the **root** session reports `session.status == "idle"`
    ///    (that idle notification is collected too; a non-root idle never
    ///    terminates the run).
    ///
    /// `events` holds root-session `session.event` payloads only;
    /// `notifications` holds every tree notification (root + discovered
    /// descendants, incl. `session.status` / `subagent.*`) in transport
    /// order.
    ///
    /// The wait-for-receipt interval (Phase 1) and the wait-for-idle interval
    /// (Phase 2) are **both unbounded** (Python parity): only the
    /// `session/prompt` request is bounded by [`Config::request_timeout`].
    /// Callers needing a bound should wrap this call in
    /// `tokio::time::timeout`.
    ///
    /// # Bounded notification buffer
    ///
    /// Tree notifications travel a broadcast channel capped at
    /// `DEFAULT_BROADCAST_CAPACITY` (4096) with documented drop-oldest
    /// behavior. If a high-volume tree floods more notifications than fit
    /// between this call's reads, the dropped set can include the inbox
    /// receipt or the root-idle notification this run depends on; rather
    /// than hang forever or return a silently truncated result, the run then
    /// fails fast with [`Error::SdkProtocol`]. A caller expecting very large
    /// bursts can bypass this cap only by using the low-level
    /// [`HarnessClient::spawn_with_broadcast_capacity`] instead of
    /// [`DeepSeekHarness::start`].
    ///
    /// # Malformed payloads
    ///
    /// A `session.event` / `session.status` notification whose payload does
    /// not match the wire shape fails the run with [`Error::SdkProtocol`]
    /// (the payload is logged). The Python SDK raises when it touches a
    /// malformed notification; Rust surfaces the same condition as a typed
    /// error instead of silently dropping an event or misreading the idle
    /// termination.
    ///
    /// # Per-notification callback
    ///
    /// `on_notification` observes **every notification delivered to this
    /// run's session-tree subscription, in wire order** — including
    /// `session.event`, `session.status`, and `subagent.*` — and is invoked
    /// as each notification arrives, not deferred until the run ends. It is
    /// a layer over the existing subscription path, never a second
    /// subscription: the run already owns one tree subscription, and the
    /// callback neither filters nor consumes. Passing `None` behaves
    /// exactly like the no-callback path, and the returned [`RunResult`] is
    /// identical with or without a callback (spec §6.5; upstream
    /// `packages/sdk/client/src/api.ts:150-156,186-200`,
    /// `python/sdk/src/deepseek_harness/api.py:124-131,139-144`).
    ///
    /// The callback is invoked **before** the run's lag gate: a lagged
    /// `recv()` can still return a retained notification, and that
    /// delivered notification is observed even though the run then fails
    /// fast with the lag error instead of trusting a truncated stream.
    ///
    /// The bound is `Fn(&Notification) + Send + Sync`, not `FnMut`: a
    /// caller needing shared mutable state captures an `Arc<Mutex<..>>` by
    /// interior mutability, and [`Session::run`] keeps taking `&self` — the
    /// callback never requires giving up ownership of the session. A panic
    /// in the callback is a caller bug and propagates; the crate does not
    /// swallow it (spec §6.5).
    pub async fn run(
        &self,
        input: Input,
        on_notification: Option<&(dyn Fn(&Notification) + Send + Sync)>,
    ) -> Result<RunResult, Error> {
        let content_blocks = match input {
            Input::Text(text) => vec![ContentBlock::Text { text }],
            Input::Blocks(blocks) => blocks,
        };
        let root = &self.session_id;

        // Python parity: the tree subscription must exist before the request
        // is written, so the receipt (and every following notification) is
        // seen from the first broadcast.
        let mut client = self.harness.client.lock().await;
        let mut subscription = client.subscribe_session_tree(root);
        let message_id = client.session_prompt(root, content_blocks).await?;
        // The subscription owns its broadcast receiver; release the client so
        // concurrent sessions on the same harness can interleave while we
        // wait for the receipt and the root idle.
        drop(client);

        // Phase 1 — the durable inbox receipt of this exact message.
        // Notifications before it are dropped from both `events` and
        // `notifications` (Python parity), but the callback still observes
        // them: it sees every notification the subscription delivers, in
        // wire order (spec §6.5).
        let receipt = loop {
            let notification = subscription.recv().await?;
            // The callback runs before the lag gate: a lagged `recv()` can
            // still return a retained notification, and that delivered
            // notification must be observed even though the run then fails
            // fast on the truncated stream (spec §6.5 rule 1).
            if let Some(callback) = on_notification {
                callback(&notification);
            }
            ensure_no_lag(&mut subscription)?;
            let is_receipt = match notification.session_event() {
                Some(Ok(event)) => {
                    event.session_id == *root && is_inbox_receipt(&event.event, &message_id)
                }
                Some(Err(err)) => {
                    // The notification IS a session.event but its payload is
                    // malformed. It could be the receipt itself — a silent
                    // skip would hang the run forever (the receipt never
                    // matches) — so fail visibly with the payload logged
                    // (Python raises on malformed notifications).
                    tracing::warn!(
                        error = %err,
                        method = %notification.method,
                        payload = ?notification.payload,
                        "malformed session.event during the receipt wait; \
                         Python raises on malformed notifications, Rust fails \
                         with SdkProtocol"
                    );
                    return Err(Error::SdkProtocol {
                        message: format!(
                            "malformed session.event during Session::run (the \
                             inbox receipt could not be confirmed): {err}"
                        ),
                    });
                }
                None => false, // not a session.event (status / subagent.*)
            };
            if is_receipt {
                break notification;
            }
        };

        // Phase 2 — collect from the receipt inclusive until the ROOT
        // session goes idle (that idle notification is collected too, then
        // stop). A non-root idle never terminates the run.
        let mut events = Vec::new();
        let mut notifications = Vec::new();
        let mut notification = receipt;
        loop {
            match notification.session_event() {
                Some(Ok(event)) => {
                    if event.session_id == *root && event.event.is_object() {
                        events.push(event.event);
                    }
                }
                Some(Err(err)) => {
                    // A malformed session.event cannot be classified as root
                    // (or child); it would silently vanish from `events`
                    // while still present in `notifications`. Fail visibly
                    // (Python raises on malformed notifications).
                    tracing::warn!(
                        error = %err,
                        method = %notification.method,
                        payload = ?notification.payload,
                        "malformed session.event during the collection phase; \
                         Python raises on malformed notifications, Rust fails \
                         with SdkProtocol"
                    );
                    return Err(Error::SdkProtocol {
                        message: format!(
                            "malformed session.event during Session::run (the \
                             event could not be collected): {err}"
                        ),
                    });
                }
                None => {}
            }
            let root_idle = match notification.session_status() {
                Some(Ok(status)) => status.session_id == *root && status.status == "idle",
                Some(Err(err)) => {
                    // A malformed session.status could be the root idle
                    // notification — a silent skip would hang the run
                    // forever. Fail visibly.
                    tracing::warn!(
                        error = %err,
                        method = %notification.method,
                        payload = ?notification.payload,
                        "malformed session.status during Session::run; Python \
                         raises on malformed notifications, Rust fails with \
                         SdkProtocol"
                    );
                    return Err(Error::SdkProtocol {
                        message: format!(
                            "malformed session.status during Session::run (the \
                             root idle state could not be determined): {err}"
                        ),
                    });
                }
                None => false,
            };
            notifications.push(notification);
            if root_idle {
                break;
            }
            notification = subscription.recv().await?;
            // Same ordering as the receipt wait: the callback observes the
            // delivered notification before the lag gate can fail the run.
            if let Some(callback) = on_notification {
                callback(&notification);
            }
            ensure_no_lag(&mut subscription)?;
        }

        let finish_reason = extract_finish_reason(&events)?;
        let final_response = derive_final_response(&events);
        Ok(RunResult {
            session_id: self.session_id.clone(),
            final_response,
            finish_reason,
            events,
            notifications,
        })
    }
}

/// Fail the run when the notification subscription has fallen behind the
/// broadcast capacity: dropped notifications are irrecoverable, and the
/// dropped set can include the inbox receipt or the root-idle notification,
/// so the run cannot be trusted (and might otherwise hang forever).
fn ensure_no_lag(subscription: &mut NotificationSubscription) -> Result<(), Error> {
    if subscription.take_lagged() {
        return Err(Error::SdkProtocol {
            message: format!(
                "the notification subscription fell behind the \
                 {DEFAULT_BROADCAST_CAPACITY}-notification broadcast buffer and \
                 dropped notifications; the inbox receipt or root-idle \
                 notification may have been lost, so this run's result cannot \
                 be trusted"
            ),
        });
    }
    Ok(())
}

/// A user turn for [`Session::run`], mirroring Python's `normalize_input`.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Plain text; normalized to a single `text` content block.
    Text(String),
    /// Raw content blocks, sent verbatim.
    Blocks(Vec<ContentBlock>),
}

/// The result of one [`Session::run`], field-for-field the **Python** SDK's
/// `RunResult` (upstream `python/sdk/src/deepseek_harness/api.py:40-46`):
/// exactly the five Python fields of spec §6.2 — the v0.1 session-root
/// path field is dropped, with no replacement (spec §5). The TypeScript
/// SDK's `RunResult` lacks `finish_reason`; Rust intentionally follows
/// Python.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    /// The SDK session id this turn ran on.
    pub session_id: String,
    /// Text concatenation of the **last** root `assistant/message` event's
    /// text blocks (`text: null` or a non-string `text` contributes `""`);
    /// `""` when the activity interval contains no `assistant/message` — or
    /// the last one has no text blocks. Never falls back to an earlier
    /// event (Python algorithm, `python/sdk/src/deepseek_harness/api.py:211-228`).
    pub final_response: String,
    /// The last root `turn/end` event's `data.reason.kind` inside the
    /// activity interval (`None` when the window has no `turn/end`; Python
    /// algorithm, `python/sdk/src/deepseek_harness/api.py:231-248`).
    pub finish_reason: Option<String>,
    /// Root-session `session.event` payloads only, in transport order.
    pub events: Vec<Value>,
    /// Every tree notification (root + discovered descendants, incl.
    /// `session.status` / `subagent.*`), in transport order.
    pub notifications: Vec<Notification>,
}

/// Extract the finish reason from a collected activity interval, Python
/// `finish_reason` verbatim: the **last** `turn/end` event's
/// `data.reason.kind` (reversed scan). No `turn/end` → `Ok(None)`. A
/// `turn/end` without a string `data.reason.kind` → [`Error::SdkProtocol`]
/// with the exact message `turn/end event requires a string data.reason.kind`.
///
/// Malformedness is checked only on the last `turn/end` — the reversed scan
/// stops there, so earlier events are never reached.
pub fn extract_finish_reason(events: &[Value]) -> Result<Option<String>, Error> {
    for event in events.iter().rev() {
        if event.get("type").and_then(Value::as_str) != Some("turn/end") {
            continue;
        }
        return match event.pointer("/data/reason/kind").and_then(Value::as_str) {
            Some(kind) => Ok(Some(kind.to_owned())),
            None => Err(Error::SdkProtocol {
                message: "turn/end event requires a string data.reason.kind".into(),
            }),
        };
    }
    Ok(None)
}

/// Python `final_response` verbatim: the last root `assistant/message`
/// event's text-block concatenation; `""` when absent or textless (never
/// falls back to an earlier event). Blocks with `type == "text"` contribute
/// their string `text`; `text: null` (or a non-string `text`) contributes
/// `""` (Python parity).
fn derive_final_response(events: &[Value]) -> String {
    let Some(last) = events
        .iter()
        .rev()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("assistant/message"))
    else {
        return String::new();
    };
    // Content lives at `data.message.content` when `data.message` is an
    // object, else at `data.content` (Python `isinstance` walk).
    let content = if last.pointer("/data/message").is_some_and(Value::is_object) {
        last.pointer("/data/message/content")
    } else {
        last.pointer("/data/content")
    };
    let Some(blocks) = content.and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .map(|block| block.get("text").and_then(Value::as_str).unwrap_or(""))
        .collect()
}

/// Python `_is_inbox_receipt` verbatim: an `agent/inbox/spliced` event whose
/// `data.inserted` list contains an object with `id` == `message_id` (the
/// field is `id`, **not** `messageId`; defensive pointer walk).
fn is_inbox_receipt(event: &Value, message_id: &str) -> bool {
    if event.get("type").and_then(Value::as_str) != Some("agent/inbox/spliced") {
        return false;
    }
    let Some(inserted) = event.pointer("/data/inserted").and_then(Value::as_array) else {
        return false;
    };
    inserted
        .iter()
        .any(|item| item.get("id").and_then(Value::as_str) == Some(message_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn_end_with_kind(kind: &str) -> Value {
        json!({"type": "turn/end", "data": {"reason": {"kind": kind}}})
    }

    fn malformed_turn_end() -> Value {
        json!({"type": "turn/end", "data": {"reason": {}}})
    }

    fn unrelated_event() -> Value {
        json!({"type": "assistant/message", "data": {"content": []}})
    }

    /// Compile-time spec §6.2 assertion: `RunResult` has exactly the five
    /// Python fields. Both the construction and the exhaustive destructure
    /// name every field — no `..`, no `_` — so a field added (including a
    /// resurrection of the v0.1 session-root path), renamed, or removed
    /// fails the build. This guards the field set only; it does not prove
    /// the absence of an accessor method.
    #[test]
    fn run_result_has_exactly_the_five_python_fields() {
        let result = RunResult {
            session_id: "session-1".to_string(),
            final_response: "hello".to_string(),
            finish_reason: Some("completed".to_string()),
            events: vec![],
            notifications: vec![],
        };
        let RunResult {
            session_id,
            final_response,
            finish_reason,
            events,
            notifications,
        } = result;
        assert_eq!(session_id, "session-1");
        assert_eq!(final_response, "hello");
        assert_eq!(finish_reason.as_deref(), Some("completed"));
        assert!(events.is_empty());
        assert!(notifications.is_empty());
    }

    #[test]
    fn extract_finish_reason_present_kind_returns_some() {
        let events = vec![unrelated_event(), turn_end_with_kind("completed")];
        assert_eq!(
            extract_finish_reason(&events).unwrap(),
            Some("completed".to_string())
        );
    }

    #[test]
    fn extract_finish_reason_without_turn_end_returns_none() {
        let events = vec![unrelated_event()];
        assert_eq!(extract_finish_reason(&events).unwrap(), None);
    }

    #[test]
    fn extract_finish_reason_malformed_turn_end_is_sdk_protocol() {
        let events = vec![malformed_turn_end()];
        let err = extract_finish_reason(&events).unwrap_err();
        match err {
            Error::SdkProtocol { message } => {
                assert_eq!(message, "turn/end event requires a string data.reason.kind");
            }
            other => panic!("expected SdkProtocol, got {other:?}"),
        }
    }

    #[test]
    fn extract_finish_reason_picks_the_last_turn_end() {
        // The last turn/end wins; malformedness is checked only on it.
        let events = vec![turn_end_with_kind("completed"), malformed_turn_end()];
        assert!(matches!(
            extract_finish_reason(&events),
            Err(Error::SdkProtocol { .. })
        ));

        // A valid last turn/end hides an earlier malformed one — the
        // reversed scan stops at the last and never reaches earlier events.
        let events = vec![malformed_turn_end(), turn_end_with_kind("max-tokens")];
        assert_eq!(
            extract_finish_reason(&events).unwrap(),
            Some("max-tokens".to_string())
        );
    }
}
