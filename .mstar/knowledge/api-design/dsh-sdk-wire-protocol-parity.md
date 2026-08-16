---
module: deepseek-harness-sdk
date: 2026-08-16
problem_type: api_design
category: api-design
severity: high
plan_id: 02-highlevel-api-runtime
tags:
  - dsh
  - wire-protocol
  - json-rpc
  - python-parity
  - sdk-client
applies_when:
  - Maintaining or extending the deepseek-harness-sdk client (protocol, client, api layers)
  - Building the runtime-bin companion crate (next iteration)
  - Debugging Session::run activity-interval behavior against the DSH runtime
related_components:
  - src/protocol.rs
  - src/client/
  - src/api.rs
  - src/runtime.rs
---

# DSH SDK wire protocol: source-verified facts and Python-parity decisions

## Context

v0.1 built a Rust client for the DeepSeek Harness (DSH) SDK runtime: stdio line-framed JSON-RPC 2.0, spawned as a subprocess. The Python SDK (`python/sdk` in DSH) is the alignment baseline; the TypeScript SDK (`packages/sdk/client`) is the design twin. An advisor feasibility study plus an architect pass that read the DSH sources end-to-end produced a set of verified facts — three of which **contradicted the original plan text** and would each have shipped a broken client. This document is the cross-iteration SSOT for those facts so future iterations (runtime-bin delivery, protocol upgrades) never re-derive them from memory.

Upstream: https://github.com/deepseek-ai/deepseek-harness (the only permitted citation in externally visible docs).

## Guidance

### Wire facts (verified against DSH source, pinned behavior)

- **Requests C→S**: `initialize` `{cwd, provider, model, maxTokens?}` → `{serverInfo {name, version}}`; `session/prompt` `{sessionId, contentBlocks}` → `{messageId}`; `shutdown` no params → `{}`.
- **Notifications S→C**: `session.event {sessionId, event}`; `session.status {sessionId, status: "running"|"idle"}`; `subagent.started {parentSessionId, childSessionId}`; `subagent.finished {provider, agentId, parentSessionId, childSessionId, status: "ok"|"error", stopReason, lastAssistantMessage?}`.
- `serverInfo.name` wire value is exactly `deepseek-harness-sdk-runtime`; version `0.0.1` hard-coded, **no version negotiation**.
- Outgoing request ids: string uuid-v4. Incoming ids: String | Number.
- `ContentBlock` is merge-extensible: 5 known variants (`text`, `reasoning`, `image`, `tool-call`, `tool-result`); `tool-call.arguments` is a **raw JSON string**; `tool-result.content` is recursive `ContentBlock[]`. Unknown `type` must pass through (`Unknown(Value)`).

### The three traps (each corrected a wrong plan line)

1. **Inbox receipt matches `inserted[].id`, NOT `inserted[].messageId`.** Python `_is_inbox_receipt` walks `inserted[]` matching `.id` against the returned messageId. Matching on `.messageId` never fires → every `Session::run()` hangs forever at Phase 1.
2. **Malformed peer lines are SKIPPED, not rejected.** Both reference clients ignore non-JSON/invalid-UTF-8 lines and keep reading. Only a local framing guard (oversize line, 16 MiB) errors. Rejecting malformed lines breaks parity with chatty prelude runtimes.
3. **stderr is captured (400-line tail), not inherited.** Python `deque(maxlen=400)`, TS `STDERR_TAIL_LIMIT=400`; the tail is embedded in `TransportClosed`/timeout/close-ladder diagnostics together with the exit code.

### Python-parity decisions (deliberate, documented divergences included)

- `RunResult` follows **Python**, not TS: 6 fields incl. `finish_reason` + `session_root` (TS has neither). Rustdoc `# Compatibility` states this explicitly.
- `finish_reason` = last root `turn/end`'s `data.reason.kind` **within the `Session::run` activity interval** (subscribe-before-prompt → root idle inclusive); no `turn/end` → `None`; malformed last `turn/end` → `Error::SdkProtocol` with exact message `"turn/end event requires a string data.reason.kind"`. Malformedness is checked only on the **last** `turn/end` (reversed scan stops there).
- `final_response` = last `assistant/message` event, pointer walk `data.message.content` else `data.content`, `text: null` → `""`, **no fallback to earlier events**.
- Close ladder with TS defaults (Python collapses all to 1s): `shutdown` 1s → stdin EOF grace 6s → SIGTERM grace 3s → SIGKILL. Teardown must be unconditional (a ladder error must not leave pending/tasks/notifications alive, and a retry `close()` must be safe).
- **Env injection**: `DSH_CWD` always; `DSH_SESSION_ROOT` / `DSH_CORDIS_CONFIG` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` iff configured; override-set wins over inherited env; **empty-string `DSH_CORDIS_CONFIG` counts as absent** (Python truthiness) → bundled default `cordis.yml` injected. Python injects the default only for its bundled carrier; Rust under 方案 A (bring-your-own runtime) injects **always-when-absent** because the runtime refuses to boot without an explicit config — a deliberate divergence, do not "fix" it back to Python behavior.
- `serverInfo.name` **strict equality** is intentionally stricter than both references (Python: fields Optional; TS: presence-only). An upstream rename would be rejected on purpose.
- Broadcast buffer is bounded (4096 default, `Lagged(n)` drop-oldest at the low level) — Python's queue is unbounded. `Session::run` fails fast with a typed `SdkProtocol` error when lag is observed mid-run (a dropped receipt or root-idle would otherwise hang or silently truncate); malformed inspected payloads likewise fail fast rather than warn-and-continue.
- No protocol-level cancellation exists: abandoning a turn means closing the runtime. Documented in README; do not invent a cancel method.

## Why This Matters

Every one of the three traps produces a client that **compiles, passes surface-level tests, and hangs or misdiagnoses in production** (receipt never matches; prelude chatter kills the transport; death diagnostics lose stderr/exit evidence). The parity decisions above are locked product behavior backed by compass AC; silently reverting any of them is a spec violation even when it "matches Python better".

## When to Apply

- New protocol methods or notification types: extend `src/protocol.rs` with Unknown-tolerant parsing; never `deny_unknown_fields`.
- Runtime-bin companion crate (`.mstar/roadmap.md` `runtime-bin-delivery`): platform matrix is linux-x64 / linux-arm64 / macos-arm64 (CI publishes exactly these three; macOS needs the sibling `-spawn-helper`).
- Protocol bumps (`serverInfo.version` leaving 0.0.1): revisit the strict-name check and the no-negotiation stance together.

## Examples

- `tests/run_semantics.rs` (13 tests) pins the interval algorithm: receipt gating, pre-receipt drop, non-root idle ignored, transport ordering, fail-fast arms.
- `tests/client_lifecycle.rs` (13 tests) pins transport/client behavior incl. malformed-line tolerance and close-ladder escalation.
- Source doc promoted from: `iteration:v0.1/specs/python-parity-surface.md` (structured rewrite; the iteration spec remains the frozen v0.1 snapshot).
