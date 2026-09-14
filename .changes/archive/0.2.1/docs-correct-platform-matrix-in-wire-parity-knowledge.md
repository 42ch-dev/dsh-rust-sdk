---
category: Changed
---
- The runtime platform matrix is documented as a pointer to the upstream published wheel list rather than a fixed enumeration, so the targets a consumer is told about stay in step with what upstream actually publishes (Windows x64 included; macOS needs the sibling `-spawn-helper`).
