//! Wire-protocol types for the DeepSeek Harness SDK runtime protocol.
//!
//! Serde 1:1 models of the JSON-RPC 2.0 request/result pairs and
//! server-to-client notification payloads exchanged over the newline-delimited
//! stdio transport. The shapes mirror the DSH protocol package in
//! [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
//! (`packages/sdk/protocol/src/types.ts`).
//!
//! Parsing is deliberately defensive, matching both reference clients
//! (Python `deepseek_harness` and the TypeScript SDK): unknown JSON fields
//! never fail parsing, `params` values that are not objects normalize to an
//! empty map, and unknown `ContentBlock` types pass through as
//! [`ContentBlock::Unknown`] preserving the raw object.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A content block in a user prompt or assistant message, tagged by `type`.
///
/// The known variants (`text`, `reasoning`, `image`, `tool-call`,
/// `tool-result`) are typed; any other `type` tag deserializes into
/// [`ContentBlock::Unknown`], preserving the raw JSON object verbatim. This
/// keeps the type merge-extensible, mirroring the DSH `ContentBlockMap` (see
/// [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)).
///
/// The untagged fallthrough also covers a *known* tag whose body does not
/// match its variant (e.g. `{"type":"text","text":123}`): serde's
/// internally-tagged codegen retries the [`ContentBlock::Unknown`] variant
/// when a tagged variant's content fails to parse, so a malformed known
/// block is surfaced as data rather than a parse error (locked by
/// `content_block_known_tag_with_malformed_body_falls_through_to_unknown`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text visible to the end user.
    #[serde(rename = "text")]
    Text { text: String },
    /// Reasoning / thinking content, distinct from visible text.
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    /// A durable raster image reference.
    #[serde(rename = "image")]
    Image { attachment: ImageAttachmentRef },
    /// A tool invocation requested by the model.
    #[serde(rename = "tool-call")]
    ToolCall {
        id: String,
        name: String,
        /// Raw JSON string exactly as produced by the model (not an object).
        arguments: String,
    },
    /// The result of a tool invocation, sent back to the model.
    #[serde(rename = "tool-result")]
    ToolResult {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        /// Nested content blocks; `tool-result` content is recursive.
        content: Vec<ContentBlock>,
        #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// Any block whose `type` tag is not a known variant — or whose body
    /// does not match its known variant — preserving the raw JSON object
    /// verbatim.
    ///
    /// `#[serde(untagged)]` on the variant lets serde fall through to it
    /// both when the `type` tag matches no known variant and when a known
    /// variant's body fails to parse; the reference clients are untyped
    /// here, so an unknown or malformed block is surfaced as data, not an
    /// error.
    #[serde(untagged)]
    Unknown(Value),
}

/// Durable, serializable metadata for one immutable image object
/// (mirrors DSH `ImageAttachmentRef`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageAttachmentRef {
    /// Opaque storage identifier; never a filesystem path or bearer URL.
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
    /// Media type verified from the stored bytes (e.g. `image/png`).
    #[serde(rename = "mediaType")]
    pub media_type: String,
    /// Exact encoded byte length.
    pub bytes: u64,
    /// Intrinsic encoded width in pixels.
    pub width: u32,
    /// Intrinsic encoded height in pixels.
    pub height: u32,
    /// Optional display name stripped of local path information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Parameters for the process-wide SDK handshake (`initialize`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeParams {
    /// Working directory recorded on every SDK-created session's header.
    pub cwd: String,
    /// Provider route every SDK-created agent runs on.
    pub provider: String,
    /// Model name every SDK-created agent runs on.
    pub model: String,
    /// Optional positive output-token cap inherited by SDK-created agents and
    /// their in-process descendants. Omitted from the wire when `None`.
    ///
    /// The server requires a **positive safe integer**; `HarnessClient::initialize`
    /// rejects `0`.
    #[serde(rename = "maxTokens", skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

/// Wire-stable server identity returned by initialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

