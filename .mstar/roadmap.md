# dsh-rust-sdk Roadmap

Durable roadmap (tracked). `status.json` / plans carry per-plan progress; this file carries cross-iteration delivery intent.

Deferred items stay here until their **trigger** fires; v0.1 must not implement them.

| id | Item | Status | Source iteration | Target iteration | Trigger | Owner | Done definition |
|----|------|--------|------------------|------------------|---------|-------|-----------------|
| runtime-bin-delivery | Runtime 交付：在 **方案 B**（companion crate，`build.rs` 按 target triple 嵌入预编译 `dsh-jsonrpc-agent-pkg-*`：linux-x64 / linux-arm64 / macos-arm64，macOS 必带 `-spawn-helper`）与 **方案 C**（首跑从 GitHub Releases 懒下载固定版本）之间 **clarify 锁定其一** 后实施。不在本条目内同时做 B+C。 | open | v0.1 (deferred by locked scope: 方案 A only) | next after v0.1 (provisionally v0.2; id not user-locked) | **All of:** (1) v0.1 merged to `main`; (2) DSH official runtime build channel can produce `dsh-jsonrpc-agent-pkg-*` via `scripts/build-exe-for-python-sdk.ts` in [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness); (3) a hosting location exists for those artifacts (crate files and/or GitHub Releases). Do **not** wait on Python `sdk-runtime` wheels — those are a Python packaging path, not the Rust trigger. | `@architect` (scheme lock + crate/CI layout). Product acceptance: `@product-manager`. Orchestration: `@project-manager` (does not own the done definition). | **Decision:** B or C written in that iteration’s compass (not left as “or”). **User outcome:** on linux-x64 / linux-arm64 / macos-arm64, `DeepSeekHarness::start()` with no `DSH_RUNTIME_BIN` / `Config::runtime_bin` launches the official runtime (macOS includes `-spawn-helper`). `Error::RuntimeNotFound` only for unsupported OS/arch or failed fetch/missing artifact. Public docs cite DSH only as `https://github.com/deepseek-ai/deepseek-harness`. 方案 A env override remains supported. |
| crates-io-publish | Publish `deepseek-harness-sdk` (and the runtime companion crate **if** 方案 B was chosen) to crates.io. | open | v0.1 (non-goal) | ≥ the iteration that completes `runtime-bin-delivery`, unless product explicitly accepts a client-only 0.x that still requires 方案 A | **All of:** (1) v0.1 on `main`; (2) `runtime-bin-delivery` scheme locked **or** product records a written waiver that crates.io 0.x is client-only (方案 A); (3) README states MSRV + platform matrix + runtime acquisition + protocol pre-release caveat (`serverInfo.version` is `0.0.1`, **no version negotiation**). Do **not** block publish on DSH inventing protocol version negotiation — that is a DSH-side event we do not control; document instability instead. | `@product-manager` (release/product call). Implementation: `@architect` / `@ops-engineer`. Orchestration: `@project-manager`. | `cargo add deepseek-harness-sdk` installs a published crate. README documents MSRV, platforms (linux-x64 / linux-arm64 / macos-arm64; Windows out), runtime acquisition, Python-vs-TS `RunResult` table, and DSH URL `https://github.com/deepseek-ai/deepseek-harness` only. If a companion crate exists, it ships on the same release train. |

## v0.1 committed scope (in flight — not yet delivered)

This section is the **iteration target**, not a completion claim. Move to “delivered” only at iteration-close (Phase 3).

- Core SDK crate `deepseek-harness-sdk`: protocol / transport / low-level `HarnessClient` / high-level Python-parity API / typed errors
- Runtime resolution **方案 A only** (`DSH_RUNTIME_BIN` / `Config::runtime_bin` / launch-args override + default `cordis.yml` injection)
- Fake-runtime coverage of wire + `Session::run` semantics + env-gated real-runtime smoke (skip when unset)
- Explicit non-goals this iteration: runtime binary delivery, crates.io, Windows, TS-parity helper, cancel/session-close RPC
