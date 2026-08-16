//! Script directives shared by the `FakeRuntime` test harness and the
//! `fake-runtime` fixture binary.
//!
//! One [`Directive`] is one scripted step for the fake stdio JSON-RPC peer:
//! wait for a request, answer it, emit a notification, emit garbage, sleep,
//! or exit. The harness builds a `Vec<Directive>` and serializes it to the
//! peer (passed as its sole argv argument); the peer interprets it against
//! the real client's request stream.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One scripted step for the fake-runtime peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Directive {
    /// Wait for the next client request with this exact method. Fails the
    /// peer when the method differs. When `params_contains` is set, every
    /// key of that object must be present in the request's `params` with an
    /// equal value (deep subset check — locks the client's wire output).
    Expect {
        method: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        params_contains: Option<Value>,
    },
    /// Respond to the most recent expected request with a success result.
    Respond { result: Value },
    /// Respond to the most recent expected request with a JSON-RPC error.
    RespondError {
        code: i64,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    /// Emit a server-to-client notification.
    Emit { method: String, params: Value },
    /// Emit one raw line on stdout that is not valid JSON (malformed-peer
    /// line tolerance).
    EmitRaw { line: String },
    /// Write one line to the peer's stderr (exercises the client's stderr
    /// capture diagnostics).
    EmitStderr { line: String },
    /// Emit one blank line on stdout.
    EmitBlank,
    /// Read and discard every further client line without ever responding,
    /// until the client closes stdin, then exit 0. Used by the
    /// request-timeout scenario.
    IgnoreAll,
    /// Read one line from the client and assert it deep-equals `frame`
    /// (asserts the client's wire output — e.g. the `-32601` auto-respond to
    /// a client-directed request). Exits 2 on mismatch.
    ExpectFrame { frame: Value },
    /// Sleep, keeping the peer (and its stdout) alive.
    SleepMs { ms: u64 },
    /// Exit with the given code.
    Exit { code: i32 },
}
