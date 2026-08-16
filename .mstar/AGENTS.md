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

Resolution rules (find-first, never above workspace root) and the empty-dir rule: `mstar-plan-conventions` skill.

## Content Boundaries

| Area | Content |
|------|---------|
| `docs/` | Human docs: installation, contribution |
| `{SPECS_DIR}` | Frozen specs / ADRs |
| `{ITERATION_DIR}` | Iteration packages (compass + guides/specs) |
| `{KNOWLEDGE_DIR}` | Implementation SSOT, reusable design |
| `{PLAN_DIR}/` | Main plans, durable gate summaries, optional residual prose |

QC/QA raw process reports go to `{SDD_DIR}/review/` (gitignored), **not** `docs/` or `{PLAN_DIR}`. Open residual state is SSOT in `status.json`, not in plan prose.

## State Machine & Done

`Todo → InProgress → InReview → Done | Blocked` — `Done` may only be set by `project-manager` or `qa-engineer`. Implement roles may set `InReview`, never `Done`. `status.json` is the durable SSOT for plans, residuals, and metadata.

## QC / QA Alignment

- `Execution mode: sdd` (default for multi-task code plans) ⇒ mandatory plan QC tri-review (`qc-specialist`, `qc-specialist-2`, `qc-specialist-3`) → `{SDD_DIR}/review/qc1.md…qc3.md` + `qc-consolidated.md`.
- `inline` (hotfix) ⇒ single-seat QC (`{SDD_DIR}/review/qc.md`).
- Runtime/behavior changes require a recorded `QA gate` decision (`mandatory` or `pm-acceptance` per `qa-trigger-matrix.md`).

## Residual Lifecycle

- Open residuals: root `status.json` → `residual_findings[<plan_id>]` (severity enum: critical/warning/suggestion; canonical strings from `mstar-plan-artifacts`).
- Closed residuals: archived to `{HARNESS_DIR}/archived/residuals/<plan-id>.json`.
- Residual prose may also live in the main plan, but `status.json` is the SSOT.

## Plan Compaction Profile

This repository uses **Profile A** from the Morning Star `mstar-plan-artifacts` skill (`references/done-compaction.md`).

- `status.json.plans[]` keeps active plans and may keep **slim `Done` rows**.
- `archived/plans/<plan-id>.json` is used as cold snapshot when available.
- Historical tooling may read both `status.json.plans[]` and `archived/plans/`.

## Git Tracking Policy (process vs results)

Principle: **process stays local, results are shared.**

- Tracked: `AGENTS.md` (this file), `knowledge/**`, `specs/**`.
- Gitignored (local session SSOT): `plans/`, `iterations/`, `sdd/`, `status.json`, `notes.json`, `archived/`.
- Cross-clone handoff happens via knowledge + specs + this file + root `AGENTS.md` — never by committing `status.json` or `plans/` by default.
- Default working style: feature branches (see root `AGENTS.md`); branch/worktree/QC-checkout alignment: `mstar-branch-worktree` skill.
