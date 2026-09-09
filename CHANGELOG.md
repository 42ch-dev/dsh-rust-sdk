# Changelog

## [Unreleased]

## [0.2.0] - 2026-09-09

### Added
- CI now enforces the changelog-fragment discipline: every PR changing user-visible behavior must add or modify a `.changes/unreleased/` fragment in the same PR; Release-prep output is exempt (recognized by a `release/*` head branch or a diff confined to the paths Release prep writes).
- `AGENTS.md` documents the fragment rule: one fragment per user-visible change (frontmatter `category:` + English bullets), `CHANGELOG.md` is machine-assembled and never hand-edited.
- New `Config` fields `profile` (default `"sdk"`), `patches`, and `dsh_home`.
- `Config::reasoning_effort` — sent on `initialize` as the wire key `reasoningEffort` only when set to a non-empty string, and omitted entirely when unset or blank.
- `Session::run` accepts an `on_notification` callback that observes every notification delivered to the session-tree subscription, in wire order.
- `ContentBlock::File` — the sixth typed block variant, `FileAttachmentRef {attachmentId, name, bytes}`.
- `ImageAttachmentRef::original_dimensions` — carried on the wire as `originalDimensions` so a parsed image block round-trips without loss.
- `DeepSeekHarness::dsh_home` — reads back the resolved absolute home (the same value `Config::resolve_dsh_home` computes).

### Changed
- The npm-published `dsh` CLI (`@deepseek-ai/dsh`) is now documented as a runtime-acquisition route: install it with `npm install -g @deepseek-ai/dsh` and point `DSH_RUNTIME_BIN` at it; the bin is a Node.js script, so Node.js must be on `PATH`.
- The `RuntimeNotFound` hint now names the npm route alongside the bring-your-own and build-from-source routes.
- README Runtime acquisition now documents three runtime routes — the npm CLI (`@deepseek-ai/dsh`, recommended), the platform wheel (self-contained, no Node.js), and build from source — and states that the interactive `dsh` CLI (`@deepseek-ai/dsh`) is the SDK runtime this crate spawns as a subprocess (`dsh --profile sdk`) and speaks to over stdio JSON-RPC.
- The npm-published `dsh` CLI is now the recommended runtime-acquisition route: install it with `npm install -g @deepseek-ai/dsh` and point `DSH_RUNTIME_BIN` at it (the bin is a Node.js script, so Node.js must be on `PATH`).
- The platform wheel is now documented after the npm CLI as the self-contained fallback (no system Node.js at runtime), with the published-target matrix corrected to Linux x64, Linux arm64, macOS arm64, and Windows x64 — macOS x64 is not published, so that platform uses the build-from-source route.
- Building from source is documented as the only route that reproduces the exact contract basis this crate was verified against.
- README Runtime acquisition (Route B) documents the runtime wheel's ripgrep `-rg` sidecar: copy it along when relocating the executable.
- The runtime is launched as `dsh --profile <name>` with ordered `--patch <path>` overlays, not a bare program spawn.
- `DSH_HOME` now resolves by upstream precedence — `Config::dsh_home`, then a non-empty `DSH_HOME` environment variable, then `~/.dsh` (blank counts as unset). Unlike the Python SDK, the crate falls back to `~/.dsh` instead of raising `ValueError`; the resolved absolute home is observable via `Config::resolve_dsh_home`.
- The `initialize` handshake is bounded by `Config::initialize_timeout` (default 30 s; `None` is a deliberate opt-out).
- `Error::RequestTimeout` gained a `profile: Option<String>` field naming the selected DSH profile when the handshake had one, so a wedged `initialize` is diagnosable.

### Removed
- `Config::session_root` is removed; use `Config::dsh_home` — sessions live under the resolved `$DSH_HOME/sessions` instead.
- `Config::cordis_config` and the `DSH_CORDIS_CONFIG` environment injection are removed; use the profile tree (`Config::profile` + `Config::patches`) — no config file is passed to the runtime.
- The `DSH_SESSION_ROOT` and `DSH_CWD` environment injections are removed; sessions live under the resolved `DSH_HOME`, and the workspace directory reaches the runtime as `Config::cwd` (sent as `initialize.cwd`).
- `RunResult::session_root` is dropped with no replacement — upstream removed it and asserts its absence.
- `Config::launch_args_override` is removed; use `Config::dsh_bin` + `Config::profile` + `Config::patches` — the launch is composed from typed fields, not an opaque argv.
- `Config::runtime_bin` is renamed to `Config::dsh_bin` (the `DSH_RUNTIME_BIN` environment override is preserved).
- The bundled `assets/cordis.yml` is removed; use the profile bundle shipped with the runtime instead.
- The `assets/cordis.yml` entry in `Cargo.toml [package] include` is removed; the file no longer exists, and the profile bundle replaces the deleted asset.
- `DEFAULT_CORDIS_YML` and `bundled_default_config_path` are removed; use the profile bundle instead.

## [0.1.0] - 2026-08-17

### Changed
- README install instructions no longer pin a point version: the documented command is a bare `cargo add deepseek-harness-sdk` with a `"*"` dependency line, leaving version selection to the user; the pre-release note keeps only channel-level guidance (explicit `@0.1.0-alpha` style request while on a pre-release line).

## [0.1.0-alpha.2] - 2026-08-17

### Added
- GitHub Workflow release pipeline: dispatch **Release prep** → review one `release v<version>` PR → merging publishes (annotated tag → verify → GitHub Release → crates.io via Trusted Publishing; no registry token, no local `cargo publish`).
- Fragment-driven changelog: user-visible changes accumulate as `.changes/unreleased/` fragments and are assembled into `CHANGELOG.md` by the Release prep run.
- Backfilled `0.1.0-alpha.1` history: `CHANGELOG.md` section, git tag `v0.1.0-alpha.1`, and a GitHub Release for the published crate.

### Fixed
- Release prep now opens a new release PR when a previous PR for the same version was already merged (re-prep after a rollback), instead of failing on `gh pr reopen`.

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