/// Server identity as carried by `initialize` results.
///
/// The wire always carries both fields; `Option` mirrors the Python SDK's
/// defensive model (`name`/`version` default to `None`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

/// One user turn on one SDK session (`session/prompt`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionPromptParams {
    /// The SDK-side session id; an unknown id lazily creates the agent+session pair.
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// The prompt content blocks, sent verbatim as the user message.
    #[serde(rename = "contentBlocks")]
    pub content_blocks: Vec<ContentBlock>,
}

/// Durable enqueue receipt for one prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionPromptResult {
    /// Identity of the queued user message.
    #[serde(rename = "messageId")]
    pub message_id: String,
}

/// A JSON-RPC request id as received from the server: a string or a number.
///
/// Outgoing client request ids are always uuid-v4 strings (matching both
/// reference clients); this untagged enum also accepts numeric ids so server
/// responses and client-directed requests parse defensively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    String(String),
    Number(i64),
}

/// A JSON-RPC error body as received on the wire, preserving `code` and
/// optional `data` verbatim.
///
/// Parsing is defensive like the reference clients (Python `_int_or_none` /
/// TS `typeof` guards): a non-number `code` becomes `None` and a non-string
/// `message` becomes `"JSON-RPC error"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorBody {
    #[serde(default, deserialize_with = "tolerant_code")]
    pub code: Option<i64>,
    #[serde(
        default = "default_error_message",
        deserialize_with = "tolerant_message"
    )]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// The discriminated `result`/`error` half of a JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponseOutcome {
    Error { error: JsonRpcErrorBody },
    Success { result: Value },
}

/// A JSON-RPC response to a client request (has an `id`, plus `result` or
/// `error`). Receive-only: the client never serializes responses.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JsonRpcResponse {
    pub id: JsonRpcId,
    #[serde(flatten)]
    pub outcome: JsonRpcResponseOutcome,
}

/// A client-directed request from the server (has an `id` and a `method`).
///
/// The DSH server currently sends none — the path is defensive. `payload` is
/// the normalized `params` object (non-object `params` become an empty map).
/// Receive-only: the client answers such requests with error frames, it never
/// re-serializes them.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingRequest {
    pub id: JsonRpcId,
    pub method: String,
    pub payload: Map<String, Value>,
}

/// A server-to-client notification (has a `method`, no `id`).
///
/// `payload` is the normalized `params` object: a non-object or missing
/// `params` becomes an empty map (Python parity). Typed accessors
/// ([`Notification::session_event`] etc.) parse the payload defensively and
/// never consume the notification, so callers can always fall back to the raw
/// `payload`. Receive-only.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: String,
    pub payload: Map<String, Value>,
}

impl Notification {
    /// Parse the payload as a `session.event` notification.
    ///
    /// Returns `None` for any other method; `Some(Err)` when the payload does
    /// not match the shape (the notification itself is preserved — callers can
    /// still inspect [`Notification::payload`]).
    pub fn session_event(&self) -> Option<Result<SessionEventNotification, serde_json::Error>> {
        (self.method == "session.event")
            .then(|| serde_json::from_value(Value::Object(self.payload.clone())))
    }

    /// Parse the payload as a `session.status` notification.
    pub fn session_status(&self) -> Option<Result<SessionStatusNotification, serde_json::Error>> {
        (self.method == "session.status")
            .then(|| serde_json::from_value(Value::Object(self.payload.clone())))
    }

    /// Parse the payload as a `subagent.started` notification.
    pub fn subagent_started(
        &self,
    ) -> Option<Result<SubagentStartedNotification, serde_json::Error>> {
        (self.method == "subagent.started")
            .then(|| serde_json::from_value(Value::Object(self.payload.clone())))
    }

    /// Parse the payload as a `subagent.finished` notification.
    pub fn subagent_finished(
        &self,
    ) -> Option<Result<SubagentFinishedNotification, serde_json::Error>> {
        (self.method == "subagent.finished")
            .then(|| serde_json::from_value(Value::Object(self.payload.clone())))
    }
}

