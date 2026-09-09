---
category: Added
---
- CI now enforces the changelog-fragment discipline: every PR changing user-visible behavior must add or modify a `.changes/unreleased/` fragment in the same PR; Release-prep output is exempt (recognized by a `release/*` head branch or a diff confined to the paths Release prep writes).
- `AGENTS.md` documents the fragment rule: one fragment per user-visible change (frontmatter `category:` + English bullets), `CHANGELOG.md` is machine-assembled and never hand-edited.
