---
category: Fixed
---
- Runtime acquisition now documents all five targets upstream publishes for the
  runtime — **Linux x64, Linux arm64, macOS arm64, macOS x64, and Windows
  x64** — and no longer states that macOS x64 has no wheel. A macOS x64 wheel
  is published, so Route B (the self-contained wheel) works there; Route C
  (build from source) remains the route that reproduces the exact contract
  basis this crate was verified against, and the fallback for any platform with
  no published wheel.
