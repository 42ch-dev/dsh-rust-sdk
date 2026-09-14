---
category: Fixed
---

- `RunResult::final_response` now documents its actual text rule: a string
  `text` contributes its value, while `null`, a missing `text`, or any other
  non-string `text` contributes `""`. The rustdoc and the wire-parity spec no
  longer claim Python parity for that rule — the Python SDK coerces a *truthy*
  non-string `text` through `str()` (`42` → `"42"`, `true` → `"True"`), and
  the crate's decision not to emulate that coercion is a recorded divergence.
  Behavior is unchanged.
