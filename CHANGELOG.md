# Changelog

## [Unreleased]

## [0.1.0-alpha.1] - 2026-08-16

### Added
- Initial release: `deepseek-harness-sdk` is a pure-client Rust SDK that spawns the official DeepSeek Harness (DSH) runtime as a subprocess and speaks its stdio JSON-RPC 2.0 protocol.
- Protocol types with serde 1:1 wire mapping and a merge-extensible `ContentBlock` with unknown-content passthrough; line-framed stdio transport with skip-malformed-lines parity and an oversized-frame guard.
- Low-level `HarnessClient`: request dispatch with timeout abandonment, session-tree tracking from `subagent.started` edges, broadcast notifications, and an unconditional close ladder.
- High-level Python-parity API: `DeepSeekHarness::start`, `Session::run`, and `RunResult` with `finish_reason` and `session_root`.
- Typed errors across the API surface: `Error::SdkProtocol`, `Error::JsonRpc` (preserving `code` and `data`), and `Error::RuntimeNotFound`.
- Published to crates.io as `0.1.0-alpha.1` — install with `cargo add deepseek-harness-sdk@0.1.0-alpha.1` (pre-release versions require an explicit version; a bare `cargo add` does not resolve to a pre-release).
- Bilingual installation-first READMEs (English / 中文) covering the quickstart, runtime acquisition routes, and platform support.
- Bring-your-own runtime (Plan A): the binary is resolved from `Config::launch_args_override` (full argv, verbatim), `Config::runtime_bin`, or the `DSH_RUNTIME_BIN` environment variable — the crate never downloads, bundles, or ships a runtime.
- Bundled default `cordis.yml` injection (byte-identical to the official default) when no `DSH_CORDIS_CONFIG` is set, plus environment injection of `DSH_CWD`, `DSH_SESSION_ROOT`, and model credentials.
- A missing runtime fails fast with `Error::RuntimeNotFound`, whose message names the acquisition routes and points to https://github.com/deepseek-ai/deepseek-harness.

### Compatibility
- `RunResult` follows the Python SDK field set, including `finish_reason` and `session_root` — fields the TypeScript SDK's `RunResult` lacks.
- The wire protocol is pre-release: the runtime identifies as `serverInfo` 0.0.1 with a strict name check and no version negotiation.
- Consumes runtime builds for linux-x64, linux-arm64, and macos-arm64; there is no Windows support (upstream ships no Windows runtime builds).
- No mid-turn cancel or session-close RPC: `Session::run` waits until the root session reports `idle`.
