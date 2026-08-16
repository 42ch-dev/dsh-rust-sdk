# deepseek-harness-sdk

Rust SDK for the [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
runtime stdio JSON-RPC 2.0 protocol: a low-level `HarnessClient` plus the
Python-parity high-level API (`DeepSeekHarness` / `Session::run` / `RunResult`).

This crate is a **pure client**. It contains no agent, LLM, or persistence
logic — the runtime process does all of that. The runtime binary is
bring-your-own (Plan A): this crate never downloads or bundles one.

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
`Config::api_key` / `Config::base_url`).

## Runtime acquisition

The runtime is bring-your-own (Plan A); the SDK only locates it.

1. **Build from source** — check out
   [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness), run
   `scripts/build-exe-for-python-sdk.ts` in the checkout (see that
   repository's build instructions), then point `DSH_RUNTIME_BIN` at the
   built executable.
2. **Bring your own binary** — set the `DSH_RUNTIME_BIN` environment
   variable, or pass `Config::runtime_bin` /
   `Config::launch_args_override` in code.

Resolution precedence: `Config::launch_args_override` (whole argv, verbatim)
→ `Config::runtime_bin` → `DSH_RUNTIME_BIN` → `Error::RuntimeNotFound`
(whose message names the acquisition routes above).

Environment injection (Python parity): `DSH_CWD` always; `DSH_SESSION_ROOT`
(`Config::session_root`), `DSH_CORDIS_CONFIG` (`Config::cordis_config`),
`DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` only when configured. With no
effective config, the SDK injects a bundled copy of the runtime's default
`cordis.yml`.

> **Deliberate divergence from the Python SDK** (documented; do not "fix" to
> match Python): the Python SDK injects its bundled default `cordis.yml`
> only when the bundled runtime carrier is used. This crate is bring-your-own
> runtime (Plan A) — there is no bundled carrier — so the bundled default is
> injected whenever no effective config exists, **regardless of how the
> runtime binary was resolved**.

## `RunResult` alignment

`RunResult` follows the **Python** SDK field set. The TypeScript SDK's
`RunResult` lacks `finish_reason` and `session_root`; Rust intentionally
follows Python, not TypeScript.

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

`finish_reason` is the last `turn/end`'s `data.reason.kind` inside the
activity interval of one `Session::run` (`None` when the window has no
`turn/end`).

## Testing

- `cargo test` — wire-protocol, lifecycle, and `Session::run` semantics
  suites against a scripted fake runtime (no real runtime needed).
- `tests/real_runtime.rs` — a single smoke test that runs **only** when both
  `DSH_RUNTIME_BIN` and `DEEPSEEK_API_KEY` are set; otherwise it prints an
  explicit skip notice and passes, so `cargo test` is green with no runtime
  binary and no credentials present.

## Non-goals

- **No cancellation.** There is no session-close / cancel RPC. `Session::run`
  waits until the root session reports `idle`; closing the harness
  (`DeepSeekHarness::close`) mid-turn abandons the in-flight turn. Callers
  needing a bound wrap `Session::run` in `tokio::time::timeout` — this bounds
  the local wait, not the runtime's turn.
- **Bounded notification buffer.** Tree notifications travel a broadcast
  channel capped at 4096 with drop-oldest semantics. If a high-volume tree
  floods more notifications than fit between the SDK's reads, the dropped set
  can include the inbox receipt or the root-idle notification — `Session::run`
  then fails fast with `Error::SdkProtocol` instead of hanging forever or
  returning a silently truncated result.
- **No Windows support.** Consumed platforms: linux-x64, linux-arm64,
  macos-arm64.
- **No runtime binary delivery / bundling / download** (planned, see roadmap).
- **No crates.io publish yet.**
- **No TypeScript-parity helper** — the TS-shaped `RunResult` (without
  `finish_reason` / `session_root`) is not provided.

## Platform support & MSRV

Consumed (not shipped) platforms: linux-x64, linux-arm64, macos-arm64.
MSRV: 1.80 (the crate uses `std::sync::LazyLock`).

## License

Apache-2.0.
