# deepseek-harness-sdk

English | [中文](README.zh.md)

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange)](Cargo.toml)
[![MSRV](https://img.shields.io/badge/MSRV-current%20stable-green)](Cargo.toml)

Rust client SDK for the [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
runtime stdio JSON-RPC 2.0 protocol: a low-level `HarnessClient` plus the
Python-parity high-level API (`DeepSeekHarness` / `Session::run` / `RunResult`).

The crate is the design twin of the official
[Python SDK](https://github.com/deepseek-ai/deepseek-harness/tree/master/python/sdk),
sharing the same runtime peer, wire protocol, and layering: `DeepSeekHarness`
is the high-level owned-run API, `HarnessClient` the lower-level protocol
client. The Python SDK surface is the alignment baseline for every type and
error that leaks into the public API; the TypeScript SDK's divergences are
documented (notably `RunResult`, see below).

This crate is a **pure client**. It contains no agent, LLM, or persistence
logic — the spawned runtime process does all of that. The runtime binary is
bring-your-own (Plan A): this crate never downloads, bundles, or ships one.

## Runtime acquisition

The runtime is bring-your-own (Plan A); the SDK only locates it. Two
acquisition routes exist:

1. **Build from source** — check out
   [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness), run
   `scripts/build-exe-for-python-sdk.ts` in the checkout (see that
   repository's build instructions), then point `DSH_RUNTIME_BIN` at the
   built executable.
2. **Install the runtime binary wheel** — the Python SDK's
   `deepseek-harness-runtime-bin` platform wheel ships the same single-file
   runtime executable; install the wheel for your platform and point
   `DSH_RUNTIME_BIN` at the shipped executable.

The binary is resolved with Python `HarnessClient` parity, plus the
Rust-only `DSH_RUNTIME_BIN` route:

1. `Config::launch_args_override` (non-empty) — the whole argv, verbatim;
2. `Config::runtime_bin`;
3. `DSH_RUNTIME_BIN` from the parent environment;
4. otherwise `Error::RuntimeNotFound`, whose message names both acquisition
   routes above (bring-your-own, and building the official runtime via
   `scripts/build-exe-for-python-sdk.ts`).

An empty `launch_args_override` and an empty `DSH_RUNTIME_BIN` both count as
absent (Python truthiness), so resolution never produces an unlaunchable
empty program.

With no effective `DSH_CORDIS_CONFIG`, `DeepSeekHarness::start` injects a
bundled copy of the runtime's default `cordis.yml` (byte-identical to the
official default), extracted to the system temp directory on first use and
byte-verified on every use. The runtime refuses to boot without an explicit
config, so this injection is required, not optional: a failure to extract or
verify the bundled default propagates as `Error::Io` — never a silent
config-less launch.

> **Deliberate divergence from the Python SDK** (documented; do not "fix" to
> match Python): the Python SDK injects its bundled default `cordis.yml`
> only when the bundled runtime carrier is used. This crate is bring-your-own
> runtime (Plan A) — there is no bundled carrier — so the bundled default is
> injected whenever no effective config exists, **regardless of how the
> runtime binary was resolved**.

## Quickstart

```rust
use deepseek_harness_sdk::{Config, DeepSeekHarness, Input};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut harness = DeepSeekHarness::start(Config {
        runtime_bin: std::env::var("DSH_RUNTIME_BIN").ok(),
        request_timeout: Some(Duration::from_secs(120)),
        ..Config::default()
    })
    .await?;

    let session = harness.start_session(None);
    let result = session
        .run(Input::Text("Reply with exactly: ok".into()))
        .await?;

    println!("finish_reason: {:?}", result.finish_reason);
    println!("final_response: {}", result.final_response);

    harness.close().await?;
    Ok(())
}
```

Prerequisites: a DeepSeek Harness runtime binary (see
[Runtime acquisition](#runtime-acquisition)) and `DEEPSEEK_API_KEY` (or
`Config::api_key` / `Config::base_url`). Like the other SDKs, the runtime
inherits `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` from the environment, so
callers can use real model endpoints directly or point those variables at a
local proxy.

## API walkthrough

### Layering

- `HarnessClient` (low-level): spawns the runtime process, owns the stdio
  transport, speaks the JSON-RPC 2.0 wire protocol, and fans notifications
  out to subscriptions. Exposes `LaunchSpec`, `ClientTimeouts`, and
  `NotificationSubscription`.
- `DeepSeekHarness` / `Session` (high-level): the Python-parity owned-run
  API on top of `HarnessClient`.
- `Input` accepts either plain text (`Input::Text`) or raw content blocks
  (`Input::Blocks`), mirroring Python's `normalize_input`.

### `DeepSeekHarness::start`

`start` is **eager**: it resolves the runtime, composes the environment
injection set, spawns the subprocess, and performs the `initialize`
handshake before returning. (This differs from the Python and TypeScript
SDKs, which start lazily on first use.) A failed handshake runs the close
ladder before the error propagates, so the spawned child is never leaked
(Python parity).

`Config::cwd` is resolved absolute (Python `Path(cwd).resolve()`) and feeds
both `DSH_CWD` and `initialize.cwd`; a nonexistent cwd fails with
`Error::Io`. `Config::request_timeout` bounds every request, including
`session/prompt`; `None` (the default) waits indefinitely.

Sessions created by `start_session` may run concurrently: the harness owns
the spawned child behind an async mutex, sessions interleave at the
`session/prompt` write, and each waits on its own subscription.

### `Session::run` — one activity interval

`run` implements the Python `Session.run` algorithm verbatim:

1. **Subscribe to the session tree before** writing the prompt, so no
   notification for this turn can be missed.
2. Send `session/prompt` (bounded by `Config::request_timeout`).
3. Wait for the durable `agent/inbox/spliced` receipt whose `inserted[].id`
   equals the returned message id (the field is `id`, **not** `messageId`);
   notifications before the receipt are dropped from both `events` and
   `notifications`.
4. Collect — from the receipt **inclusive** — every tree notification until
   the **root** session reports `session.status == "idle"` (that idle
   notification is collected too; a non-root idle never terminates the run).

`events` holds root-session `session.event` payloads only; `notifications`
holds every tree notification (root + discovered descendants, incl.
`session.status` / `subagent.*`) in transport order.

Both waits — the receipt wait and the idle wait — are **unbounded** (Python
parity); only the `session/prompt` request is bounded by
`Config::request_timeout`. Callers needing a bound wrap the call in
`tokio::time::timeout` — this bounds the local wait, not the runtime's turn.
A `session.event` / `session.status` notification whose payload does not
match the wire shape fails the run with `Error::SdkProtocol` (the Python SDK
raises; Rust surfaces the same condition as a typed error instead of
silently dropping an event or misreading the idle termination).

### `RunResult`

`RunResult` follows the **Python** SDK field set. The TypeScript SDK's
`RunResult` lacks `finish_reason` and `session_root`; Rust intentionally
follows Python, not TypeScript:

| Field | Python | TypeScript | Rust (this crate) |
|---|---|---|---|
| `session_id` / `sessionId` | yes | yes | `session_id: String` |
| `final_response` / `finalResponse` | yes | yes | `final_response: String` |
| `finish_reason` | yes (Python extension) | no | `finish_reason: Option<String>` |
| `events` | yes (root session only) | yes | `events: Vec<serde_json::Value>` |
| `notifications` | yes (root + descendants, transport order) | yes | `notifications: Vec<Notification>` |
| `session_root` | yes (Python extension) | no | `session_root: Option<PathBuf>` |

(The table is mirrored in the crate rustdoc, `# Compatibility`; keep the two
copies in sync.)

Both derived fields describe the owned activity interval rather than an
output causally assigned to the prompt: `final_response` is the last
committed root-session assistant text in the interval — steering, injected
context, and other queued work may contribute before idle — and
`finish_reason` is the `kind` of the last root-session `turn/end` in the
interval (such as `completed`, `max-tokens`, or `error`), `None` when no
turn ended. A `turn/end` without a string `data.reason.kind` violates the
runtime protocol and fails with `Error::SdkProtocol`.

### Typed errors

All failure paths return `Error` variants instead of ad-hoc strings:

| Variant | Meaning |
|---|---|
| `Error::RuntimeNotFound` | No runtime binary configured anywhere; message names the acquisition routes |
| `Error::TransportClosed` | Runtime process not running, or stdio closed unexpectedly; carries diagnostics (exit status and captured stderr tail) |
| `Error::RequestTimeout` | A request got no response within the configured timeout; carries the method name |
| `Error::SdkProtocol` | A protocol-level violation (missing server identity, missing `messageId`, `finish_reason` extraction failure, malformed notifications, subscription lag); `Error::is_protocol()` detects it |
| `Error::JsonRpc` | A JSON-RPC error response, preserving `code` (`Option<i64>`) and optional `data` |
| `Error::Io` / `Error::Json` | I/O (spawn, stdio, transport) and JSON serialization/deserialization errors |

### Close ladder

`DeepSeekHarness::close` (and `HarnessClient::close`) runs the plan-01
close ladder: a cooperative `shutdown` request bounded by
`shutdown_timeout` (default 1s, diagnostic only on failure) → drop stdin
(EOF) → wait `eof_grace` (default 6s — the runtime gets time to flush
durable state after stdin closes) → SIGTERM → wait `term_grace` (default
3s) → SIGKILL → wait. The ladder is idempotent, is unconditional teardown
(failure at any tier still reaps the child — the child is also killed on
drop, so a ladder failure cannot strand the process), and resolves all
pending requests with `Error::TransportClosed`.

### Notifications

Tree notifications travel a broadcast channel capped at 4096 with
drop-oldest semantics. If a high-volume tree floods more notifications than
fit between the SDK's reads, the dropped set can include the inbox receipt
or the root-idle notification a run depends on — rather than hang forever
or return a silently truncated result, `Session::run` then **fails fast**
with `Error::SdkProtocol`. A caller expecting very large bursts can bypass
the cap only via the low-level `HarnessClient::spawn_with_broadcast_capacity`
instead of `DeepSeekHarness::start`.

## Environment variables

The parent environment is inherited wholesale; the SDK injects or overrides
only the keys below (SDK keys win over caller-provided `Config::env`
entries — Python `dict.update` semantics):

| Variable | Role | Semantics |
|---|---|---|
| `DSH_RUNTIME_BIN` | Runtime binary resolution | Consulted when neither `Config::launch_args_override` nor `Config::runtime_bin` is set; empty counts as absent |
| `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` | Model endpoint and credentials | Inherited as-is; overridden only when `Config::base_url` / `Config::api_key` is configured |
| `DSH_CORDIS_CONFIG` | Runtime composition config | `Config::cordis_config` (non-empty) wins; otherwise a non-empty value from `Config::env` or the parent environment is inherited as-is. Empty strings count as absent — an empty-string `Config::env` entry is skipped on copy so it can never clobber a non-empty parent value. With no effective value, the SDK injects the bundled default `cordis.yml` |
| `DSH_CWD` | Agent working directory | Always injected, from `Config::cwd` (resolved absolute) |
| `DSH_SESSION_ROOT` | Session root | Injected only when `Config::session_root` is configured; surfaced on every `RunResult` |

## Testing

- `cargo test` — wire-protocol, lifecycle, and `Session::run` semantics
  suites against a scripted fake runtime (no real runtime needed).
- `tests/real_runtime.rs` — a single smoke test that runs **only** when both
  `DSH_RUNTIME_BIN` and `DEEPSEEK_API_KEY` are set; otherwise it prints an
  explicit skip notice and passes, so `cargo test` is green with no runtime
  binary and no credentials present.

## Platform support & MSRV

Consumed (not shipped) platforms — the runtime binary matrix: linux-x64,
linux-arm64, macos-arm64. **No Windows**: the runtime has no Windows
builds, so the SDK cannot support it.

MSRV: current stable Rust (no minimum is pinned in `Cargo.toml`; the crate
tracks the stable toolchain).

## Known limitations

- **No mid-turn cancel** — there is no session-close / cancel RPC on the
  wire. `Session::run` waits until the root session reports `idle`; closing
  the harness mid-turn abandons the in-flight turn. A `Config::request_timeout`
  only abandons the local wait — the server-side work still runs until
  close.
- **No version negotiation** — the runtime identifies as `serverInfo` 0.0.1
  pre-release, and `initialize` enforces a strict `serverInfo.name` check
  (`deepseek-harness-sdk-runtime`) with `version` required: the protocol
  declares the name wire-stable and has no negotiation, so an unexpected
  identity is a hard `Error::SdkProtocol`.
- **No runtime binary delivery / bundling / download** — the runtime
  companion crate and crates.io publish are roadmap items, not part of this
  version. Acquire the runtime per
  [Runtime acquisition](#runtime-acquisition).
- **No TypeScript-parity helper** — the TS-shaped `RunResult` (without
  `finish_reason` / `session_root`) is not provided.

## License

Apache-2.0.
