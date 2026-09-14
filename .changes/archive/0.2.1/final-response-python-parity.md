---
category: Fixed
---
- `RunResult::final_response` now falls back to an earlier root `assistant/message`
  when the last one is malformed (non-object `data` or non-array `content`),
  matching the Python SDK's `final_response` reversed-scan algorithm. The
  previous implementation returned `""` instead of trying an earlier event.
- Corrected the public rustdoc on `RunResult::final_response` and the normative
  spec §6.2, which both incorrectly claimed `final_response` "never falls back
  to an earlier event" while citing the Python source that does fall back.
  The behavior is unreachable from a conformant runtime (which always emits
  `message.content` as a JSON array) but the algorithm and docs now agree.
