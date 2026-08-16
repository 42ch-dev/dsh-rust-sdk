//! `Session::run` semantics against the scripted fake runtime: the
//! Python-parity activity-interval algorithm (plan 02 task 3) proven end to
//! end. One `#[tokio::test]` per plan scenario; the fixture peer
//! (`tests/fixtures/fake_runtime.rs`) scripts every interval
//! deterministically — ordering is enforced by the wire, never by wall-clock
//! sleeps.

mod common;

use deepseek_harness_sdk::{DeepSeekHarness, Error, Input, RunResult};
use serde_json::{json, Value};

use common::fake_runtime::{
    emit, exit, expect, expect_params, harness_config, respond, respond_error, run_prefix,
    server_info_result, test_session_root, Directive,
};

/// The canonical session ids for the run-semantics scenarios.
const ROOT_SESSION: &str = "root";
const CHILD_SESSION: &str = "child";

/// Spawn a [`DeepSeekHarness`] running the fake-runtime peer against
/// `script`.
async fn harness(script: &[Directive]) -> DeepSeekHarness {
    DeepSeekHarness::start(harness_config(script).expect("serialize script"))
        .await
        .expect("harness starts")
}

/// Run one text turn against `script` and close the harness on both the
/// success and the error path.
async fn run_once(script: &[Directive], input: &str) -> Result<RunResult, Error> {
    let mut h = harness(script).await;
    let result = h
        .start_session(Some(ROOT_SESSION.to_string()))
        .run(Input::Text(input.to_string()))
        .await;
    h.close().await.expect("clean close");
    result
}

/// A `session.event` notification payload for one session.
fn session_event(session_id: &str, event: Value) -> Value {
    json!({"sessionId": session_id, "event": event})
}

/// A `session.event` notification payload for the root session.
fn root_event(event: Value) -> Value {
    session_event(ROOT_SESSION, event)
}

/// The durable inbox receipt event for `message_id` (`agent/inbox/spliced`
/// with `inserted[].id` — the Python `_is_inbox_receipt` field, not
/// `messageId`).
fn receipt_event(message_id: &str) -> Value {
    json!({
        "type": "agent/inbox/spliced",
        "data": {"inserted": [{"id": message_id}]}
    })
}

/// An `assistant/message` event whose content list is `content`.
fn assistant_event(content: Value) -> Value {
    json!({"type": "assistant/message", "data": {"message": {"content": content}}})
}

/// A `turn/end` event with the given reason kind.
fn turn_end(kind: &str) -> Value {
    json!({"type": "turn/end", "data": {"reason": {"kind": kind}}})
}

/// A `session.status` payload reporting `status` for one session.
fn status(session_id: &str, status: &str) -> Value {
    json!({"sessionId": session_id, "status": status})
}

/// A `session.status` payload reporting `idle`.
fn idle(session_id: &str) -> Value {
    status(session_id, "idle")
}

/// The event `type` of a `session.event` notification payload, when the
/// payload is a well-formed event envelope.
fn event_type(payload: &serde_json::Map<String, Value>) -> Option<&str> {
    payload
        .get("event")
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
}

#[tokio::test]
async fn full_happy_path_yields_python_run_result() {
    let script = [
        // Lock the Python-parity defaults on the wire.
        expect_params(
            "initialize",
            json!({"provider": "deepseek-official", "model": "deepseek-v4-flash"}),
        ),
        respond(server_info_result()),
        // Lock the normalized single-text-block prompt.
        expect_params(
            "session/prompt",
            json!({
                "sessionId": ROOT_SESSION,
                "contentBlocks": [{"type": "text", "text": "hello"}],
            }),
        ),
        respond(json!({"messageId": "msg-1"})),
        emit("session.event", root_event(receipt_event("msg-1"))),
        emit(
            "session.event",
            root_event(assistant_event(json!([{
                "type": "text",
                "text": "Hello from the fake runtime"
            }]))),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ];
    let result = run_once(&script, "hello").await.expect("run succeeds");

    assert_eq!(result.session_id, ROOT_SESSION);
    assert_eq!(result.final_response, "Hello from the fake runtime");
    assert_eq!(result.finish_reason.as_deref(), Some("completed"));
    // events: root session.event payloads only, receipt inclusive.
    let types: Vec<&str> = result
        .events
        .iter()
        .filter_map(|e| e.get("type").and_then(Value::as_str))
        .collect();
    assert_eq!(
        types,
        ["agent/inbox/spliced", "assistant/message", "turn/end"]
    );
    // notifications: every tree notification in transport order.
    let methods: Vec<&str> = result
        .notifications
        .iter()
        .map(|n| n.method.as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "session.event",
            "session.event",
            "session.event",
            "session.status"
        ]
    );
    assert_eq!(
        event_type(&result.notifications[0].payload),
        Some("agent/inbox/spliced")
    );
    assert_eq!(
        result.notifications[3]
            .payload
            .get("status")
            .and_then(Value::as_str),
        Some("idle")
    );
    assert_eq!(result.session_root, Some(test_session_root()));
}

