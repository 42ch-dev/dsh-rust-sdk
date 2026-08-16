# AGENTS.md — dsh-rust-sdk

## Repository

Rust SDK for DSH (DeepSeek Harness). Builds with Cargo (`cargo build` / `cargo test`); formatting via `cargo fmt`, linting via `cargo clippy`.

## Source Priority

1. Current user instruction
2. This file (repo-level durable constraints only)
3. `.mstar/AGENTS.md` (Morning Star harness subtree contract — SSOT for plans, status, residuals, QC/QA gates)
4. `mstar-*` skills (harness process rules; `mstar-harness-core` is the entry)

## Boundaries

- Rust SDK only — no unrelated languages/toolchains in this repo.
- Public API must be `rustdoc`-documented; breaking changes require a spec/ADR in `.mstar/specs/`.
- Keep `docs/` for human-facing docs; plans and specs belong in `.mstar/` (see `.mstar/AGENTS.md`).

## Build & Test (interface)

- `cargo build` — compiles the workspace.
- `cargo test` — runs the suite (default before merge).
- `cargo fmt --check` / `cargo clippy -- -D warnings` — style/lint gates.

## Git & Branch Policy

- Default working style: feature branches off `main`; no direct pushes to `main`.
- Branch/worktree alignment and QC checkout rules: `mstar-branch-worktree` skill; status/residual SSOT: `status.json` (see `.mstar/AGENTS.md`).
- Never commit `status.json`, `plans/`, `iterations/`, `sdd/`, `notes.json` by default (process-local).

## Escalation

- Ambiguous acceptance, conflicting review verdicts, repeated failures, or non-converging root cause → escalate to `project-manager` with status, options, and recommended path.
