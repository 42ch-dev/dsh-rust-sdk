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

mod common;

/// One smoke turn: start a harness with a temp `dsh_home` and default
/// config, run `Session::run`, and assert **structural** facts only (LLM
/// output is nondeterministic): a success-class `finish_reason`
/// (`completed`/`max-tokens`), a non-empty `final_response`, and the
/// session id present.
#[tokio::test]
async fn real_runtime_smoke() {
    // Runtime gating — read at runtime, not at compile time.
    let runtime_path = std::env::var("DSH_RUNTIME_BIN")
        .ok()
        .filter(|bin| !bin.trim().is_empty());
    let Some(runtime_path) = runtime_path else {
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

    // A unique temp harness home so repeated runs never reuse stale session
    // state (process id + monotonic nanos; no extra dependency needed),
    // under the per-run test temp root so the suite's artifacts stay
    // consolidated and bounded (F6).
    let dsh_home = common::fake_runtime::test_temp_root().join(format!(
        "dsh-sdk-real-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dsh_home).expect("create temp harness home");

    let mut harness = DeepSeekHarness::start(Config {
        dsh_bin: Some(runtime_path),
        api_key: Some(api_key),
        dsh_home: Some(dsh_home.clone()),
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
            .run(Input::Text("Reply with exactly: ok".into()), None),
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

    let response = &result.final_response;
    let preview: String = response.chars().take(200).collect();
    println!(
        "real-runtime smoke ok: session_id={} finish_reason={:?} \
         final_response (first 200 chars)={:?}",
        result.session_id, result.finish_reason, preview
    );
}
