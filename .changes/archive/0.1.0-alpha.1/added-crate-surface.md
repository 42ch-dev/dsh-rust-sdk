---
category: Added
---
- Initial release: `deepseek-harness-sdk` is a pure-client Rust SDK that spawns the official DeepSeek Harness (DSH) runtime as a subprocess and speaks its stdio JSON-RPC 2.0 protocol.
- Protocol types with serde 1:1 wire mapping and a merge-extensible `ContentBlock` with unknown-content passthrough; line-framed stdio transport with skip-malformed-lines parity and an oversized-frame guard.
- Low-level `HarnessClient`: request dispatch with timeout abandonment, session-tree tracking from `subagent.started` edges, broadcast notifications, and an unconditional close ladder.
- High-level Python-parity API: `DeepSeekHarness::start`, `Session::run`, and `RunResult` with `finish_reason` and `session_root`.
- Typed errors across the API surface: `Error::SdkProtocol`, `Error::JsonRpc` (preserving `code` and `data`), and `Error::RuntimeNotFound`.
