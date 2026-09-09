---
category: Added
---
- New `Config` fields `profile` (default `"sdk"`), `patches`, and `dsh_home`.
- `Config::reasoning_effort` — sent on `initialize` as the wire key `reasoningEffort` only when set to a non-empty string, and omitted entirely when unset or blank.
- `Session::run` accepts an `on_notification` callback that observes every notification delivered to the session-tree subscription, in wire order.
- `ContentBlock::File` — the sixth typed block variant, `FileAttachmentRef {attachmentId, name, bytes}`.
- `ImageAttachmentRef::original_dimensions` — carried on the wire as `originalDimensions` so a parsed image block round-trips without loss.
- `DeepSeekHarness::dsh_home` — reads back the resolved absolute home (the same value `Config::resolve_dsh_home` computes).
