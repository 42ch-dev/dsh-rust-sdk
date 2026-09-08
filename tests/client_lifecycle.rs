//! End-to-end client lifecycle tests against the scripted `fake-runtime`
//! stdio JSON-RPC peer (harness: `tests/common/fake_runtime.rs`; peer binary:
//! `tests/fixtures/fake_runtime.rs`). One `#[tokio::test]` per plan scenario.
//!
//! No external DSH checkout or network is involved: the peer is compiled
//! from this crate's own fixtures and spawned as a subprocess.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use deepseek_harness_sdk::{
    ClientTimeouts, Config, ContentBlock, DeepSeekHarness, Error, HarnessClient, LaunchSpec,
};
use serde_json::{json, Value};
use uuid::Uuid;

use common::fake_runtime::{
    emit, emit_blank, emit_raw, emit_stderr, env_dump_bin, exit, expect, expect_frame,
    expect_params, fake_runtime_path, fake_runtime_spec, harness_config, ignore_all, respond,
    respond_error, server_info_result, sleep_forever_bin, sleep_ms, test_temp_root, test_timeouts,
    FakeRuntime,
};

/// The canonical client-side session ids used across scenarios.
const ROOT_SESSION: &str = "root";
const CHILD_SESSION: &str = "child";
const UNRELATED_SESSION: &str = "unrelated";

