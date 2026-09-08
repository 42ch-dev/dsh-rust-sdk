---
category: Removed
---
- `Config::session_root` is removed; use `Config::dsh_home` — sessions live under the resolved `$DSH_HOME/sessions` instead.
- `Config::cordis_config` and the `DSH_CORDIS_CONFIG` environment injection are removed; use the profile tree (`Config::profile` + `Config::patches`) — no config file is passed to the runtime.
- The `DSH_SESSION_ROOT` and `DSH_CWD` environment injections are removed; sessions live under the resolved `DSH_HOME`, and the workspace directory reaches the runtime as `Config::cwd` (sent as `initialize.cwd`).
- `RunResult::session_root` is dropped with no replacement — upstream removed it and asserts its absence.
- `Config::launch_args_override` is removed; use `Config::dsh_bin` + `Config::profile` + `Config::patches` — the launch is composed from typed fields, not an opaque argv.
- `Config::runtime_bin` is renamed to `Config::dsh_bin` (the `DSH_RUNTIME_BIN` environment override is preserved).
- The bundled `assets/cordis.yml` is removed; use the profile bundle shipped with the runtime instead.
- `DEFAULT_CORDIS_YML` and `bundled_default_config_path` are removed; use the profile bundle instead.

### Changed
- The runtime is launched as `dsh --profile <name>` with ordered `--patch <path>` overlays, not a bare program spawn.
- `DSH_HOME` now resolves by upstream precedence — `Config::dsh_home`, then a non-empty `DSH_HOME` environment variable, then `~/.dsh` (blank counts as unset). Unlike the Python SDK, the crate falls back to `~/.dsh` instead of raising `ValueError`; the resolved absolute home is observable via `Config::resolve_dsh_home` / `DeepSeekHarness::dsh_home`.
- The `initialize` handshake is bounded by `Config::initialize_timeout` (default 30 s; `None` is a deliberate opt-out).
- `Error::RequestTimeout` gained a `profile: Option<String>` field naming the selected DSH profile when the handshake had one, so a wedged `initialize` is diagnosable.

### Added
- New `Config` fields `profile` (default `"sdk"`), `patches`, and `dsh_home`.
- `Config::reasoning_effort` — sent on `initialize` as the wire key `reasoningEffort` only when set to a non-empty string, and omitted entirely when unset or blank.
- `Config::initialize_timeout` — bounds the `initialize` handshake (default 30 s; `None` = unbounded).
- `Session::run` accepts an `on_notification` callback that observes every notification delivered to the session-tree subscription, in wire order.
- `ContentBlock::File` — the sixth typed block variant, `FileAttachmentRef {attachmentId, name, bytes}`.
- `ImageAttachmentRef::original_dimensions` — carried on the wire as `originalDimensions` so a parsed image block round-trips without loss.