#[tokio::test]
async fn notifications_before_the_receipt_are_excluded() {
    let mut script = run_prefix("msg-2");
    script.extend([
        // A root event arriving after the prompt response but before the
        // receipt: it must reach neither `events` nor `notifications`.
        emit(
            "session.event",
            root_event(json!({"type": "test", "text": "before-receipt"})),
        ),
        emit("session.event", root_event(receipt_event("msg-2"))),
        emit(
            "session.event",
            root_event(assistant_event(
                json!([{"type": "text", "text": "after-gate"}]),
            )),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let result = run_once(&script, "hello").await.expect("run succeeds");

    assert!(
        result
            .events
            .iter()
            .all(|e| e.get("type").and_then(Value::as_str) != Some("test")),
        "the pre-receipt event must not reach events: {:?}",
        result.events
    );
    assert!(
        result
            .notifications
            .iter()
            .all(|n| event_type(&n.payload) != Some("test")),
        "the pre-receipt event must not reach notifications: {:?}",
        result.notifications
    );
    // The window starts at the receipt, inclusive.
    assert_eq!(result.notifications.len(), 4);
    assert_eq!(
        event_type(&result.notifications[0].payload),
        Some("agent/inbox/spliced")
    );
    assert_eq!(result.final_response, "after-gate");
}

#[tokio::test]
async fn subagent_events_are_notifications_not_events() {
    let mut script = run_prefix("msg-3");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-3"))),
        emit(
            "subagent.started",
            json!({"parentSessionId": ROOT_SESSION, "childSessionId": CHILD_SESSION}),
        ),
        emit(
            "session.event",
            session_event(
                CHILD_SESSION,
                assistant_event(json!([{"type": "text", "text": "child-output"}])),
            ),
        ),
        emit(
            "session.event",
            root_event(assistant_event(
                json!([{"type": "text", "text": "root-output"}]),
            )),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let result = run_once(&script, "hello").await.expect("run succeeds");

    // The child event is in notifications, in transport order...
    let methods: Vec<&str> = result
        .notifications
        .iter()
        .map(|n| n.method.as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "session.event",
            "subagent.started",
            "session.event",
            "session.event",
            "session.event",
            "session.status"
        ]
    );
    assert_eq!(
        result.notifications[2]
            .payload
            .get("sessionId")
            .and_then(Value::as_str),
        Some(CHILD_SESSION)
    );
    // ...but not in events (root session.event payloads only).
    let types: Vec<&str> = result
        .events
        .iter()
        .filter_map(|e| e.get("type").and_then(Value::as_str))
        .collect();
    assert_eq!(
        types,
        ["agent/inbox/spliced", "assistant/message", "turn/end"]
    );
    // The child's payload (both assistant/message events share the type tag,
    // so match the body directly) must not be among the root events.
    assert!(
        result.events.iter().all(|e| {
            e.pointer("/data/message/content/0/text")
                .and_then(Value::as_str)
                != Some("child-output")
        }),
        "the child event must not leak into events: {:?}",
        result.events
    );
    // final_response derives from the root assistant/message, not the
    // child's.
    assert_eq!(result.final_response, "root-output");
}

#[tokio::test]
async fn non_root_idle_does_not_terminate_the_run() {
    let mut script = run_prefix("msg-4");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-4"))),
        emit(
            "subagent.started",
            json!({"parentSessionId": ROOT_SESSION, "childSessionId": CHILD_SESSION}),
        ),
        emit("session.status", idle(CHILD_SESSION)),
        emit(
            "session.event",
            root_event(assistant_event(json!([{
                "type": "text",
                "text": "after-child-idle"
            }]))),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let result = run_once(&script, "hello").await.expect("run succeeds");

    // The root assistant/message after the child idle is collected: the run
    // survived the non-root idle.
    assert_eq!(result.final_response, "after-child-idle");
    assert_eq!(result.finish_reason.as_deref(), Some("completed"));
    assert_eq!(result.notifications.len(), 6);
    // [receipt, subagent.started, child idle, assistant, turn/end, root idle]
    assert_eq!(
        result.notifications[2]
            .payload
            .get("sessionId")
            .and_then(Value::as_str),
        Some(CHILD_SESSION)
    );
    assert_eq!(
        result.notifications[2]
            .payload
            .get("status")
            .and_then(Value::as_str),
        Some("idle")
    );
}

#[tokio::test]
async fn no_turn_end_in_window_means_finish_reason_none() {
    let mut script = run_prefix("msg-5");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-5"))),
        emit(
            "session.event",
            root_event(assistant_event(
                json!([{"type": "text", "text": "no-reason"}]),
            )),
        ),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let result = run_once(&script, "hello").await.expect("run succeeds");

    assert_eq!(result.finish_reason, None);
    assert_eq!(result.final_response, "no-reason");
}

#[tokio::test]
async fn malformed_turn_end_is_sdk_protocol() {
    let mut script = run_prefix("msg-6");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-6"))),
        // A turn/end without a string data.reason.kind.
        emit(
            "session.event",
            root_event(json!({"type": "turn/end", "data": {"reason": {}}})),
        ),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let err = run_once(&script, "hello")
        .await
        .expect_err("malformed turn/end must fail");
    match err {
        Error::SdkProtocol { message } => {
            assert_eq!(message, "turn/end event requires a string data.reason.kind");
        }
        other => panic!("expected SdkProtocol, got {other:?}"),
    }
}

#[tokio::test]
async fn final_response_concatenates_last_assistant_message_text_blocks() {
    let mut script = run_prefix("msg-7");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-7"))),
        // An earlier assistant/message must not win.
        emit(
            "session.event",
            root_event(assistant_event(
                json!([{"type": "text", "text": "old-output"}]),
            )),
        ),
        emit(
            "session.event",
            root_event(assistant_event(json!([
                {"type": "text", "text": "Hello, "},
                {"type": "reasoning", "text": "hidden"},
                {"type": "text", "text": null},
                {"type": "text", "text": "world!"},
            ]))),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let result = run_once(&script, "hello").await.expect("run succeeds");

    // Text blocks concatenated in order; non-text blocks skipped; a null
    // `text` contributes "" (Python parity); only the LAST assistant/message
    // is used.
    assert_eq!(result.final_response, "Hello, world!");
}

#[tokio::test]
async fn last_assistant_message_with_only_null_text_blocks_yields_empty_response() {
    let mut script = run_prefix("msg-9");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-9"))),
        // An earlier assistant/message must not win the pointer walk.
        emit(
            "session.event",
            root_event(assistant_event(
                json!([{"type": "text", "text": "stale-output"}]),
            )),
        ),
        // The last assistant/message has a single text block with
        // `text: null` — the discriminating case for the literal
        // "text: null contributes `""`" constraint (a null-only last message
        // must yield an empty response, not fall back to the earlier text).
        emit(
            "session.event",
            root_event(assistant_event(json!([{"type": "text", "text": null}]))),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let result = run_once(&script, "hello").await.expect("run succeeds");

    assert_eq!(
        result.final_response, "",
        "`text: null` contributes \"\" — the last message's only text block \
         is null, so the response must be empty (no fallback to the earlier \
         assistant/message)"
    );
    assert_eq!(result.finish_reason.as_deref(), Some("completed"));
}

#[tokio::test]
async fn prompt_error_propagates_and_client_stays_usable() {
    let script = [
        expect("initialize"),
        respond(server_info_result()),
        expect("session/prompt"),
        respond_error(
            -32000,
            "model overloaded",
            Some(json!({"detail": "queue full"})),
        ),
        expect("session/prompt"),
        respond(json!({"messageId": "msg-8"})),
        emit("session.event", root_event(receipt_event("msg-8"))),
        emit(
            "session.event",
            root_event(assistant_event(
                json!([{"type": "text", "text": "second-chance"}]),
            )),
        ),
        emit("session.event", root_event(turn_end("completed"))),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ];
    let mut h = harness(&script).await;
    let session = h.start_session(Some(ROOT_SESSION.to_string()));

    let err = session
        .run(Input::Text("hello".to_string()))
        .await
        .expect_err("a JSON-RPC error on session/prompt must propagate");
    match err {
        Error::JsonRpc {
            code,
            message,
            data,
        } => {
            assert_eq!(code, Some(-32000));
            assert_eq!(message, "model overloaded");
            assert_eq!(data, Some(json!({"detail": "queue full"})));
        }
        other => panic!("expected JsonRpc, got {other:?}"),
    }

    // The harness (and the session) stay usable: a second turn runs to
    // completion against the same peer.
    let result = session
        .run(Input::Text("again".to_string()))
        .await
        .expect("the client must remain usable after a prompt error");
    assert_eq!(result.final_response, "second-chance");
    assert_eq!(result.finish_reason.as_deref(), Some("completed"));

    h.close().await.expect("clean close");
}

#[tokio::test]
async fn malformed_session_event_during_receipt_wait_fails_fast() {
    // Phase 1: the first session.event could be the receipt itself. The
    // payload keeps a valid `sessionId` (so the tree filter admits it) but
    // drops the required `event` field, failing the wire parse — the run
    // must surface SdkProtocol instead of treating it as a non-event, where
    // a silent skip would wait forever for a receipt that can never match
    // (Python raises on malformed notifications; Rust surfaces the typed
    // error).
    let mut script = run_prefix("msg-mal-p1");
    script.extend([
        emit("session.event", json!({"sessionId": ROOT_SESSION})),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let err = run_once(&script, "hello")
        .await
        .expect_err("a malformed session.event during the receipt wait must fail the run");
    match err {
        Error::SdkProtocol { message } => {
            assert!(
                message.contains("malformed session.event during Session::run"),
                "the error must name the malformed payload: {message}"
            );
            assert!(
                message.contains("inbox receipt could not be confirmed"),
                "the Phase-1 arm must be the one that fired: {message}"
            );
        }
        other => panic!("expected SdkProtocol, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_session_event_during_collection_fails_fast() {
    // Phase 2: a malformed session.event cannot be classified as root (or
    // child) — it would silently vanish from `events` while still appearing
    // in `notifications`. The payload keeps a valid `sessionId` (so the
    // tree filter admits it) but drops the required `event` field, failing
    // the wire parse; the run must fail instead of returning a silently
    // truncated result.
    let mut script = run_prefix("msg-mal-p2");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-mal-p2"))),
        emit("session.event", json!({"sessionId": ROOT_SESSION})),
        emit("session.status", idle(ROOT_SESSION)),
        exit(0),
    ]);
    let err = run_once(&script, "hello")
        .await
        .expect_err("a malformed session.event during collection must fail the run");
    match err {
        Error::SdkProtocol { message } => {
            assert!(
                message.contains("malformed session.event during Session::run"),
                "the error must name the malformed payload: {message}"
            );
            assert!(
                message.contains("could not be collected"),
                "the Phase-2 session.event arm must be the one that fired: {message}"
            );
        }
        other => panic!("expected SdkProtocol, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_session_status_fails_fast() {
    // Phase 2: a malformed session.status could be the root idle
    // notification — a silent skip would hang the run forever. A non-string
    // `status` (the wire requires a string) must surface as SdkProtocol.
    let mut script = run_prefix("msg-mal-s");
    script.extend([
        emit("session.event", root_event(receipt_event("msg-mal-s"))),
        emit("session.event", root_event(turn_end("completed"))),
        emit(
            "session.status",
            json!({"sessionId": ROOT_SESSION, "status": 42}),
        ),
        exit(0),
    ]);
    let err = run_once(&script, "hello")
        .await
        .expect_err("a malformed session.status must fail the run");
    match err {
        Error::SdkProtocol { message } => {
            assert!(
                message.contains("malformed session.status during Session::run"),
                "the error must name the malformed payload: {message}"
            );
            assert!(
                message.contains("root idle state could not be determined"),
                "the session.status arm must be the one that fired: {message}"
            );
        }
        other => panic!("expected SdkProtocol, got {other:?}"),
    }
}

#[tokio::test]
async fn broadcast_overflow_before_receipt_fails_fast_with_lag_error() {
    // The run's subscription is created before the prompt is written and
    // only starts reading after `session/prompt` returns. The peer emits
    // 4097 root notifications (one more than the documented
    // DEFAULT_BROADCAST_CAPACITY of 4096) BEFORE answering the prompt, so
    // the receiver falls behind by exactly one while the run is not reading
    // — deterministic by wire order, no wall-clock timing involved. The run
    // must fail fast with the lag SdkProtocol error instead of trusting a
    // truncated stream (the dropped set could include the receipt or the
    // root idle, either of which would otherwise hang the run).
    let mut script = vec![
        expect_params(
            "initialize",
            json!({"provider": "deepseek-official", "model": "deepseek-v4-flash"}),
        ),
        respond(server_info_result()),
        expect("session/prompt"),
    ];
    for _ in 0..4097 {
        script.push(emit(
            "session.event",
            json!({"sessionId": ROOT_SESSION, "event": {}}),
        ));
    }
    script.push(respond(json!({"messageId": "msg-lag"})));
    script.push(exit(0));

    let err = run_once(&script, "hello")
        .await
        .expect_err("a lagged subscription must fail the run");
    match err {
        Error::SdkProtocol { message } => {
            assert!(
                message.contains("fell behind the 4096-notification broadcast buffer"),
                "the lag error must cite the buffer boundary: {message}"
            );
        }
        other => panic!("expected SdkProtocol, got {other:?}"),
    }
}
