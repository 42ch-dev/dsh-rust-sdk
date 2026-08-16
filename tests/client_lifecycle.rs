//! End-to-end client lifecycle tests against the scripted `fake-runtime`
//! stdio JSON-RPC peer (harness: `tests/common/fake_runtime.rs`; peer binary:
//! `tests/fixtures/fake_runtime.rs`). One `#[tokio::test]` per plan scenario.
//!
//! No external DSH checkout or network is involved: the peer is compiled
//! from this crate's own fixtures and spawned as a subprocess.

mod common;

use std::time::Duration;

use deepseek_harness_sdk::{ClientTimeouts, ContentBlock, Error, HarnessClient, LaunchSpec};
use serde_json::json;

use common::fake_runtime::{
    emit, emit_blank, emit_raw, exit, expect, expect_params, fake_runtime_spec, ignore_all,
    respond, respond_error, server_info_result, sleep_forever_bin, sleep_ms, test_timeouts,
    FakeRuntime,
};

/// The canonical client-side session ids used across scenarios.
const ROOT_SESSION: &str = "root";
const CHILD_SESSION: &str = "child";
const UNRELATED_SESSION: &str = "unrelated";

async fn initialize_ok(rt: &mut FakeRuntime) {
    rt.client
        .initialize("/tmp", "deepseek", "deepseek-chat", Some(1024))
        .await
        .expect("initialize succeeds");
}

#[tokio::test]
async fn initialize_happy_path_returns_server_info() {
    let mut rt = FakeRuntime::spawn(&[
        expect_params(
            "initialize",
            json!({
                "cwd": "/tmp",
                "provider": "deepseek",
                "model": "deepseek-chat",
                "maxTokens": 1024,
            }),
        ),
        respond(server_info_result()),
    ])
    .expect("spawn fake runtime");

    let result = rt
        .client
        .initialize("/tmp", "deepseek", "deepseek-chat", Some(1024))
        .await
        .expect("initialize succeeds");
    assert_eq!(
        result.server_info.name.as_deref(),
        Some("deepseek-harness-sdk-runtime")
    );
    assert_eq!(result.server_info.version.as_deref(), Some("0.0.1"));

    rt.client.close().await.expect("clean close");
}

#[tokio::test]
async fn initialize_with_wrong_server_name_returns_sdk_protocol() {
    let mut rt = FakeRuntime::spawn(&[
        expect("initialize"),
        respond(json!({"serverInfo": {"name": "some-other-runtime", "version": "0.0.1"}})),
    ])
    .expect("spawn fake runtime");

    let err = rt
        .client
        .initialize("/tmp", "deepseek", "deepseek-chat", Some(1024))
        .await
        .expect_err("initialize must reject a foreign server identity");
    assert!(
        matches!(err, Error::SdkProtocol { .. }),
        "unexpected error: {err}"
    );

    rt.client.close().await.expect("clean close");
}

#[tokio::test]
async fn session_prompt_round_trips_message_id() {
    let mut rt = FakeRuntime::spawn(&[
        expect_params(
            "session/prompt",
            json!({"sessionId": "sess-1", "contentBlocks": [{"type": "text", "text": "Hello"}]}),
        ),
        respond(json!({"messageId": "msg-42"})),
    ])
    .expect("spawn fake runtime");

    let message_id = rt
        .client
        .session_prompt(
            "sess-1",
            vec![ContentBlock::Text {
                text: "Hello".into(),
            }],
        )
        .await
        .expect("session/prompt succeeds");
    assert_eq!(message_id, "msg-42");

    rt.client.close().await.expect("clean close");
}

#[tokio::test]
async fn jsonrpc_error_response_preserves_code_and_data() {
    let mut rt = FakeRuntime::spawn(&[
        expect("session/prompt"),
        respond_error(
            -32000,
            "model overloaded",
            Some(json!({"detail": "queue full"})),
        ),
    ])
    .expect("spawn fake runtime");

    let err = rt
        .client
        .session_prompt("sess-1", vec![ContentBlock::Text { text: "hi".into() }])
        .await
        .expect_err("error response must surface as Error::JsonRpc");
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
        other => panic!("unexpected error: {other}"),
    }

    rt.client.close().await.expect("clean close");
}

