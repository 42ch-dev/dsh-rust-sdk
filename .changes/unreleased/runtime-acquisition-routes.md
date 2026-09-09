---
category: Changed
---
- README Runtime acquisition now defaults to the prebuilt single-file executable (wheel) and no longer documents an npm route: the two documented routes are the platform wheel and building from source, and the docs state that the interactive `dsh` CLI (`@deepseek-ai/dsh`) is the SDK runtime — this crate spawns it as a subprocess (`dsh --profile sdk`) and speaks its stdio JSON-RPC protocol.
