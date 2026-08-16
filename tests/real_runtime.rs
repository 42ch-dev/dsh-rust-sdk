//! Real-runtime smoke test, gated on `DSH_RUNTIME_BIN` + `DEEPSEEK_API_KEY`.
//!
//! This test exercises the full Python-parity stack — `DeepSeekHarness::start`
//! → `Session::run` — against a **real** DeepSeek Harness runtime binary
//! (bring-your-own; see <https://github.com/deepseek-ai/deepseek-harness>).
//!
//! It is skipped (with an explicit notice) when either environment variable is
//! absent, so `cargo test` stays green on machines without a runtime binary
//! and without credentials. Gating uses `std::env::var` at test start, never
//! the compile-time `env!` macro — `env!` would break builds where the
//! variables are unset.

use std::time::Duration;

use deepseek_harness_sdk::{Config, DeepSeekHarness, Input};

/// One smoke turn: start a harness with a temp `session_root` and default
/// config, run `Session::run`, and assert **structural** facts only (LLM
/// output is nondeterministic): a success-class `finish_reason`
/// (`completed`/`max-tokens`), a non-empty `final_response`, the session ids
/// present, and the configured `session_root` surfaced.
#[tokio::test]
async fn real_runtime_smoke() {
    // Runtime gating — read at runtime, not at compile time.
    let runtime_bin = std::env::var("DSH_RUNTIME_BIN")
        .ok()
        .filter(|bin| !bin.trim().is_empty());
    let Some(runtime_bin) = runtime_bin else {
        eprintln!(
            "skipping real-runtime smoke: DSH_RUNTIME_BIN is unset or empty; \
             set it to a DeepSeek Harness runtime binary \
             (https://github.com/deepseek-ai/deepseek-harness) to run this test"
        );
        return;
    };

    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty());
    let Some(api_key) = api_key else {
        eprintln!(
            "skipping real-runtime smoke: DEEPSEEK_API_KEY is unset or empty; \
             set it to run one live LLM turn"
        );
        return;
    };

    // A unique temp session root so repeated runs never reuse stale session
    // state (process id + monotonic nanos; no extra dependency needed).
    let session_root = std::env::temp_dir().join(format!(
        "dsh-sdk-real-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&session_root).expect("create temp session root");

    let mut harness = DeepSeekHarness::start(Config {
        runtime_bin: Some(runtime_bin),
        api_key: Some(api_key),
        session_root: Some(session_root.to_string_lossy().into_owned()),
        // Bound the wire requests so a wedged runtime fails fast instead of
        // hanging the suite; the activity interval itself is unbounded
        // (Python parity) and is bounded below by the outer timeout.
        request_timeout: Some(Duration::from_secs(120)),
        ..Config::default()
    })
    .await
    .expect("harness starts against the real runtime");

    let result = tokio::time::timeout(
        Duration::from_secs(600),
        harness
            .start_session(None)
            .run(Input::Text("Reply with exactly: ok".into())),
    )
    .await
    .expect("real-runtime turn completes within the smoke timeout")
    .expect("Session::run succeeds against the real runtime");

    harness.close().await.expect("clean close");

    // Structural facts only — no assertion on the response text (LLM
    // nondeterminism). The finish reason must be a success-class kind: a
    // `turn/end` with kind "error" would not count as a completed turn, and
    // `is_some()` alone would pass an error-class end.
    assert!(
        matches!(
            result.finish_reason.as_deref(),
            Some("completed" | "max-tokens")
        ),
        "expected a success-class finish_reason (completed/max-tokens), got {:?}",
        result.finish_reason
    );
    assert!(
        !result.final_response.is_empty(),
        "expected a non-empty final_response"
    );
    assert!(!result.session_id.is_empty(), "session id present");
    assert_eq!(
        result.session_root.as_deref(),
        Some(session_root.as_path()),
        "configured session_root is surfaced on the RunResult"
    );

    let response = &result.final_response;
    let preview: String = response.chars().take(200).collect();
    println!(
        "real-runtime smoke ok: session_id={} finish_reason={:?} \
         final_response (first 200 chars)={:?}",
        result.session_id, result.finish_reason, preview
    );
}
