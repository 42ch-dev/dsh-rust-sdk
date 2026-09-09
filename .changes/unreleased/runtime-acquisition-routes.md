---
category: Changed
---
- README Runtime acquisition now documents three runtime routes — the pinned npm CLI (`@deepseek-ai/dsh@0.1.3-alpha.2`, recommended), the platform wheel (self-contained, no Node.js), and build from source — and states that the interactive `dsh` CLI (`@deepseek-ai/dsh`) is the SDK runtime this crate spawns as a subprocess (`dsh --profile sdk`) and speaks to over stdio JSON-RPC.
