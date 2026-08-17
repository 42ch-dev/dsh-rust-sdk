---
category: Changed
---
- README Runtime acquisition now defaults to the prebuilt single-file executable (wheel) and demotes npm to a no-build route: npm packages are installed from the coherent `next` dist-tag (the `latest` tags currently form a stale mixed matrix), and the docs call out that the interactive `dsh` CLI (`@deepseek-ai/dsh`) is not the SDK runtime — it does not serve the stdio JSON-RPC protocol.