/// `session.event` payload: one session-log event, streamed as it is recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEventNotification {
    /// Session the event belongs to (every session in the runtime, not only
    /// SDK-created ones).
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// The full session-log event envelope. Left as a raw value because the
    /// event vocabulary is merge-extensible (DSH `SessionEventMap`); typed
    /// extraction of specific events is a higher-layer concern.
    pub event: Value,
}

/// `session.status` payload: whole-agent lifecycle state for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStatusNotification {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// The whole-agent state after the transition (`"running"` | `"idle"`).
    /// Kept as `String` — like both reference clients, which compare stringly —
    /// so a future status value still parses.
    pub status: String,
}

/// `subagent.started` payload: an in-runtime child session was created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentStartedNotification {
    /// The delegating session.
    #[serde(rename = "parentSessionId")]
    pub parent_session_id: String,
    /// The new child session.
    #[serde(rename = "childSessionId")]
    pub child_session_id: String,
}

/// `subagent.finished` payload: an in-process subagent run ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentFinishedNotification {
    /// Subagent provider name that ran the child.
    pub provider: String,
    /// The child agent's id (equals `child_session_id` for local runs).
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// The delegating session.
    #[serde(rename = "parentSessionId")]
    pub parent_session_id: String,
    /// The child session.
    #[serde(rename = "childSessionId")]
    pub child_session_id: String,
    /// Deployment-mapped run outcome (`"ok"` | `"error"`), kept as `String`
    /// for the same reason as [`SessionStatusNotification::status`].
    pub status: String,
    /// The provider-reported stop reason. Merge-extensible in DSH
    /// (`SubagentStopReasonMap`); `String` accepts every existing and future
    /// value.
    #[serde(rename = "stopReason")]
    pub stop_reason: String,
    /// The child's selected assistant output; absent when the child produced
    /// none.
    #[serde(
        rename = "lastAssistantMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_assistant_message: Option<Vec<ContentBlock>>,
}

/// A server-to-client frame on the wire.
///
/// Mirrors the reference transports' dispatch rules: a frame with `id` +
/// `method` is a client-directed request, `id` alone is a response, and
/// `method` alone is a notification. Frames that match none of the shapes fail
/// to parse and are skipped by the line transport (malformed-peer-line
/// tolerance).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum IncomingFrame {
    Response(JsonRpcResponse),
    Request(IncomingRequest),
    Notification(Notification),
}

impl<'de> Deserialize<'de> for IncomingRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| de::Error::custom("request must be a JSON object"))?;
        let id = object
            .get("id")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(de::Error::custom)?
            .ok_or_else(|| de::Error::custom("request requires an `id`"))?;
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::custom("request requires a string `method`"))?
            .to_string();
        let payload = match object.get("params") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        Ok(IncomingRequest {
            id,
            method,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for Notification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| de::Error::custom("notification must be a JSON object"))?;
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::custom("notification requires a string `method`"))?
            .to_string();
        let payload = match object.get("params") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        Ok(Notification { method, payload })
    }
}

/// Normalize a JSON-RPC error `code`: any integer becomes `Some`, anything
/// else becomes `None` (Python `_int_or_none` parity).
fn tolerant_code<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Value::deserialize(deserializer)?.as_i64())
}

/// Normalize a JSON-RPC error `message`: any string is kept, anything else
/// (including a missing value) becomes `"JSON-RPC error"`.
fn tolerant_message<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Value::deserialize(deserializer)?
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| "JSON-RPC error".to_string()))
}

