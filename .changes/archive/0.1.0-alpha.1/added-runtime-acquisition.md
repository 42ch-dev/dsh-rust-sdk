---
category: Added
---
- Bring-your-own runtime (Plan A): the binary is resolved from `Config::launch_args_override` (full argv, verbatim), `Config::runtime_bin`, or the `DSH_RUNTIME_BIN` environment variable — the crate never downloads, bundles, or ships a runtime.
- Bundled default `cordis.yml` injection (byte-identical to the official default) when no `DSH_CORDIS_CONFIG` is set, plus environment injection of `DSH_CWD`, `DSH_SESSION_ROOT`, and model credentials.
- A missing runtime fails fast with `Error::RuntimeNotFound`, whose message names the acquisition routes and points to https://github.com/deepseek-ai/deepseek-harness.
