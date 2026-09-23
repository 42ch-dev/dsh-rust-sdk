---
category: Changed
---
- The CI keyless-runtime-handshake job now verifies the npm route against `@deepseek-ai/dsh@0.1.7-rc.1`: the crate's wire and launch contracts were re-verified green against that upstream tag before the pin moved. The exact version under test still lives in `.github/workflows/ci.yml`.
- The fake-runtime suite now locks pass-through of the newest upstream session-event payloads: flattened `role:'tool'` tool results and `developer/message` events flow through parsing, `Session::run` collection, and the notification callback verbatim.
- No public API change.
