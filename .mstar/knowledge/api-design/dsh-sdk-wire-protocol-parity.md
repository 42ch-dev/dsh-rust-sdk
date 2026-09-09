---
module: deepseek-harness-sdk
date: 2026-08-16
last_updated: 2026-09-08
problem_type: api_design
category: api-design
severity: high
tags:
  - dsh
  - wire-protocol
  - json-rpc
  - python-parity
  - sdk-client
applies_when:
  - Maintaining or extending the deepseek-harness-sdk client (protocol, client, api layers)
  - Building the runtime-bin companion crate (future)
  - Debugging Session::run activity-interval behavior against the DSH runtime
related_components:
  - src/protocol.rs
  - src/client/
  - src/api.rs
  - src/runtime.rs
---

# DSH SDK wire protocol: source-verified facts and Python-parity decisions

> **Status (2026-09-08): contract facts superseded.** The launch/env/config
> half and the wire-surface facts this document used to carry are now the
> frozen contract of `.mstar/specs/dsh-runtime-launch-contract.md` and
> `.mstar/specs/dsh-sdk-wire-parity-surface.md` (both grounded at upstream
> ref `c389f96bf3`). This document keeps only what the specs do not restate:
> the source-verification method and the three corrected traps below.

## Context

v0.1 built a Rust client for the DeepSeek Harness (DSH) SDK runtime: stdio line-framed JSON-RPC 2.0, spawned as a subprocess. The Python SDK (`python/sdk` in DSH) is the alignment baseline; the TypeScript SDK (`packages/sdk/client`) is the design twin. An advisor feasibility study plus an architect pass that read the DSH sources end-to-end produced a set of verified facts — three of which **contradicted the original plan text** and would each have shipped a broken client. The three traps, and the source-verification method that surfaced them, remain this document's value; every other fact moved to the two specs above.

Upstream: https://github.com/deepseek-ai/deepseek-harness (the only permitted citation in externally visible docs).

## Guidance

### Source-verification method

Contract facts are verified **against the upstream runtime source at a pinned ref**, not against the SDK wrappers or a prior audit's conclusions:

- Pin an upstream commit and cite `path:line` for every contract statement — the two specs cite `c389f96bf3` throughout. A moved line is not a contract change; a changed rule is. Re-run the specs' re-verification greps when upstream advances (launch spec §10, wire spec §10).
- Treat both official SDKs as **clients**, not as the authority: a wrapper can carry its own divergence from the runtime (Python raises `ValueError` on a missing `DSH_HOME`; the runtime itself falls back to `~/.dsh` — launch spec §3.3).
- Re-verify in passes that classify each fact as **still true** vs **no longer true** and record what changed. The 2026-09-08 re-verification at `c389f96bf3` found the wire surface unchanged but the launch/env/config contract replaced (the runtime became `dsh --profile <name>` with a resolved `DSH_HOME`, and the old `DSH_CORDIS_CONFIG` / `DSH_SESSION_ROOT` / `DSH_CWD` knobs lost their readers) — which is why the contract half now lives in the specs.
- Keep the reusable facts in durable docs; one-off audit reports are evidence, not the source of truth.

### The three traps (each corrected a wrong plan line)

1. **Inbox receipt matches `inserted[].id`, NOT `inserted[].messageId`.** Python `_is_inbox_receipt` walks `inserted[]` matching `.id` against the returned messageId. Matching on `.messageId` never fires → every `Session::run()` hangs forever at Phase 1.
2. **Malformed peer lines are SKIPPED, not rejected.** Both reference clients ignore non-JSON/invalid-UTF-8 lines and keep reading. Only a local framing guard (oversize line, 16 MiB) errors. Rejecting malformed lines breaks parity with chatty prelude runtimes.
3. **stderr is captured (400-line tail), not inherited.** Python `deque(maxlen=400)`, TS `STDERR_TAIL_LIMIT=400`; the tail is embedded in `TransportClosed`/timeout/close-ladder diagnostics together with the exit code.

### Contract facts now live in the two frozen specs

Do not restate or re-derive these here; read them from the specs:

- **Wire surface** — request methods, notification names and payloads, request-id rules, `serverInfo` contract, the six-variant content-block vocabulary, `reasoningEffort` omit-when-unset, the bounded `initialize`, the per-notification callback, and the documented divergences (strict `serverInfo.name` equality, malformed-notification strictness, bounded broadcast buffer, stderr-tail cap): `.mstar/specs/dsh-sdk-wire-parity-surface.md` §2–§7.
- **Launch/env/config half** — launch grammar (`dsh --profile <name>` + ordered `--patch` overlays), `DSH_HOME` precedence and the `~/.dsh` divergence from Python, the child-env key set including the three forbidden keys (`DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, `DSH_CWD`), the removal list with a replacement per item, the `Config` / `RunResult` field sets, the close ladder, and error semantics: `.mstar/specs/dsh-runtime-launch-contract.md` §2–§7.

## Why This Matters

Every one of the three traps produces a client that **compiles, passes surface-level tests, and hangs or misdiagnoses in production** (receipt never matches; prelude chatter kills the transport; death diagnostics lose stderr/exit evidence). The parity decisions are locked product behavior backed by the frozen specs; silently reverting any of them is a spec violation even when it "matches Python better".

## When to Apply

- New protocol methods or notification types: extend `src/protocol.rs` with Unknown-tolerant parsing; never `deny_unknown_fields` (wire spec §5.2).
- Runtime-bin companion crate (deferred item `runtime-bin-delivery`): platform matrix is linux-x64 / linux-arm64 / macos-arm64 (CI publishes exactly these three; macOS needs the sibling `-spawn-helper`).
- Protocol bumps (`serverInfo.version` leaving 0.0.1): revisit the strict-name check and the no-negotiation stance together (wire spec §7.4).

## Examples

- `tests/run_semantics.rs` (16 tests) pins the interval algorithm: receipt gating, pre-receipt drop, non-root idle ignored, transport ordering, fail-fast arms.
- `tests/client_lifecycle.rs` (19 tests) pins transport/client behavior incl. malformed-line tolerance and close-ladder escalation.