#[tokio::test]
async fn session_tree_fanout_reaches_only_root_and_child_in_order() {
    let mut rt = FakeRuntime::spawn(&[
        expect("initialize"),
        respond(server_info_result()),
        emit(
            "subagent.started",
            json!({"parentSessionId": ROOT_SESSION, "childSessionId": CHILD_SESSION}),
        ),
        emit(
            "session.event",
            json!({"sessionId": ROOT_SESSION, "event": {"type": "test", "text": "root-event"}}),
        ),
        emit(
            "session.event",
            json!({"sessionId": CHILD_SESSION, "event": {"type": "test", "text": "child-event"}}),
        ),
        emit(
            "session.event",
            json!({"sessionId": UNRELATED_SESSION, "event": {"type": "test", "text": "other-event"}}),
        ),
        // Keep the peer alive so the final no-more-events probe can time out
        // instead of racing the process exit.
        sleep_ms(1000),
    ])
    .expect("spawn fake runtime");

    let mut subscription = rt.client.subscribe_session_tree(ROOT_SESSION);
    initialize_ok(&mut rt).await;

    let started = subscription
        .recv()
        .await
        .expect("subagent.started delivered");
    assert_eq!(started.method, "subagent.started");
    assert_eq!(
        started
            .payload
            .get("childSessionId")
            .and_then(|v| v.as_str()),
        Some(CHILD_SESSION)
    );

    let root_event = subscription.recv().await.expect("root event delivered");
    assert_eq!(root_event.method, "session.event");
    assert_eq!(
        root_event.payload.get("sessionId").and_then(|v| v.as_str()),
        Some(ROOT_SESSION)
    );

    let child_event = subscription.recv().await.expect("child event delivered");
    assert_eq!(child_event.method, "session.event");
    assert_eq!(
        child_event
            .payload
            .get("sessionId")
            .and_then(|v| v.as_str()),
        Some(CHILD_SESSION)
    );

    // The unrelated session's event must never reach this subscriber, in
    // transport order the stream is now quiet.
    let unexpected = tokio::time::timeout(Duration::from_millis(250), subscription.recv()).await;
    if let Ok(Ok(notification)) = unexpected {
        panic!("unrelated notification leaked: {notification:?}");
    }

    rt.client.close().await.expect("clean close");
}

#[tokio::test]
async fn descendant_discovered_mid_stream_passes_filter() {
    let mut rt = FakeRuntime::spawn(&[
        expect("initialize"),
        respond(server_info_result()),
        // Emitted before the parent edge is known; the peer then sleeps so
        // the subscriber drains this event while the tree is still just
        // `root` (the filter is receive-time against the live edge map).
        emit(
            "session.event",
            json!({"sessionId": CHILD_SESSION, "event": {"type": "test", "text": "before-edge"}}),
        ),
        sleep_ms(500),
        // The edge arrives mid-stream; from here on child events pass.
        emit(
            "subagent.started",
            json!({"parentSessionId": ROOT_SESSION, "childSessionId": CHILD_SESSION}),
        ),
        emit(
            "session.event",
            json!({"sessionId": CHILD_SESSION, "event": {"type": "test", "text": "after-edge"}}),
        ),
    ])
    .expect("spawn fake runtime");

    let mut subscription = rt.client.subscribe_session_tree(ROOT_SESSION);
    initialize_ok(&mut rt).await;

    // While the peer sleeps, `child` is not yet in the tree: the pre-edge
    // child event is consumed and dropped by the filter.
    let quiet = tokio::time::timeout(Duration::from_millis(200), subscription.recv()).await;
    if let Ok(Ok(notification)) = quiet {
        panic!("pre-edge child event leaked: {notification:?}");
    }

    // After the mid-stream edge, the subsequent child event passes.
    let started = subscription
        .recv()
        .await
        .expect("subagent.started delivered");
    assert_eq!(started.method, "subagent.started");

    let after_edge = subscription
        .recv()
        .await
        .expect("post-edge child event delivered");
    assert_eq!(after_edge.method, "session.event");
    assert_eq!(
        after_edge.payload.get("sessionId").and_then(|v| v.as_str()),
        Some(CHILD_SESSION)
    );
    assert_eq!(
        after_edge
            .payload
            .get("event")
            .and_then(|e| e.get("text"))
            .and_then(|v| v.as_str()),
        Some("after-edge"),
        "the post-edge child event must be the one delivered, not the pre-edge one"
    );

    rt.client.close().await.expect("clean close");
}

