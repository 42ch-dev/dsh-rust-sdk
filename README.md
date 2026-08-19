# deepseek-harness-sdk

English | [中文](README.zh.md)

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange)](Cargo.toml)
[![crates.io](https://img.shields.io/crates/v/deepseek-harness-sdk)](https://crates.io/crates/deepseek-harness-sdk)

See the [CHANGELOG](CHANGELOG.md) for the release history.

Rust client SDK for the [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
(DSH) runtime: it spawns the official runtime as a subprocess and speaks its
stdio JSON-RPC 2.0 protocol. One crate, two layers: the high-level
Python-parity API (`DeepSeekHarness` / `Session::run` / `RunResult`) and the
low-level protocol client (`HarnessClient`).

The crate is the design twin of the official
[Python SDK](https://github.com/deepseek-ai/deepseek-harness/tree/master/python/sdk),
sharing the same runtime peer, wire protocol, and layering; the Python SDK
surface is the alignment baseline for every public type and error. The
TypeScript SDK's divergences are documented (notably `RunResult`, see below).

This crate is a **pure client**. It contains no agent, LLM, or persistence
logic — the spawned runtime process does all of that. The runtime binary is
bring-your-own: this crate never downloads, bundles, or ships one.

## Installation

```sh
cargo add deepseek-harness-sdk
```

or in `Cargo.toml`:

```toml
[dependencies]
deepseek-harness-sdk = "*"
```

Pick the version that suits you (`cargo search deepseek-harness-sdk` or the
[crates.io page](https://crates.io/crates/deepseek-harness-sdk) shows the
latest). While the crate is on a pre-release line, a bare
`cargo add deepseek-harness-sdk` may not resolve to the newest pre-release —
request it explicitly (e.g. `cargo add deepseek-harness-sdk@0.1.0-alpha`) when
you want it. The API may still change before `0.1.0`.

Two prerequisites before the first run: a DSH runtime (see
[Runtime acquisition](#runtime-acquisition)) and model credentials
(`DEEPSEEK_API_KEY` in the environment, or `Config::api_key` /
`Config::base_url`).

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

`DeepSeekHarness::start` is eager: it resolves the runtime, spawns the
subprocess, and completes the `initialize` handshake before returning. Like
the other SDKs, the runtime inherits `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY`
from the environment, so callers can use real model endpoints directly or
point those variables at a local proxy.

## Runtime acquisition

The runtime is bring-your-own; the SDK only locates it. One distinction
first: the interactive `dsh` CLI (`@deepseek-ai/dsh` — the program you may
know from `npx` / global installs) is **not** this SDK's runtime; it does not
serve the stdio JSON-RPC protocol the SDK speaks. The SDK needs the headless
JSON-RPC runtime (`dsh-jsonrpc-agent`) — the same runtime carrier the
official Python SDK bundles. Upstream ships it two ways: a self-contained
single-file executable (via the Python SDK's platform wheel) and an
npm-distributed Node.js program. Pick one route:

### Route A — prebuilt executable (default)

Upstream packs the runtime as a self-contained Node.js single-file
executable (no Node.js needed at runtime; plugin tree embedded) and
distributes it through the Python SDK's `deepseek-harness-runtime-bin`
platform wheel (linux-x64, linux-arm64, macos-arm64):

```sh
python -m pip install deepseek-harness-runtime-bin
export DSH_RUNTIME_BIN="$(python -c 'import deepseek_harness_runtime as r; print(r.bundled_runtime_path())')"
```

The `python -c` invocation only *locates* the installed executable and prints
its path — **no Python runs at SDK runtime**. On macOS the executable needs
its sibling `-spawn-helper` file in the same directory (the wheel installs
both) — if you copy the executable elsewhere, copy the helper too. The wheel also ships a matching ripgrep `-rg` sidecar beside the executable — the bundled default config does not use it, but a config that mounts the fs-search tool needs it there too (copy it along when relocating the executable).

This route works out of the box with the SDK-injected bundled default
`cordis.yml` (see below): no `DSH_CORDIS_CONFIG` needed.

### Route B — npm (no-build)

The runtime bin is published as
[`@deepseek-ai/dsh-sdk-jsonrpc-demo`](https://www.npmjs.com/package/@deepseek-ai/dsh-sdk-jsonrpc-demo)
(bin: `dsh-jsonrpc-agent`). Requires Node.js ≥ 22.19. Install from the
**`next` dist-tag**: the `latest` tags of these packages currently point at an
older, mixed version matrix, while `next` resolves the whole set to one
coherent release line (the line the interactive `dsh` CLI ships on). Run it
either way:

- **`npx` (no install)** — `npx` itself is the program, so use
  `Config::launch_args_override`:

  ```rust
  Config {
      launch_args_override: Some(vec![
          "npx".into(),
          "--yes".into(),
          "@deepseek-ai/dsh-sdk-jsonrpc-demo@next".into(),
      ]),
      ..Config::default()
  }
  ```

- **`npm install -g`** — the bin lands on `PATH`:

  ```sh
  npm install -g @deepseek-ai/dsh-sdk-jsonrpc-demo@next
  export DSH_RUNTIME_BIN=dsh-jsonrpc-agent
  ```

Either way, the npm bin resolves the plugins named in `cordis.yml` from the
**config project** — the directory the config file lives in — so this route
also needs a small config project with the plugin set installed:

```sh
mkdir dsh-runtime && cd dsh-runtime
npm init -y >/dev/null
npm install @deepseek-ai/dsh-sdk-jsonrpc-server@next @deepseek-ai/dsh-agent-spine-demo@next \
  @deepseek-ai/dsh-llm-deepseek@next @deepseek-ai/dsh-session-persistence-jsonl@next \
  @deepseek-ai/dsh-session-checkpoint-policy@next @deepseek-ai/dsh-subprocess-local@next \
  @deepseek-ai/dsh-bash-local@next @deepseek-ai/dsh-fs-local@next
# Drop the default cordis.yml next to package.json (see below), then:
export DSH_CORDIS_CONFIG="$PWD/cordis.yml"
```

For step 3, use the upstream default config —
[`python/sdk-runtime/src/deepseek_harness_runtime/runtime/cordis.yml`](https://github.com/deepseek-ai/deepseek-harness/blob/master/python/sdk-runtime/src/deepseek_harness_runtime/runtime/cordis.yml)
in the DSH repository — or compose your own (keep the
`@deepseek-ai/dsh-sdk-jsonrpc-server` entry; without it the runtime serves
nothing).

> **npm route caveat:** the npm bin has no built-in plugin tree — plugin load
> failures are fatal, and the SDK's bundled default config (extracted to a
> temp directory, no `node_modules` beside it) cannot satisfy plugin
> resolution. That is why this route sets `DSH_CORDIS_CONFIG` explicitly.

### How the SDK resolves the runtime (reference)

The binary is resolved with Python `HarnessClient` parity, plus the
Rust-only `DSH_RUNTIME_BIN` route:

1. `Config::launch_args_override` (non-empty) — the whole argv, verbatim
   (the npx variant of Route B uses this);
2. `Config::runtime_bin`;
3. `DSH_RUNTIME_BIN` from the parent environment;
4. otherwise `Error::RuntimeNotFound`, whose message names the acquisition
   routes.

An empty `launch_args_override` and an empty `DSH_RUNTIME_BIN` both count as
absent (Python truthiness), so resolution never produces an unlaunchable
empty program.

**Bundled default config.** With no effective `DSH_CORDIS_CONFIG`,
`DeepSeekHarness::start` injects a bundled copy of the runtime's default
`cordis.yml` (byte-identical to the official default), extracted to the
system temp directory on first use and byte-verified on every use — the
runtime refuses to boot without an explicit config, so this injection is
required, and an extraction/verification failure propagates as `Error::Io`
(never a silent config-less launch). Note the [npm route caveat](#route-b--npm-no-build):
the bundled default only resolves plugins when the runtime carries its own
plugin tree (Route A's executable); with the npm bin, always provide
`DSH_CORDIS_CONFIG` yourself.

> **Deliberate divergence from the Python SDK** (documented; do not "fix"):
> Python injects its bundled default only when its bundled runtime carrier is
> used. This crate has no bundled carrier (bring-your-own runtime), so the
> default is injected whenever no effective config exists, regardless of how
> the binary was resolved.

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

`DeepSeekHarness::close` (and `HarnessClient::close`) runs the close ladder:
a cooperative `shutdown` request bounded by `shutdown_timeout` (default 1s,
diagnostic only on failure) → drop stdin (EOF) → wait `eof_grace`
(default 6s — the runtime gets time to flush durable state after stdin
closes) → SIGTERM → wait `term_grace` (default 3s) → SIGKILL → wait. The
ladder is idempotent, is unconditional teardown (failure at any tier still
reaps the child — the child is also killed on drop, so a ladder failure
cannot strand the process), and resolves all pending requests with
`Error::TransportClosed`.

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
| `DSH_CORDIS_CONFIG` | Runtime composition config | `Config::cordis_config` (non-empty) wins; otherwise a non-empty value from `Config::env` or the parent environment is inherited as-is. Empty strings count as absent — an empty-string `Config::env` entry is skipped on copy so it can never clobber a non-empty parent value. With no effective value, the SDK injects the bundled default `cordis.yml` |
| `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` | Model endpoint and credentials | Inherited as-is; overridden only when `Config::base_url` / `Config::api_key` is configured |
| `DSH_CWD` | Agent working directory | Always injected, from `Config::cwd` (resolved absolute) |
| `DSH_SESSION_ROOT` | Session root | Injected only when `Config::session_root` is configured; surfaced on every `RunResult` |

## Testing

- `cargo test` — wire-protocol, lifecycle, and `Session::run` semantics
  suites against a scripted fake runtime (no real runtime needed).
- `tests/real_runtime.rs` — a single smoke test that runs **only** when both
  `DSH_RUNTIME_BIN` and `DEEPSEEK_API_KEY` are set; otherwise it prints an
  explicit skip notice and passes, so `cargo test` is green with no runtime
  and no credentials present.

## Platform support & MSRV

The SDK itself is pure Rust and platform-light; the consumed runtime decides
the platform matrix. Route A (prebuilt executable) ships linux-x64, linux-arm64,
macos-arm64; Route B (npm) runs wherever Node.js ≥ 22.19 does. **No
Windows**: upstream has no Windows runtime builds.

MSRV: current stable Rust (no minimum is pinned in `Cargo.toml`; the crate
tracks the stable toolchain).

## Known limitations

- **Pre-release software** — the crate ships pre-release versions while the
  runtime protocol settles; the API may change before `0.1.0`. The
  real-runtime smoke test is environment-gated (see [Testing](#testing)); the
  fake-runtime suites carry protocol correctness.
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
  companion crate is a roadmap item, not part of this version. Acquire the
  runtime per [Runtime acquisition](#runtime-acquisition).
- **No TypeScript-parity helper** — the TS-shaped `RunResult` (without
  `finish_reason` / `session_root`) is not provided.

## License

Apache-2.0.
