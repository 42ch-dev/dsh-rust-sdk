//! Reusable `FakeRuntime` scenario harness: spawns the `fake-runtime` fixture
//! binary (a scripted stdio JSON-RPC peer, see `tests/fixtures/fake_runtime.rs`)
//! as a stand-in for the DSH runtime and drives it with [`Directive`] scripts.
//!
//! Consumed by this plan's integration suite (`tests/client_lifecycle.rs`)
//! and by plan 02's `run()` semantics tests — the peer answers real client
//! requests, so the same harness serves any client-level scenario.

use std::collections::HashMap;
use std::time::Duration;

use deepseek_harness_sdk::{ClientTimeouts, Error, HarnessClient, LaunchSpec};

#[path = "directive.rs"]
mod directive;
pub use directive::Directive;
use serde_json::json;

/// Short ladder timeouts for the integration suite: every close-ladder tier
/// resolves in milliseconds, so escalation scenarios run fast and the
/// request-timeout scenario needs only a short `request_timeout` override.
pub fn test_timeouts() -> ClientTimeouts {
    ClientTimeouts {
        request_timeout: None,
        shutdown_timeout: Duration::from_millis(200),
        eof_grace: Duration::from_millis(300),
        term_grace: Duration::from_millis(300),
    }
}

/// Absolute path to the `fake-runtime` fixture binary. Cargo sets
/// `CARGO_BIN_EXE_<name>` for integration tests at compile time.
pub fn fake_runtime_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fake-runtime")
}

/// Absolute path to the `sleep-forever` fixture binary.
pub fn sleep_forever_bin() -> &'static str {
    env!("CARGO_BIN_EXE_sleep-forever")
}

/// A [`LaunchSpec`] running the fake-runtime peer against `script`.
pub fn fake_runtime_spec(script: &[Directive]) -> Result<LaunchSpec, serde_json::Error> {
    let script = serde_json::to_string(script)?;
    Ok(LaunchSpec {
        program: fake_runtime_bin().to_string(),
        args: vec![script],
        envs: HashMap::new(),
        cwd: None,
    })
}

/// A spawned fake-runtime client with default test timeouts.
pub struct FakeRuntime {
    /// The live client; tests drive it directly.
    pub client: HarnessClient,
}

impl FakeRuntime {
    /// Spawn the fake runtime with `script` and default test timeouts.
    pub fn spawn(script: &[Directive]) -> Result<Self, Error> {
        let client = HarnessClient::spawn(fake_runtime_spec(script)?, test_timeouts())?;
        Ok(Self { client })
    }
}

/// The canonical `initialize` success result served by happy-path scripts:
/// the wire-stable identity the client validates against.
pub fn server_info_result() -> serde_json::Value {
    json!({
        "serverInfo": {
            "name": "deepseek-harness-sdk-runtime",
            "version": "0.0.1",
        }
    })
}

// Convenience constructors so scenario scripts read as plain statements.
pub fn expect(method: &str) -> Directive {
    Directive::Expect {
        method: method.to_string(),
        params_contains: None,
    }
}

pub fn expect_params(method: &str, params: serde_json::Value) -> Directive {
    Directive::Expect {
        method: method.to_string(),
        params_contains: Some(params),
    }
}

pub fn respond(result: serde_json::Value) -> Directive {
    Directive::Respond { result }
}

pub fn respond_error(code: i64, message: &str, data: Option<serde_json::Value>) -> Directive {
    Directive::RespondError {
        code,
        message: message.to_string(),
        data,
    }
}

pub fn emit(method: &str, params: serde_json::Value) -> Directive {
    Directive::Emit {
        method: method.to_string(),
        params,
    }
}

pub fn emit_raw(line: &str) -> Directive {
    Directive::EmitRaw {
        line: line.to_string(),
    }
}

pub fn emit_blank() -> Directive {
    Directive::EmitBlank
}

pub fn ignore_all() -> Directive {
    Directive::IgnoreAll
}

pub fn sleep_ms(ms: u64) -> Directive {
    Directive::SleepMs { ms }
}

pub fn exit(code: i32) -> Directive {
    Directive::Exit { code }
}
