---
category: Fixed
---

- `RunResult::final_response` treats a non-string `text` block (including
  `null`) as `""`. That is a recorded divergence from the Python SDK, which
  coerces a truthy non-string via `str()` (`42` → `"42"`, `true` → `"True"`).
  There is no runtime behavior change.
