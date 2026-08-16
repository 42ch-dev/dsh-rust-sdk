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
//! [`RunResult`] follows the **Python** SDK field set, including
//! `finish_reason` and `session_root`; the TypeScript SDK's `RunResult`
//! lacks both fields, and Rust intentionally does not claim TypeScript
//! surface parity.
//!
//! The runtime binary is bring-your-own (Plan A): [`DeepSeekHarness::start`]
//! resolves it from `Config::runtime_bin` / `launch_args_override` or the
//! `DSH_RUNTIME_BIN` environment variable. This crate never downloads or
//! bundles a runtime. The official runtime and its sources live at
//! <https://github.com/deepseek-ai/deepseek-harness>.

use std::path::PathBuf;

use serde_json::Value;
use uuid::Uuid;

use crate::client::{HarnessClient, LaunchSpec};
use crate::error::Error;
use crate::protocol::{ContentBlock, Notification};
use crate::runtime::{compose_env, resolve_runtime, Config};

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
    /// The configured session root (`DSH_SESSION_ROOT`), surfaced on every
    /// [`RunResult`] (Python extension field; TypeScript lacks it).
    session_root: Option<PathBuf>,
}

impl DeepSeekHarness {
    /// Resolve the runtime, compose the env injection set, spawn the
    /// subprocess, and perform the `initialize` handshake.
    ///
    /// `Config::cwd` is resolved absolute (Python `Path(cwd).resolve()`) and
    /// feeds both `DSH_CWD` and `initialize.cwd`; a nonexistent cwd fails
    /// with [`Error::Io`]. The runtime subprocess cwd defaults to the same
    /// resolved cwd (`Config::runtime_cwd` overrides it — Python parity).
    ///
    /// [`Config::request_timeout`] bounds every request, including
    /// `session/prompt`; `None` (the default) waits indefinitely.
    ///
    /// On `initialize` failure the close ladder is run before the error
    /// propagates, so the spawned child is never leaked (Python parity).
    pub async fn start(config: Config) -> Result<Self, Error> {
        let launch = resolve_runtime(&config)?;
        let cwd = match &config.cwd {
            Some(path) => path.canonicalize().map_err(Error::Io)?,
            None => std::env::current_dir().map_err(Error::Io)?,
        };
        let spec = LaunchSpec {
            program: launch.program,
            args: launch.args,
            envs: compose_env(&config, &cwd).into_iter().collect(),
            cwd: Some(config.runtime_cwd.clone().unwrap_or_else(|| cwd.clone())),
        };
        // `Config::request_timeout` is the Python-parity request deadline
        // (`None` = wait indefinitely); `Config::timeouts` supplies the
        // close-ladder timings.
        let mut timeouts = config.timeouts;
        timeouts.request_timeout = config.request_timeout;
        let mut client = HarnessClient::spawn(spec, timeouts)?;
        if let Err(err) = client
            .initialize(
                cwd.to_string_lossy().into_owned(),
                &config.provider,
                &config.model,
                config.max_tokens,
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
            session_root: config.session_root.map(PathBuf::from),
        })
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
    /// The wait-for-idle interval is **unbounded** (Python parity): only the
    /// `session/prompt` request is bounded by `request_timeout`. Callers
    /// needing a bound should wrap this call in `tokio::time::timeout`.
    pub async fn run(&self, input: Input) -> Result<RunResult, Error> {
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
        // `notifications` (Python parity).
        let receipt = loop {
            let notification = subscription.recv().await?;
            let is_receipt = match notification.session_event() {
                Some(Ok(event)) => {
                    event.session_id == *root && is_inbox_receipt(&event.event, &message_id)
                }
                _ => false,
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
            if let Some(Ok(event)) = notification.session_event() {
                if event.session_id == *root && event.event.is_object() {
                    events.push(event.event);
                }
            }
            let root_idle = matches!(
                notification.session_status(),
                Some(Ok(status)) if status.session_id == *root && status.status == "idle"
            );
            notifications.push(notification);
            if root_idle {
                break;
            }
            notification = subscription.recv().await?;
        }

        let finish_reason = extract_finish_reason(&events)?;
        let final_response = derive_final_response(&events);
        Ok(RunResult {
            session_id: self.session_id.clone(),
            final_response,
            finish_reason,
            events,
            notifications,
            session_root: self.harness.session_root.clone(),
        })
    }
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
/// `RunResult` (the TypeScript SDK's `RunResult` lacks `finish_reason` and
/// `session_root`; Rust intentionally follows Python).
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    /// The SDK session id this turn ran on.
    pub session_id: String,
    /// Text concatenation of the **last** root `assistant/message` event's
    /// text blocks (`text: null` blocks contribute `""`); `""` when the
    /// activity interval contains no `assistant/message` — or the last one
    /// has no text blocks. Never falls back to an earlier event.
    pub final_response: String,
    /// The last root `turn/end` event's `data.reason.kind` inside the
    /// activity interval (`None` when the window has no `turn/end`).
    pub finish_reason: Option<String>,
    /// Root-session `session.event` payloads only, in transport order.
    pub events: Vec<Value>,
    /// Every tree notification (root + discovered descendants, incl.
    /// `session.status` / `subagent.*`), in transport order.
    pub notifications: Vec<Notification>,
    /// The configured session root (`DSH_SESSION_ROOT`), Python extension
    /// field.
    pub session_root: Option<PathBuf>,
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
/// falls back to an earlier event).
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
        .filter_map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            block.get("text").and_then(Value::as_str)
        })
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