#[tokio::test]
async fn request_timeout_returns_request_timeout() {
    let spec =
        fake_runtime_spec(&[expect("session/prompt"), ignore_all()]).expect("serialize script");
    let timeouts = ClientTimeouts {
        request_timeout: Some(Duration::from_millis(150)),
        ..test_timeouts()
    };
    let mut client = HarnessClient::spawn(spec, timeouts).expect("spawn fake runtime");

    let err = client
        .session_prompt("sess-1", vec![ContentBlock::Text { text: "hi".into() }])
        .await
        .expect_err("an unanswered request must time out");
    assert!(
        matches!(err, Error::RequestTimeout { .. }),
        "unexpected error: {err}"
    );

    // The peer never responds; close() escalates the ladder and reaps it.
    client.close().await.expect("close reaps the ignoring peer");
}

#[tokio::test]
async fn close_ladder_level_one_cooperative_shutdown_and_exit() {
    let mut rt = FakeRuntime::spawn(&[
        expect("initialize"),
        respond(server_info_result()),
        expect("shutdown"),
        respond(json!({})),
        exit(0),
    ])
    .expect("spawn fake runtime");

    initialize_ok(&mut rt).await;

    // The peer answers `shutdown` and exits on its own; the ladder's first
    // tier suffices — no EOF wait, no signals.
    rt.client.close().await.expect("clean close after shutdown");
}

#[tokio::test]
async fn close_ladder_escalates_to_sigterm_when_peer_ignores_shutdown_and_eof() {
    let spec = LaunchSpec {
        program: sleep_forever_bin().to_string(),
        args: vec![],
        envs: Default::default(),
        cwd: None,
    };
    let mut client = HarnessClient::spawn(spec, test_timeouts()).expect("spawn sleep-forever");

    // `shutdown` is never answered and stdin EOF is never read; the ladder
    // must escalate to SIGTERM to reap the process. Every tier is short, so
    // a successful close proves the escalation happened promptly.
    let started = std::time::Instant::now();
    client
        .close()
        .await
        .expect("close reaps the sleeping process");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "close should escalate through the short tiers, not linger"
    );
}

#[tokio::test]
async fn spawn_failures_map_to_typed_errors() {
    let timeouts = test_timeouts();

    // Missing program: ENOENT -> RuntimeNotFound.
    let spec = LaunchSpec {
        program: "/definitely/not/a/deepseek/runtime".into(),
        args: vec![],
        envs: Default::default(),
        cwd: None,
    };
    let err = HarnessClient::spawn(spec, timeouts).expect_err("missing program must fail");
    assert!(
        matches!(err, Error::RuntimeNotFound(_)),
        "unexpected error: {err}"
    );

    // A program that exists but cannot be launched (a directory is not
    // executable) is a plain spawn I/O error, not a NotFound.
    let spec = LaunchSpec {
        program: std::env::temp_dir().to_string_lossy().into_owned(),
        args: vec![],
        envs: Default::default(),
        cwd: None,
    };
    let err = HarnessClient::spawn(spec, timeouts).expect_err("unlaunchable program must fail");
    assert!(matches!(err, Error::Io(_)), "unexpected error: {err}");
}

#[tokio::test]
async fn malformed_and_blank_lines_are_skipped_not_fatal() {
    let mut rt = FakeRuntime::spawn(&[
        expect("initialize"),
        // Garbage + blank between valid frames: both are skipped, and the
        // stream survives to serve the next request (skip-not-reject).
        emit_raw("this is {not json at all"),
        emit_blank(),
        respond(server_info_result()),
        expect("session/prompt"),
        respond(json!({"messageId": "msg-after-garbage"})),
    ])
    .expect("spawn fake runtime");

    initialize_ok(&mut rt).await;

    let message_id = rt
        .client
        .session_prompt("sess-1", vec![ContentBlock::Text { text: "hi".into() }])
        .await
        .expect("dispatch continues after garbage frames");
    assert_eq!(message_id, "msg-after-garbage");

    rt.client.close().await.expect("clean close");
}