/// `#[serde(default)]` for a missing `message` field (reference parity).
fn default_error_message() -> String {
    "JSON-RPC error".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// Deserialize `literal`, assert it equals `value`, serialize it back and
    /// assert the compact output equals the same JSON document.
    fn assert_round_trip<T>(value: &T, literal: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let parsed: T = serde_json::from_str(literal).unwrap();
        assert_eq!(
            &parsed, value,
            "deserialized struct must equal expected value"
        );
        let serialized = serde_json::to_value(&parsed).unwrap();
        let expected: Value = serde_json::from_str(literal).unwrap();
        assert_eq!(
            serialized, expected,
            "serialized output must equal the wire literal"
        );
        let text = serde_json::to_string(&parsed).unwrap();
        assert!(
            !text.contains(' '),
            "serialization must be compact, got: {text}"
        );
    }

    #[test]
    fn initialize_params_round_trip() {
        let params = InitializeParams {
            cwd: "/x".into(),
            provider: "deepseek".into(),
            model: "m".into(),
            max_tokens: Some(1024),
        };
        assert_round_trip(
            &params,
            r#"{"cwd":"/x","provider":"deepseek","model":"m","maxTokens":1024}"#,
        );
    }

    #[test]
    fn initialize_params_omits_max_tokens_when_none() {
        let params = InitializeParams {
            cwd: "/x".into(),
            provider: "deepseek".into(),
            model: "m".into(),
            max_tokens: None,
        };
        let out = serde_json::to_value(&params).unwrap();
        assert_eq!(out, json!({"cwd":"/x","provider":"deepseek","model":"m"}));
    }

    #[test]
    fn initialize_result_round_trip() {
        let result = InitializeResult {
            server_info: ServerInfo {
                name: Some("deepseek-harness-sdk-runtime".into()),
                version: Some("0.0.1".into()),
            },
        };
        assert_round_trip(
            &result,
            r#"{"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.0.1"}}"#,
        );
    }

    #[test]
    fn server_info_ignores_unknown_fields_and_missing_identity() {
        let parsed: ServerInfo =
            serde_json::from_str(r#"{"name":"x","version":"1.0.0","futureField":123}"#).unwrap();
        assert_eq!(
            parsed,
            ServerInfo {
                name: Some("x".into()),
                version: Some("1.0.0".into())
            }
        );
        let empty: ServerInfo = serde_json::from_str("{}").unwrap();
        assert_eq!(
            empty,
            ServerInfo {
                name: None,
                version: None
            }
        );
    }

    #[test]
    fn session_prompt_params_round_trip() {
        let params = SessionPromptParams {
            session_id: "s1".into(),
            content_blocks: vec![ContentBlock::Text { text: "hi".into() }],
        };
        assert_round_trip(
            &params,
            r#"{"sessionId":"s1","contentBlocks":[{"type":"text","text":"hi"}]}"#,
        );
    }

    #[test]
    fn session_prompt_result_round_trip() {
        let result = SessionPromptResult {
            message_id: "m1".into(),
        };
        assert_round_trip(&result, r#"{"messageId":"m1"}"#);
    }

    #[test]
    fn content_block_known_variants_round_trip() {
        let literals = [
            json!({"type":"text","text":"hello"}),
            json!({"type":"reasoning","text":"think"}),
            json!({"type":"image","attachment":{
                "attachmentId":"att-1","mediaType":"image/png","bytes":123,"width":10,"height":20
            }}),
            json!({"type":"tool-call","id":"c1","name":"fs_read","arguments":"{\"path\":\"/x\"}"}),
            json!({"type":"tool-result","toolCallId":"c2",
                "content":[{"type":"text","text":"ok"}],"isError":true}),
        ];
        for literal in literals {
            let block: ContentBlock = serde_json::from_value(literal.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&block).unwrap(),
                literal,
                "known variant must round-trip verbatim"
            );
        }
        // Typed access: each variant keeps its documented fields.
        let block: ContentBlock = serde_json::from_value(
            json!({"type":"tool-call","id":"c1","name":"fs_read","arguments":"{}"}),
        )
        .unwrap();
        match block {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "fs_read");
                assert_eq!(arguments, "{}", "tool-call arguments is a raw JSON string");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn content_block_unknown_type_preserves_raw_object() {
        let raw = json!({"type":"fancy-block","foo":1,"bar":[1,2]});
        let block: ContentBlock = serde_json::from_value(raw.clone()).unwrap();
        assert!(
            matches!(block, ContentBlock::Unknown(_)),
            "unknown type -> Unknown"
        );
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            raw,
            "Unknown must preserve the raw object verbatim"
        );
    }

    #[test]
    fn content_block_known_tag_with_malformed_body_falls_through_to_unknown() {
        // FIX-8 behavior lock: serde's internally-tagged codegen retries the
        // untagged variant when a *known* tag's body fails to parse, so a
        // malformed known block degrades to Unknown (data), not a parse
        // error — the doc comment's untagged fallthrough semantics. Verified
        // empirically; locked here so a serde behavior change alerts us.
        for literal in [
            json!({"type":"text","text":123}),
            json!({"type":"tool-call"}), // missing id/name/arguments
        ] {
            let block: ContentBlock =
                serde_json::from_value(literal.clone()).expect("malformed known body -> Unknown");
            assert!(
                matches!(block, ContentBlock::Unknown(_)),
                "malformed known body must degrade to Unknown: {literal}"
            );
        }
        // The wire path (from_str) behaves identically.
        let block: ContentBlock =
            serde_json::from_str(r#"{"type":"text","text":123}"#).unwrap();
        assert!(matches!(block, ContentBlock::Unknown(_)));

        // Through a typed accessor the block parses as Unknown; the
        // notification itself is preserved.
        let n: Notification = serde_json::from_value(json!({
            "method": "subagent.finished",
            "params": {
                "provider": "deepseek", "agentId": "a1",
                "parentSessionId": "p", "childSessionId": "c",
                "status": "ok", "stopReason": "completed",
                "lastAssistantMessage": [{"type": "text", "text": 123}]
            }
        }))
        .unwrap();
        let finished = n
            .subagent_finished()
            .expect("accessor dispatch by method")
            .expect("a malformed known block must not fail the whole payload");
        match &finished.last_assistant_message {
            Some(blocks) => assert!(
                matches!(&blocks[0], ContentBlock::Unknown(_)),
                "the malformed block is surfaced as Unknown data"
            ),
            None => panic!("lastAssistantMessage must be present"),
        }
        assert_eq!(n.method, "subagent.finished", "the notification is preserved");
    }

    #[test]
    fn content_block_ignores_unknown_fields() {
        let block: ContentBlock =
            serde_json::from_value(json!({"type":"text","text":"hi","future":1})).unwrap();
        assert_eq!(block, ContentBlock::Text { text: "hi".into() });
    }

    #[test]
    fn content_block_tool_result_is_error_optional() {
        let block: ContentBlock =
            serde_json::from_value(json!({"type":"tool-result","toolCallId":"c","content":[]}))
                .unwrap();
        match &block {
            ContentBlock::ToolResult { is_error: None, .. } => {}
            other => panic!("expected ToolResult with is_error None, got {other:?}"),
        }
        let out = serde_json::to_value(&block).unwrap();
        assert!(!out.as_object().unwrap().contains_key("isError"));
    }

    #[test]
    fn notification_parses_and_normalizes_params() {
        let n: Notification = serde_json::from_value(json!({
            "jsonrpc":"2.0","method":"session.status",
            "params":{"sessionId":"s","status":"idle"}
        }))
        .unwrap();
        assert_eq!(n.method, "session.status");
        assert_eq!(
            n.payload.get("sessionId").and_then(Value::as_str),
            Some("s")
        );
        assert_eq!(
            n.payload.get("status").and_then(Value::as_str),
            Some("idle")
        );

        // Non-object params normalize to an empty map (Python parity).
        let n: Notification =
            serde_json::from_value(json!({"method":"session.event","params":"oops"})).unwrap();
        assert!(n.payload.is_empty());
        // Missing params normalize to an empty map too.
        let n: Notification = serde_json::from_value(json!({"method":"session.event"})).unwrap();
        assert!(n.payload.is_empty());
        // Unknown top-level fields are ignored.
        let n: Notification = serde_json::from_value(json!({
            "jsonrpc":"2.0","method":"session.status",
            "params":{"sessionId":"s","status":"idle"},"extra":1
        }))
        .unwrap();
        assert_eq!(n.payload.len(), 2);
    }

    #[test]
    fn notification_typed_accessors_dispatch_by_method() {
        let n: Notification = serde_json::from_value(
            json!({"method":"session.status","params":{"sessionId":"s","status":"idle"}}),
        )
        .unwrap();
        assert!(n.session_event().is_none(), "wrong method -> None");
        let status = n.session_status().unwrap().unwrap();
        assert_eq!(status.session_id, "s");
        assert_eq!(status.status, "idle");

        // Malformed payload for the right method -> Some(Err); the notification
        // itself is never dropped.
        let bad: Notification =
            serde_json::from_value(json!({"method":"session.status","params":{"sessionId":42}}))
                .unwrap();
        assert!(bad.session_status().unwrap().is_err());
        assert_eq!(bad.method, "session.status");

        let n: Notification = serde_json::from_value(json!({
            "method":"subagent.started","params":{"parentSessionId":"p","childSessionId":"c"}
        }))
        .unwrap();
        let started = n.subagent_started().unwrap().unwrap();
        assert_eq!(started.parent_session_id, "p");
        assert_eq!(started.child_session_id, "c");
        assert!(n.subagent_finished().is_none());
    }

    #[test]
    fn notification_payload_shapes_round_trip() {
        // session.event: the full session-log event envelope stays a raw Value.
        let n: Notification = serde_json::from_value(json!({
            "method":"session.event",
            "params":{"sessionId":"s","event":{"type":"user/message","seq":1,"time":123,
                "data":{"content":[{"type":"text","text":"hi"}]}}}
        }))
        .unwrap();
        let event = n.session_event().unwrap().unwrap();
        assert_eq!(event.session_id, "s");
        assert_eq!(
            event.event.get("type").and_then(Value::as_str),
            Some("user/message")
        );
        assert_round_trip(
            &SessionEventNotification {
                session_id: "s".into(),
                event: json!({"type":"user/message"}),
            },
            r#"{"sessionId":"s","event":{"type":"user/message"}}"#,
        );

        assert_round_trip(
            &SessionStatusNotification {
                session_id: "s".into(),
                status: "running".into(),
            },
            r#"{"sessionId":"s","status":"running"}"#,
        );
        assert_round_trip(
            &SubagentStartedNotification {
                parent_session_id: "p".into(),
                child_session_id: "c".into(),
            },
            r#"{"parentSessionId":"p","childSessionId":"c"}"#,
        );

        let finished = SubagentFinishedNotification {
            provider: "deepseek".into(),
            agent_id: "a1".into(),
            parent_session_id: "p".into(),
            child_session_id: "c".into(),
            status: "ok".into(),
            stop_reason: "completed".into(),
            last_assistant_message: Some(vec![ContentBlock::Text {
                text: "done".into(),
            }]),
        };
        assert_round_trip(
            &finished,
            r#"{"provider":"deepseek","agentId":"a1","parentSessionId":"p","childSessionId":"c","status":"ok","stopReason":"completed","lastAssistantMessage":[{"type":"text","text":"done"}]}"#,
        );

        // lastAssistantMessage is omitted when absent; "error" status round-trips.
        let without = SubagentFinishedNotification {
            last_assistant_message: None,
            status: "error".into(),
            ..finished.clone()
        };
        let out = serde_json::to_value(&without).unwrap();
        assert!(!out
            .as_object()
            .unwrap()
            .contains_key("lastAssistantMessage"));
        assert_eq!(without.status, "error");
    }

    #[test]
    fn incoming_frame_discrimination() {
        // id + result -> response.
        let frame: IncomingFrame = serde_json::from_value(json!({
            "jsonrpc":"2.0","id":"req_1","result":{"serverInfo":{"name":"x"}}
        }))
        .unwrap();
        match frame {
            IncomingFrame::Response(r) => {
                assert_eq!(r.id, JsonRpcId::String("req_1".into()));
                assert!(matches!(r.outcome, JsonRpcResponseOutcome::Success { .. }));
            }
            other => panic!("expected response, got {other:?}"),
        }

        // id + error -> response preserving code/message/data.
        let frame: IncomingFrame = serde_json::from_value(json!({
            "jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"method not found: foo","data":{"x":1}}
        }))
        .unwrap();
        match frame {
            IncomingFrame::Response(r) => match r.outcome {
                JsonRpcResponseOutcome::Error { error } => {
                    assert_eq!(error.code, Some(-32601));
                    assert_eq!(error.message, "method not found: foo");
                    assert_eq!(error.data, Some(json!({"x":1})));
                }
                other => panic!("expected error outcome, got {other:?}"),
            },
            other => panic!("expected response, got {other:?}"),
        }

        // id + method -> client-directed request with normalized payload.
        let frame: IncomingFrame = serde_json::from_value(json!({
            "jsonrpc":"2.0","id":9,"method":"some.request","params":{"a":1}
        }))
        .unwrap();
        match frame {
            IncomingFrame::Request(r) => {
                assert_eq!(r.id, JsonRpcId::Number(9));
                assert_eq!(r.method, "some.request");
                assert_eq!(r.payload.get("a"), Some(&json!(1)));
            }
            other => panic!("expected request, got {other:?}"),
        }

        // id + method without params -> empty payload.
        let frame: IncomingFrame = serde_json::from_value(json!({"id":10,"method":"x"})).unwrap();
        match frame {
            IncomingFrame::Request(r) => assert!(r.payload.is_empty()),
            other => panic!("expected request, got {other:?}"),
        }

        // method without id -> notification.
        let frame: IncomingFrame = serde_json::from_value(json!({
            "jsonrpc":"2.0","method":"session.status","params":{"sessionId":"s","status":"idle"}
        }))
        .unwrap();
        match frame {
            IncomingFrame::Notification(n) => assert_eq!(n.method, "session.status"),
            other => panic!("expected notification, got {other:?}"),
        }
    }

    #[test]
    fn json_rpc_id_untagged_string_or_number() {
        let id: JsonRpcId = serde_json::from_str(r#""req_1""#).unwrap();
        assert_eq!(id, JsonRpcId::String("req_1".into()));
        let id: JsonRpcId = serde_json::from_str("42").unwrap();
        assert_eq!(id, JsonRpcId::Number(42));
        assert_eq!(
            serde_json::to_string(&JsonRpcId::String("x".into())).unwrap(),
            r#""x""#
        );
        assert_eq!(serde_json::to_string(&JsonRpcId::Number(1)).unwrap(), "1");
    }

    #[test]
    fn json_rpc_error_body_defensive_parsing() {
        // Non-number code and non-string message normalize like the reference
        // clients (Python `_int_or_none` / TS `typeof` guards).
        let e: JsonRpcErrorBody =
            serde_json::from_value(json!({"code":"bad","message":42})).unwrap();
        assert_eq!(e.code, None);
        assert_eq!(e.message, "JSON-RPC error");
        assert_eq!(e.data, None);

        // JSON null data collapses to None (Option semantics; Python parity).
        let e: JsonRpcErrorBody =
            serde_json::from_value(json!({"code":-32000,"message":"x","data":null})).unwrap();
        assert_eq!(e.code, Some(-32000));
        assert_eq!(e.data, None);

        assert_round_trip(
            &JsonRpcErrorBody {
                code: Some(-32601),
                message: "m".into(),
                data: Some(json!({"k":1})),
            },
            r#"{"code":-32601,"message":"m","data":{"k":1}}"#,
        );
    }
}
