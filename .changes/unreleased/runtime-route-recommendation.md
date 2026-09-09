---
category: Changed
---
- The npm-published `dsh` CLI is now the recommended runtime-acquisition route: install the exact CI-verified pin `npm install -g @deepseek-ai/dsh@0.1.3-alpha.2` and point `DSH_RUNTIME_BIN` at it (the bin is a Node.js script, so Node.js must be on `PATH`).
- The platform wheel is now documented after the npm CLI as the self-contained fallback (no system Node.js at runtime), with the published-target matrix corrected to Linux x64, Linux arm64, macOS arm64, and Windows x64 — macOS x64 is not published, so that platform uses the build-from-source route.
- Building from source is documented as the only route that reproduces the exact contract basis this crate was verified against.
