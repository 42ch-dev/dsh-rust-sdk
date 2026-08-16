---
category: Compatibility
---
- `RunResult` follows the Python SDK field set, including `finish_reason` and `session_root` — fields the TypeScript SDK's `RunResult` lacks.
- The wire protocol is pre-release: the runtime identifies as `serverInfo` 0.0.1 with a strict name check and no version negotiation.
- Consumes runtime builds for linux-x64, linux-arm64, and macos-arm64; there is no Windows support (upstream ships no Windows runtime builds).
- No mid-turn cancel or session-close RPC: `Session::run` waits until the root session reports `idle`.
