---
category: Changed
---
- The runtime is launched as `dsh --profile <name>` with ordered `--patch <path>` overlays, not a bare program spawn.
- `DSH_HOME` now resolves by upstream precedence — `Config::dsh_home`, then a non-empty `DSH_HOME` environment variable, then `~/.dsh` (blank counts as unset). Unlike the Python SDK, the crate falls back to `~/.dsh` instead of raising `ValueError`; the resolved absolute home is observable via `Config::resolve_dsh_home`.
- The `initialize` handshake is bounded by `Config::initialize_timeout` (default 30 s; `None` is a deliberate opt-out).
- `Error::RequestTimeout` gained a `profile: Option<String>` field naming the selected DSH profile when the handshake had one, so a wedged `initialize` is diagnosable.
