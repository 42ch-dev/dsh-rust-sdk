# deepseek-harness-sdk

English | [中文](README.zh.md)

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange)](Cargo.toml)
[![crates.io](https://img.shields.io/crates/v/deepseek-harness-sdk)](https://crates.io/crates/deepseek-harness-sdk)

See the [CHANGELOG](CHANGELOG.md) for the release history.

Rust client SDK for the [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
(DSH) runtime. The runtime is the `dsh` CLI booted under a named profile —
this crate spawns it as a subprocess (`dsh --profile sdk`, the profile
default) and speaks its stdio JSON-RPC 2.0 protocol. One crate, two layers:
the high-level Python-parity API (`DeepSeekHarness` / `Session::run` /
`RunResult`) and the low-level protocol client (`HarnessClient`).

The crate is the design twin of the official
[Python SDK](https://github.com/deepseek-ai/deepseek-harness), sharing the
same runtime peer, wire protocol, and layering; the Python SDK surface is the
alignment baseline for every public type and error. The TypeScript SDK's
divergences are documented (notably `RunResult`, see below), as are the
crate's own deliberate divergences from both references (see
[Deliberate divergences](#deliberate-divergences)).

This crate is a **pure client**. It contains no agent, LLM, or persistence
logic — the spawned runtime process does all of that. The runtime is
bring-your-own: this crate never downloads, bundles, or ships one (see
[Runtime acquisition](#runtime-acquisition)).

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
        dsh_bin: std::env::var("DSH_RUNTIME_BIN").ok(),
        request_timeout: Some(Duration::from_secs(120)),
        ..Config::default()
    })
    .await?;

    let session = harness.start_session(None);
    let result = session
        .run(Input::Text("Reply with exactly: ok".into()), None)
        .await?;

    println!("finish_reason: {:?}", result.finish_reason);
    println!("final_response: {}", result.final_response);

    harness.close().await?;
    Ok(())
}
```

`DeepSeekHarness::start` is eager: it resolves the runtime, resolves (and
creates) the harness home, spawns the subprocess, and completes the
`initialize` handshake before returning. The runtime inherits
`DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` from the environment unless the
crate injects overrides, so callers can use real model endpoints directly or
point those variables at a local proxy.

## Runtime acquisition

The runtime is bring-your-own; the SDK only resolves and launches it. There
is **no separate JSON-RPC agent program**: the stdio JSON-RPC server the SDK
talks to is a plugin row inside the runtime's profile bundle. The runtime is
the ordinary `dsh` CLI from
[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness), booted
under the `sdk` profile (or any profile you name via `Config::profile`).

Upstream packages the runtime as a self-contained Node.js single-file
executable (no system Node.js needed at runtime; plugin tree embedded) and
distributes it through the `deepseek-harness-runtime-bin` platform wheels,
which install the normal `dsh` CLI as
`deepseek-harness-sdk-runtime-<platform>-<arch>`. Published targets are
**Linux x64, Linux arm64, macOS arm64, macOS x64, and Windows x64** (Windows
uses the `.exe` suffix). macOS needs its sibling `-spawn-helper` beside the
executable (`node-pty`), and the Linux/macOS wheels carry a `-rg` ripgrep
sidecar (Windows `-rg.exe`) — copy any sidecar along when you relocate the
executable.

Two routes to a runtime:

### Route A — the platform wheel (recommended)

```sh
python -m pip install deepseek-harness-runtime-bin
export DSH_RUNTIME_BIN="$(python -c 'import deepseek_harness_runtime as r; print(r.bundled_runtime_path())')"
```

The `python -c` invocation only *locates* the installed executable and prints
its path — **no Python runs at SDK runtime**. The SDK launches the executable
directly (always injecting the resolved `DSH_HOME`, so the home is explicit
even on first boot).

### Route B — build from source

Build the runtime executable from source with the
`build-exe-for-python-sdk` script from the
[official repository](https://github.com/deepseek-ai/deepseek-harness), then
point `DSH_RUNTIME_BIN` (or `Config::dsh_bin`) at the built executable. This
is the route to use when the published wheel does not cover your platform.

### How the SDK resolves the runtime

1. `Config::dsh_bin` (non-empty);
2. `DSH_RUNTIME_BIN` from the parent environment (non-empty);
3. otherwise `Error::RuntimeNotFound`, whose message names the acquisition
   routes and cites the official repository.

An empty `Config::dsh_bin` and an empty `DSH_RUNTIME_BIN` both count as
absent, so resolution never produces an unlaunchable empty program.
`DSH_RUNTIME_BIN` is **explicitly preserved** after the `runtime_bin` →
`dsh_bin` rename: it is a supported product surface, not a compatibility
shim for a removed field.

The launch argv is exactly `--profile <profile>` (default `"sdk"`) followed
by one `--patch <absolute path>` pair per configured `Config::patches` entry,
in caller order. Patch paths are resolved absolute before spawn. The crate
never passes application arguments and never emits the diagnostic
`--dump-config` / `--dump-default-config` subcommands. An empty `profile`
is rejected locally before spawn.

## `DSH_HOME` resolution

The harness home resolves with the **runtime's own precedence**, highest
first:

1. an explicit `Config::dsh_home`;
2. a **non-empty** `$DSH_HOME` (first from `Config::env`, then inherited from
   the parent environment) — blank or whitespace-only counts as **unset**;
3. `~/.dsh`.

The resolved home is normalized to an absolute path with `~` expanded, is
created when absent so a fresh home boots, and is always injected into the
runtime child environment as `DSH_HOME`.

The selection is **observable, never silent**: call
`Config::resolve_dsh_home(&parent_env)` to read what a launch would pick, and
`DeepSeekHarness::dsh_home()` on a running instance to read the exact home it
was launched with.

> **Deliberate divergence from the Python SDK** (documented; do not "fix"):
> where Python raises `ValueError` rather than falling back, this crate
> resolves `~/.dsh` — `~/.dsh` is the runtime's own documented default. The
> crate follows the runtime's precedence (like the TypeScript SDK) instead of
> Python's refusal. The real defect the Python refusal guards against is a
> *silent* home selection; observability above addresses that instead.

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

`start` is **eager**: it resolves the runtime, resolves and creates the
harness home (see [`DSH_HOME` resolution](#dsh_home-resolution)), composes
the child environment, spawns the subprocess, and performs the `initialize`
handshake before returning. (This differs from the Python and TypeScript
SDKs, which start lazily on first use.) A failed handshake runs the close
ladder before the error propagates, so the spawned child is never leaked.

The `initialize` handshake is **bounded** by `Config::initialize_timeout`
(default **30 s**, following Python; `None` means unbounded and is a
deliberate opt-out). The bound applies to the handshake only — never to the
activity interval or to `session/prompt`. On expiry `start` returns
`Error::RequestTimeout { method: "initialize", .. }`, whose message names
the **selected profile** (e.g. `(selected dsh profile 'sdk')`), and the child
is closed rather than left running.

`Config::reasoning_effort` (an `Option<String>`) is sent on `initialize` as
the wire key `reasoningEffort` **only when set to a non-empty string**: unset
or blank values are omitted entirely. The runtime rejects a non-string or
empty `reasoningEffort`, so blank is dropped rather than sent.

`Config::cwd` is resolved absolute and sent as `initialize.cwd`;
`Config::runtime_cwd` sets the subprocess cwd and defaults to `cwd`.
`Config::request_timeout` bounds every other request, including
`session/prompt`; `None` (the default) waits indefinitely.

Sessions created by `start_session` may run concurrently: the harness owns
the spawned child behind an async mutex, sessions interleave at the
`session/prompt` write, and each waits on its own subscription.

### `Session::run` — one activity interval

`run` implements the Python `Session.run` algorithm:

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
`session.status` / `subagent.started` / `subagent.finished`) in transport
order.

`run` accepts an optional **per-notification callback**:

```rust
pub async fn run(
    &self,
    input: Input,
    on_notification: Option<&(dyn Fn(&Notification) + Send + Sync)>,
) -> Result<RunResult, Error>
```

The callback observes **every notification delivered to the run's session-tree
subscription, in wire order**, and is invoked **as each notification
arrives**, not deferred until the end. It is a pure observer: it cannot
change the returned `RunResult` (with and without a callback, `events` and
`notifications` hold the same sets in the same order), it is a layer over the
existing notification path rather than a second subscription, and it is
optional and additive (passing `None` behaves exactly like the plain run
path). A panic inside the callback is a caller bug and propagates; the crate
does not swallow it.

Both waits — the receipt wait and the idle wait — are **unbounded** (Python
parity); only the `session/prompt` request is bounded by
`Config::request_timeout`. Callers needing a bound wrap the call in
`tokio::time::timeout` — this bounds the local wait, not the runtime's turn.

### Session format v2

The runtime's `session.event` vocabulary is **session format v2** (no wire
change). The crate documents the v2 vocabulary and keeps event payloads
untyped, so the run path is unaffected:

- `assistant/message` now carries an embedded
  `stream: AssistantStreamRecord[]`;
- `assistant/attempt` was added;
- `assistant/chunk` was removed — the crate does not claim `assistant/chunk`
  support and does not parse the embedded v2 stream (non-goal).

### Content block vocabulary

`ContentBlock` models the DSH `ContentBlockMap` with **six** typed variants
plus an `Unknown` fallback for unrecognized tags and malformed bodies:

| Variant | Shape |
|---|---|
| `text` | `{type:"text", text}` |
| `reasoning` | `{type:"reasoning", text}` |
| `image` | `{type:"image", attachment: ImageAttachmentRef}` |
| `file` | `{type:"file", attachment: FileAttachmentRef}` |
| `tool-call` | `{type:"tool-call", id, name, arguments}` — `arguments` is a **raw JSON string** |
| `tool-result` | `{type:"tool-result", toolCallId, content: ContentBlock[], isError?}` — `content` is recursive |

`FileAttachmentRef` is `{attachmentId, name, bytes}`. `ImageAttachmentRef` is
`{attachmentId, mediaType, bytes, width, height, name?, originalDimensions?}`;
`originalDimensions` is present only when normalization reduced the image, and
parses → serializes without loss. An unrecognized `type` (or a known tag with
a malformed body) falls through to `ContentBlock::Unknown`, preserving the raw
object verbatim.

### `RunResult`

`RunResult` follows the **Python** SDK field set — exactly five fields, no
`session_root` (upstream removed it and asserts its absence). The TypeScript
SDK's `RunResult` lacks `finish_reason`; Rust follows Python:

| Field | Python | TypeScript | Rust (this crate) |
|---|---|---|---|
| `session_id` / `sessionId` | yes | yes | `session_id: String` |
| `final_response` / `finalResponse` | yes | yes | `final_response: String` |
| `finish_reason` | yes | no | `finish_reason: Option<String>` |
| `events` | yes (root session only) | yes | `events: Vec<serde_json::Value>` |
| `notifications` | yes (root + descendants, transport order) | yes | `notifications: Vec<Notification>` |
| `session_root` | no (removed) | no | **does not exist** |

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
| `Error::Config` | Invalid launch configuration (e.g. an empty `profile`), rejected locally before spawn |
| `Error::TransportClosed` | Runtime process not running, or stdio closed unexpectedly; carries diagnostics (exit status and captured stderr tail) |
| `Error::RequestTimeout` | A request got no response within the configured timeout; carries the method name and, for `initialize`, names the selected profile (e.g. `(selected dsh profile 'sdk')`) |
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

The wire has four server-to-client notifications: `session.event`,
`session.status`, `subagent.started`, and `subagent.finished`. Tree
notifications travel a broadcast channel capped at 4096 with drop-oldest
semantics. If a high-volume tree floods more notifications than fit between
the SDK's reads, the dropped set can include the inbox receipt or the
root-idle notification a run depends on — rather than hang forever or return
a silently truncated result, `Session::run` then **fails fast** with
`Error::SdkProtocol`. A caller expecting very large bursts can bypass the cap
only via the low-level `HarnessClient::spawn_with_broadcast_capacity` instead
of `DeepSeekHarness::start`. For arrival-order observation of every
notification, pass the per-notification callback to `Session::run` (above).

## Environment variables

The parent environment is inherited wholesale; the SDK injects or overrides
only the keys below. Crate-injected values are applied first and the caller's
`Config::env` entries are applied after, so on collision the caller's value
wins (Python `dict.update` semantics) — except `DSH_HOME`, which is resolved,
never post-resolution overridden:

| Variable | Role | Semantics |
|---|---|---|
| `DSH_HOME` | Harness home (child env) | Always written with the resolved absolute, `~`-expanded home. The caller's value is an **input to resolution** (`Config::dsh_home` → non-empty `DSH_HOME` → `~/.dsh`), not a post-resolution override; read the selection back via `Config::resolve_dsh_home` / `DeepSeekHarness::dsh_home` |
| `DSH_RUNTIME_BIN` | Runtime binary resolution | Consulted when `Config::dsh_bin` is not set; empty counts as absent. Explicitly preserved after the `runtime_bin` → `dsh_bin` rename (not a compatibility shim) |
| `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` | Model endpoint and credentials | Inherited as-is; overridden when `Config::base_url` / `Config::api_key` is configured, and a caller-supplied `Config::env` entry for either key is applied after injection and wins on collision |
| `DSH_CORDIS_CONFIG` / `DSH_SESSION_ROOT` / `DSH_CWD` | Removed — never written | No reader upstream; see the [removal table](#removed-surface-v01--current) |

## Deliberate divergences

Each divergence below is deliberate and documented; a contributor must not
"fix" it back to reference behaviour without a superseding spec decision.

1. **`DSH_HOME` fallback** — the crate resolves `~/.dsh` where Python raises
   `ValueError` ([`DSH_HOME` resolution](#dsh_home-resolution)).
2. **No client-directed request API** — Python exposes `next_request` /
   `respond` / `notify`; the crate exposes none (the runtime emits no
   client-directed requests; they are auto-answered `-32601`). Non-goal, not
   a gap.
3. **Stricter malformed-notification policy** — a `session.event` /
   `session.status` whose payload fails its shape check fails the run with
   `Error::SdkProtocol`. Python silently skips a malformed event/status and
   only raises on a malformed last `turn/end`; TypeScript raises for a
   malformed `session.event` but ignores a malformed `session.status`. This
   converts a silent hang into a typed failure and is **not** Python parity.
   Related local robustness choices: the embedded stderr tail is capped
   (8 KiB, newest lines first) and the broadcast buffer is bounded with a
   fail-fast on observed lag.
4. **Strict `serverInfo.name` equality** — `initialize` requires the identity
   to be exactly `deepseek-harness-sdk-runtime`; Python treats the fields as
   optional and TypeScript checks presence only. An upstream rename fails
   loudly instead of being silently accepted.
5. **No `run()` convenience, no lazy start** — the crate requires an explicit
   `DeepSeekHarness::start`; Python and TypeScript can start lazily on first
   use. Non-goal, not a gap.

## Removed surface (v0.1 → current)

The following v0.1 identifiers are **gone — no alias, no deprecated shim**.
Each row names what it was and what replaces it:

| Removed item | What it was | Replacement |
|---|---|---|
| `Config::session_root` | claimed to control where sessions land | `Config::dsh_home` — sessions live under `$DSH_HOME/sessions` |
| `Config::cordis_config` | path to a `cordis.yml` config file | the profile tree (`Config::profile` + `Config::patches`) — no config file is passed to the runtime |
| the `DSH_CORDIS_CONFIG` injection | wrote `DSH_CORDIS_CONFIG` into the child env | the profile tree — no reader upstream |
| the `DSH_SESSION_ROOT` injection | wrote `DSH_SESSION_ROOT` into the child env | `Config::dsh_home` — sessions live under `$DSH_HOME/sessions` |
| the `DSH_CWD` injection | wrote `DSH_CWD` into the child env | `Config::cwd` — sent as `initialize.cwd` |
| `Config::launch_args_override` | replaced the whole argv with an opaque list | `Config::dsh_bin` + `Config::profile` + `Config::patches` — the launch is composed from typed fields |
| `Config::runtime_bin` | the runtime-path override field | `Config::dsh_bin` (rename; `DSH_RUNTIME_BIN` env route preserved) |
| `RunResult::session_root` | surfaced the session directory on every result | **dropped — no replacement** (upstream removed it) |
| `assets/cordis.yml` | bundled default config file | the profile bundle — it mounted a package deleted upstream |
| `DEFAULT_CORDIS_YML` | embedded the config file above | the profile bundle |
| `bundled_default_config_path` | the temp extraction path for the config | the profile bundle — its whole purpose was the deleted injection channel |
| the `assets/cordis.yml` entry in `Cargo.toml [package] include` | shipped the deleted file | — (the file no longer exists) |

## Testing

- `cargo test` — wire-protocol, lifecycle, and `Session::run` semantics
  suites against a scripted fake runtime (no real runtime needed).
- `tests/real_runtime.rs` — a keyless handshake tier (start → `initialize` →
  close against a real `dsh`, no API key) that runs when a `dsh` binary is
  available, plus a live-turn tier gated on `DEEPSEEK_API_KEY`; otherwise it
  prints an explicit skip notice and passes, so `cargo test` is green with no
  runtime and no credentials present.

## Platform support & MSRV

The SDK itself is pure Rust and platform-light; the consumed runtime decides
the platform matrix. Upstream publishes the runtime for **5 targets**: Linux
x64, Linux arm64, macOS arm64, macOS x64, and Windows x64 (see
[Runtime acquisition](#runtime-acquisition)).

MSRV: current stable Rust (no minimum is pinned in `Cargo.toml`; the crate
tracks the stable toolchain).

## Known limitations

- **Pre-release software** — the crate ships pre-release versions while the
  runtime protocol settles; the API may change before `0.1.0`. The
  real-runtime tests are environment-gated (see [Testing](#testing)); the
  fake-runtime suites carry protocol correctness.
- **No mid-turn cancel** — there is no session-close / cancel RPC on the
  wire. `Session::run` waits until the root session reports `idle`; closing
  the harness mid-turn abandons the in-flight turn. A `Config::request_timeout`
  only abandons the local wait — the server-side work still runs until
  close.
- **No version negotiation** — the runtime identifies as `serverInfo` 0.0.1
  pre-release, and `initialize` enforces a strict `serverInfo.name` check
  (`deepseek-harness-sdk-runtime`): the protocol declares the name
  wire-stable and has no negotiation, so an unexpected identity is a hard
  `Error::SdkProtocol`.
- **No runtime binary delivery / bundling / download** — the runtime
  companion crate is a roadmap item, not part of this version. Acquire the
  runtime per [Runtime acquisition](#runtime-acquisition).

## License

Apache-2.0.
