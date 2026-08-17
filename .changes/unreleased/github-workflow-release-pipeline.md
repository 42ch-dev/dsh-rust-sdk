---
category: Added
---
- GitHub Workflow release pipeline: dispatch **Release prep** → review one `release v<version>` PR → merging publishes (annotated tag → verify → GitHub Release → crates.io via Trusted Publishing; no registry token, no local `cargo publish`).
- Fragment-driven changelog: user-visible changes accumulate as `.changes/unreleased/` fragments and are assembled into `CHANGELOG.md` by the Release prep run.
- Backfilled `0.1.0-alpha.1` history: `CHANGELOG.md` section, git tag `v0.1.0-alpha.1`, and a GitHub Release for the published crate.
