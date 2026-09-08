//! Real-runtime tests against a **real** DeepSeek Harness runtime binary
//! (bring-your-own; see <https://github.com/deepseek-ai/deepseek-harness>).
//!
//! Two tiers:
//!
//! 1. `real_runtime_handshake` — **keyless**: resolves a `dsh` binary from
//!    `DSH_RUNTIME_BIN` or `dsh` on `PATH`, boots it with a temp `dsh_home`,
//!    and proves `start()` completes the `initialize` handshake and `close()`
//!    reaps the child. No API key, no `Session::run`. This proves boot +
//!    `initialize` + `close` only (launch spec §9 item 4) — it is **not**
//!    end-to-end proof of a live turn.
//! 2. `real_runtime_smoke` — one live LLM turn, gated on `DEEPSEEK_API_KEY`
//!    (and a resolvable runtime). Structural assertions only.
//!
//! Both tiers skip (with an explicit notice) when their prerequisites are
//! absent, so `cargo test` stays green on machines without a runtime binary
//! and without credentials. Gating uses `std::env::var` at test start, never
//! the compile-time `env!` macro — `env!` would break builds where the
//! variables are unset.

use std::path::PathBuf;
use std::time::Duration;

use deepseek_harness_sdk::{Config, DeepSeekHarness, Input};

mod common;

/// Best-effort probe of the resolved runtime's own version (`dsh --version`),
/// so the notice reports the version the test actually ran against instead of
/// a hard-coded literal. Returns `None` when the probe fails (non-zero exit,
/// empty or non-UTF-8 output) — the notice then degrades to "version unknown"
/// and the test itself is never blocked by the probe.
fn probe_dsh_version(runtime_bin: &str) -> Option<String> {
    let output = std::process::Command::new(runtime_bin)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!version.is_empty()).then_some(version)
}

/// Resolve the runtime binary: `DSH_RUNTIME_BIN` (non-empty) first, then
/// `dsh` on `PATH`. Returns `None` when neither exists, so the caller can
/// skip cleanly instead of failing.
fn resolve_runtime_bin() -> Option<String> {
    if let Some(bin) = std::env::var("DSH_RUNTIME_BIN")
        .ok()
        .filter(|bin| !bin.trim().is_empty())
    {
        return Some(bin);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["dsh", "dsh.exe"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// A unique temp harness home so repeated runs never reuse stale session
/// state (process id + monotonic nanos; no extra dependency needed), under
/// the per-run test temp root so the suite's artifacts stay consolidated
/// and bounded (F6). `DeepSeekHarness::start` creates the home at boot
/// (launch spec §3.2.5).
fn temp_dsh_home() -> PathBuf {
    common::fake_runtime::test_temp_root().join(format!(
        "dsh-sdk-real-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos()
    ))
}

/// Keyless boot tier: resolve a `dsh` binary, start a harness with a temp
/// `dsh_home`, and assert the `initialize` handshake succeeded, then close.
///
/// No API key and no `Session::run` — this proves boot + `initialize` +
/// `close` only (launch spec §9 item 4), not end-to-end function.
#[tokio::test]
async fn real_runtime_handshake() {
    let Some(runtime_bin) = resolve_runtime_bin() else {
        eprintln!(
            "skipping real-runtime handshake: no dsh binary found; set DSH_RUNTIME_BIN \
             or put dsh on PATH (https://github.com/deepseek-ai/deepseek-harness) to run \
             this test (validated locally against a real dsh runtime)"
        );
        return;
    };

    let dsh_home = temp_dsh_home();
    let mut harness = DeepSeekHarness::start(Config {
        dsh_bin: Some(runtime_bin.clone()),
        dsh_home: Some(dsh_home.clone()),
        ..Config::default()
    })
    .await
    .expect("harness starts against the real runtime: initialize handshake succeeded");

    harness.close().await.expect("clean close");

    println!(
        "real-runtime handshake ok: dsh={runtime_bin} dsh_home={} (dsh {})",
        dsh_home.display(),
        probe_dsh_version(&runtime_bin).unwrap_or_else(|| "version unknown".into())
    );
}

/// One smoke turn: start a harness with a temp `dsh_home` and default
/// config, run `Session::run`, and assert **structural** facts only (LLM
/// output is nondeterministic): a success-class `finish_reason`
/// (`completed`/`max-tokens`), a non-empty `final_response`, and the
/// session id present.
///
/// Gated on `DEEPSEEK_API_KEY` (and a resolvable runtime, shared with the
/// keyless tier); skipped with an explicit notice when either is absent.
#[tokio::test]
async fn real_runtime_smoke() {
    let Some(runtime_bin) = resolve_runtime_bin() else {
        eprintln!(
            "skipping real-runtime smoke: no dsh binary found; set DSH_RUNTIME_BIN \
             or put dsh on PATH (https://github.com/deepseek-ai/deepseek-harness) to run \
             this test (validated locally against a real dsh runtime)"
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

    let dsh_home = temp_dsh_home();
    let mut harness = DeepSeekHarness::start(Config {
        dsh_bin: Some(runtime_bin),
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
