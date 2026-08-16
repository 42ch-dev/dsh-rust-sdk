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
- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` — style/lint gates.
- Toolchain: `rust-toolchain.toml` pins `channel = "stable"`, `profile = "minimal"`; MSRV statement is "current stable".
- CI (`.github/workflows/ci.yml`): same gates on `pull_request` and `push` to `main` / `iteration/**`; pinned action SHAs, per-ref concurrency, rust-cache.

## Git & Branch Policy

- Default working style: feature branches off `main` + PR; no direct pushes to `main` (repo rule "Changes must be made through a pull request"). Small doc/toolchain chores may go direct on owner instruction.
- Branch/worktree alignment and QC checkout rules: `mstar-branch-worktree` skill; status/residual SSOT: `status.json` (see `.mstar/AGENTS.md`).
- Never commit `status.json`, `plans/`, `iterations/`, `sdd/`, `notes.json` (process-local).
- `.mstar/roadmap.md` is **local-only**: untracked, gitignored; do not `git add` it. Cross-clone durable roadmap content belongs in `.mstar/knowledge/` or this file.

## Bilingual README rule

- `README.md` (English) and `README.zh.md` (Chinese) are a **parity pair**: same sections, same order, same facts, equal authority. Never edit one without bringing the other along in the same change.
- `README.i18n.yaml` records the git blob hash of each side at the last confirmed-consistent state. After editing either side, update the other, then re-record: `git hash-object README.md` / `git hash-object README.zh.md`.
- Head-of-file language switcher on both sides: EN `English | [中文](README.zh.md)`; ZH `[English](README.md) | 中文`.
- Upstream DSH references only via `https://github.com/deepseek-ai/deepseek-harness` (citation rule); no local filesystem paths in either README.

## Escalation

- Ambiguous acceptance, conflicting review verdicts, repeated failures, or non-converging root cause → escalate to `project-manager` with status, options, and recommended path.
