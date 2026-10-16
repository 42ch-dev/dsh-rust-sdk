# AGENTS.md — Morning Star harness (`.mstar/`)

Harness-subtree contract for this repository. Full rules live in the `mstar-*` skills (Morning Star harness); this file only pins the local contract and points at the SSOT. Do not duplicate the harness manual here.

## Source Priority

1. Current user instruction
2. Root `AGENTS.md` (repo-level durable constraints)
3. This file
4. `mstar-*` skills (`mstar-harness-core` is the global entry and arbiter)

## Path Symbols (SSOT)

| Symbol | Path | Tracked (git) |
|--------|------|---------------|
| `{HARNESS_DIR}` | `.mstar/` | partial |
| `{PLAN_DIR}` | `.mstar/plans/` | no (process-local) |
| `{SDD_DIR}` | `.mstar/sdd/<plan-id>/` | no (runtime scratch + review bundles) |
| `{ITERATION_DIR}` | `.mstar/iterations/` | no (process-local) |
| `{KNOWLEDGE_DIR}` | `.mstar/knowledge/` | yes (results) |
| `{SPECS_DIR}` | `.mstar/specs/` (default; fallback `docs/specs/`, root `specs/`) | yes (results) |
| `{WORKFLOW_DIR}` | `.mstar/workflows/` | no (process-local) |
| `{PROJECT_DIR}` | `.mstar/projects/` | no (process-local) |
| `{HARNESS_DIR}/store.db` | ACTIVE execution authority (root register, workflow/plan rows) + issue/catalog/roadmap/milestone store | no (process-local) |

Resolution rules (find-first, never above workspace root) and the empty-dir rule: `mstar-conventions` skill.

## Content Boundaries

| Area | Content |
|------|---------|
| `docs/` | Human docs: installation, contribution |
| `{SPECS_DIR}` | Frozen specs / ADRs |
| `{ITERATION_DIR}` | Iteration packages (compass + guides/specs) |
| `{KNOWLEDGE_DIR}` | Implementation SSOT, reusable design |
| `{PLAN_DIR}/` | Main plans, durable gate summaries, optional residual prose |
| `{PROJECT_DIR}/<id>/` | Project layer: `references/` research corpus; `roadmap.md` / `residuals.json` are legacy migration/history files only |
| `archived/` | Retained local history (retired legacy sources, store-migration archives) |

QC/QA raw process reports go to `{SDD_DIR}/review/` (gitignored), **not** `docs/` or `{PLAN_DIR}`. Open findings are issues in `{HARNESS_DIR}/store.db`, never plan prose.

## State Machine & Done

`Todo → InProgress → InReview → Done | Blocked` — `Done` may only be set by `project-manager` or `qa-engineer`. Implement roles may set `InReview`, never `Done`. Execution state is SSOT in `.mstar/store.db` (`execution_*` rows, driven through the ordinary `mstar plan` / `mstar workflow` operations); `status.json` and `workflows/<id>/snapshot.json` are migration sources / retained history and are never a live execution route.

## QC / QA Alignment

- `Execution mode: sdd` (default for multi-task code plans) ⇒ mandatory plan QC tri-review (`qc-specialist`, `qc-specialist-2`, `qc-specialist-3`) → `{SDD_DIR}/review/qc1.md…qc3.md` + `qc-consolidated.md`.
- `inline` (hotfix) ⇒ single-seat QC (`{SDD_DIR}/review/qc.md`).
- Runtime/behavior changes require a recorded `QA gate` decision (`mandatory` or `pm-acceptance` per `qa-trigger-matrix.md`).

## Findings / Issues

- A confirmed finding is captured as an issue in `{HARNESS_DIR}/store.db` — plan-scoped `mstar plan issue-add`, unscoped `mstar issue add`; a recurrence appends an occurrence, never a second issue.
- Closure is a separate authorized act by the closure authority: `mstar issue close | waive | duplicate | supersede`, plan-scoped `mstar plan issue-close`.
- `projects/<id>/residuals.json` is migration history: never a write target, never dual-written.
- `severity` is a machine enum (`mstar-artifacts` is the SSOT); QC report section titles (Critical / Warning / Suggestion) are not severity values.

## Git Tracking Policy (process vs results)

Principle: **process stays local, results are shared.**

- Tracked: `AGENTS.md` (this file), `knowledge/**`, `specs/**`.
- Gitignored (process-local): `store.db`, `plans/`, `iterations/`, `sdd/`, `workflows/`, `projects/`, `archived/`, and the retired migration sources `status.json` / `workflows/<id>/snapshot.json`.
- Cross-clone handoff happens via knowledge + specs + this file + root `AGENTS.md` — never by committing `status.json`, `plans/`, or `store.db`.
- Default working style: feature branches (see root `AGENTS.md`); branch/worktree/QC-checkout alignment: `mstar-branch-worktree` skill.
