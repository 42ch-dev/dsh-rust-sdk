---
category: Fixed
---
- Release prep now opens a new release PR when a previous PR for the same version was already merged (re-prep after a rollback), instead of failing on `gh pr reopen`.