async fn initialize_ok(rt: &mut FakeRuntime) {
    rt.client
        .initialize("/tmp", "deepseek", "deepseek-chat", None, Some(1024))
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
                "reasoningEffort": "high",
                "maxTokens": 1024,
            }),
        ),
        respond(server_info_result()),
    ])
    .expect("spawn fake runtime");

    let result = rt
        .client
        .initialize(
            "/tmp",
            "deepseek",
            "deepseek-chat",
            Some("high"),
            Some(1024),
        )
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
        .initialize("/tmp", "deepseek", "deepseek-chat", None, Some(1024))
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
    // Deterministic by construction (FIX-4): no wall-clock window. The peer
    // emits the pre-edge child event, then a root "sentinel" event that
    // follows it in transport order — receiving the sentinel proves the
    // filter already consumed and dropped the pre-edge event while the child
    // was not yet in the tree. The peer then blocks on a marker
    // `session/prompt`; only after the test sends it does the peer emit the
    // `subagent.started` edge and the post-edge event, so the ordering is
    // enforced by the wire, never by sleep timing.
    let mut rt = FakeRuntime::spawn(&[
        expect("initialize"),
        respond(server_info_result()),
        emit(
            "session.event",
            json!({"sessionId": CHILD_SESSION, "event": {"type": "test", "text": "before-edge"}}),
        ),
        emit(
            "session.event",
            json!({"sessionId": ROOT_SESSION, "event": {"type": "test", "text": "sentinel"}}),
        ),
        // Marker: the peer answers it, and only then emits the edge and the
        // post-edge event.
        expect("session/prompt"),
        respond(json!({"messageId": "marker-1"})),
        emit(
            "subagent.started",
            json!({"parentSessionId": ROOT_SESSION, "childSessionId": CHILD_SESSION}),
        ),
        emit(
            "session.event",
            json!({"sessionId": CHILD_SESSION, "event": {"type": "test", "text": "after-edge"}}),
        ),
        // Keep the peer alive for the final no-more-events probe.
        sleep_ms(1000),
    ])
    .expect("spawn fake runtime");

    let mut subscription = rt.client.subscribe_session_tree(ROOT_SESSION);
    initialize_ok(&mut rt).await;

    // The root sentinel follows the pre-edge child event in transport order;
    // receiving it proves the pre-edge event was drained and dropped by the
    // filter while the edge map was still empty (the peer is still blocked
    // on the marker below).
    let sentinel = subscription.recv().await.expect("root sentinel delivered");
    assert_eq!(sentinel.method, "session.event");
    assert_eq!(
        sentinel
            .payload
            .get("event")
            .and_then(|e| e.get("text"))
            .and_then(Value::as_str),
        Some("sentinel")
    );

    // Unblock the peer; the edge and the post-edge event now arrive in wire
    // order.
    let message_id = rt
        .client
        .session_prompt(
            "sess-1",
            vec![ContentBlock::Text {
                text: "sync".into(),
            }],
        )
        .await
        .expect("marker prompt answered");
    assert_eq!(message_id, "marker-1");

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
        after_edge.payload.get("sessionId").and_then(Value::as_str),
        Some(CHILD_SESSION)
    );
    assert_eq!(
        after_edge
            .payload
            .get("event")
            .and_then(|e| e.get("text"))
            .and_then(Value::as_str),
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
async fn spontaneous_death_surfaces_exit_code_and_stderr_tail() {
    // The canonical crash scenario: the runtime dies (exit 101, a Rust
    // panic-like code) while a request is in flight. The read loop's EOF
    // path must attach the exit code and the captured stderr tail to the
    // pending request's TransportClosed error (plan contract), not just
    // the tail.
    let mut rt = FakeRuntime::spawn(&[
        expect("session/prompt"),
        emit_stderr("fatal: runtime panicked"),
        exit(101),
    ])
    .expect("spawn fake runtime");

    let err = rt
        .client
        .session_prompt("sess-1", vec![ContentBlock::Text { text: "hi".into() }])
        .await
        .expect_err("a request pending at runtime death must fail");
    match &err {
        Error::TransportClosed(message) => {
            assert!(
                message.contains("exit code: 101"),
                "EOF-path error must carry the exit code, got: {message}"
            );
            assert!(
                message.contains("fatal: runtime panicked"),
                "EOF-path error must carry the stderr tail, got: {message}"
            );
        }
        other => panic!("expected TransportClosed, got {other:?}"),
    }

    rt.client.close().await.expect("close after death");
}

#[tokio::test]
async fn client_answers_client_directed_requests_with_method_not_found() {
    // FIX-9: the defensive read-loop path — the peer emits a client-directed
    // request and asserts the client answers -32601 with the echoed id, for
    // numeric and string id forms. The peer emits a wire-visible completion
    // marker (a root-tree session.event) only after both responses were
    // verified; the test waits for it before sending any request of its own,
    // so the peer's expect_frames can never read a stray session/prompt line.
    let mut rt = FakeRuntime::spawn(&[
        emit_raw(r#"{"jsonrpc":"2.0","id":9,"method":"some.request","params":{"a":1}}"#),
        expect_frame(json!({
            "jsonrpc": "2.0",
            "id": 9,
            "error": {"code": -32601, "message": "method not found: some.request"}
        })),
        emit_raw(r#"{"jsonrpc":"2.0","id":"probe-1","method":"another.request"}"#),
        expect_frame(json!({
            "jsonrpc": "2.0",
            "id": "probe-1",
            "error": {"code": -32601, "message": "method not found: another.request"}
        })),
        emit(
            "session.event",
            json!({"sessionId": ROOT_SESSION, "event": {"type": "test", "text": "exchange-done"}}),
        ),
        expect("session/prompt"),
        respond(json!({"messageId": "probe-ok"})),
        exit(0),
    ])
    .expect("spawn fake runtime");

    let mut subscription = rt.client.subscribe_session_tree(ROOT_SESSION);

    // The marker proves both -32601 exchanges completed (the peer exits 2 on
    // any response mismatch, so it would never reach the marker).
    let marker = subscription
        .recv()
        .await
        .expect("exchange-done marker delivered");
    assert_eq!(
        marker
            .payload
            .get("event")
            .and_then(|e| e.get("text"))
            .and_then(Value::as_str),
        Some("exchange-done")
    );

    let message_id = rt
        .client
        .session_prompt("sess-1", vec![ContentBlock::Text { text: "hi".into() }])
        .await
        .expect("the peer must survive the -32601 exchange to answer this");
    assert_eq!(message_id, "probe-ok");

    rt.client.close().await.expect("clean close");
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
        program: PathBuf::from(sleep_forever_bin()),
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

    // Missing program: ENOENT -> Io. A spawn failure is an I/O error
    // (spec §7); `Error::RuntimeNotFound` is reserved for "no runtime could
    // be resolved" (spec §8), which `resolve_runtime` reports before spawn.
    let spec = LaunchSpec {
        program: PathBuf::from("/definitely/not/a/deepseek/runtime"),
        args: vec![],
        envs: Default::default(),
        cwd: None,
    };
    let err = HarnessClient::spawn(spec, timeouts).expect_err("missing program must fail");
    assert!(matches!(err, Error::Io(_)), "unexpected error: {err}");

    // A program that exists but cannot be launched (a directory is not
    // executable) is a plain spawn I/O error, not a NotFound.
    let spec = LaunchSpec {
        program: std::env::temp_dir(),
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

#[tokio::test]
async fn start_creates_missing_configured_home_before_launch() {
    // Spec §3.2.5: the resolved harness home is created when absent so a
    // fresh home boots. The configured home is a unique temp dir that does
    // not exist yet — the real ~/.dsh is never touched.
    let config = harness_config(&[
        expect_params(
            "initialize",
            json!({
                "provider": "deepseek-official",
                "model": "deepseek-v4-flash",
            }),
        ),
        respond(server_info_result()),
    ])
    .expect("serialize script");
    let home = config.dsh_home.clone().expect("temp home configured");
    assert!(
        !home.exists(),
        "precondition: the temp home must not exist before start"
    );
    let mut harness = DeepSeekHarness::start(config)
        .await
        .expect("harness starts");
    assert!(
        home.is_dir(),
        "the resolved home must be created before the child is launched"
    );
    assert_eq!(
        harness.dsh_home(),
        home.as_path(),
        "the instance accessor must expose the resolved home the harness created and injected"
    );
    harness.close().await.expect("clean close");
}

#[tokio::test]
async fn spawn_strips_forbidden_keys_from_inherited_parent_env() {
    // Spec §4.2 / AC1: the child env carries no DSH_CORDIS_CONFIG /
    // DSH_SESSION_ROOT / DSH_CWD under any configuration. The override set
    // filters them from Config::env, and the spawn layer strips them from
    // the inherited parent env — so even a parent that still exports the
    // v0.1 keys cannot leak them into the runtime child.
    let output = test_temp_root().join(format!("env-dump-{}.txt", Uuid::new_v4()));
    // The three keys are never read by the crate, so mutating the process
    // env here cannot affect sibling tests; the guard restores it on drop.
    let _guard = ForbiddenEnvGuard;
    std::env::set_var("DSH_CORDIS_CONFIG", "/parent/cordis.yml");
    std::env::set_var("DSH_SESSION_ROOT", "/parent/sessions");
    std::env::set_var("DSH_CWD", "/parent/cwd");
    let spec = LaunchSpec {
        program: PathBuf::from(env_dump_bin()),
        args: vec![
            OsString::from(&output),
            OsString::from("DSH_CORDIS_CONFIG"),
            OsString::from("DSH_SESSION_ROOT"),
            OsString::from("DSH_CWD"),
            OsString::from("PATH"),
        ],
        envs: Default::default(),
        cwd: None,
    };
    let mut client = HarnessClient::spawn(spec, test_timeouts()).expect("spawn env-dump");
    // The fixture writes the dump synchronously at startup; poll for it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !output.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the env dump never appeared"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let dump = std::fs::read_to_string(&output).expect("read the env dump");
    for forbidden in ["DSH_CORDIS_CONFIG", "DSH_SESSION_ROOT", "DSH_CWD"] {
        assert!(
            !dump
                .lines()
                .any(|line| line.starts_with(&format!("{forbidden}="))),
            "{forbidden} must be stripped from the child env: {dump}"
        );
    }
    assert!(
        dump.lines().any(|line| line.starts_with("PATH=")),
        "the ordinary parent env must still be inherited (no env_clear): {dump}"
    );
    client.close().await.expect("clean close");
}

#[tokio::test]
async fn home_creation_failure_surfaces_as_io_before_spawn() {
    // Spec §3.2.5: a create_dir_all failure surfaces as Error::Io before
    // the child is spawned (no child leak). Point dsh_home at a path under
    // a regular file so mkdir fails with NotADirectory — a kind only the
    // home-creation step can produce (a spawn failure would be NotFound or
    // PermissionDenied), pinning that the error precedes HarnessClient::spawn.
    let blocker = test_temp_root().join(format!("home-blocker-{}", Uuid::new_v4()));
    std::fs::write(&blocker, "").expect("create the blocker file");
    let config = Config {
        dsh_bin: Some(fake_runtime_path().to_string()),
        dsh_home: Some(blocker.join("home")),
        timeouts: test_timeouts(),
        ..Config::default()
    };
    let err = DeepSeekHarness::start(config)
        .await
        .expect_err("home creation under a regular file must fail");
    assert!(
        matches!(&err, Error::Io(io) if io.kind() == std::io::ErrorKind::NotADirectory),
        "expected Error::Io(NotADirectory) from create_dir_all before spawn, got: {err}"
    );
}

/// Restores the parent env after the forbidden-key spawn test. The three
/// keys are never read by the crate, so leaving them set would be benign,
/// but the test restores them anyway to keep the process env pristine.
struct ForbiddenEnvGuard;

impl Drop for ForbiddenEnvGuard {
    fn drop(&mut self) {
        for key in ["DSH_CORDIS_CONFIG", "DSH_SESSION_ROOT", "DSH_CWD"] {
            std::env::remove_var(key);
        }
    }
}
