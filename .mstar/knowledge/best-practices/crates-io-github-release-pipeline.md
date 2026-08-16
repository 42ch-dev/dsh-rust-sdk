---
title: "crates.io release pipeline on GitHub Workflows — trusted publishing, PR-driven tags, fragment changelogs"
problem_type: best_practice
category: best-practices
module: release-pipeline
date: 2026-08-17
severity: high
plan_id: 03-release-pipeline
tags:
  - github-actions
  - crates-io
  - trusted-publishing
  - release
  - changelog
  - rust
applies_when:
  - adding or altering release workflows for Rust crates in this org
  - publishing a companion crate or stabilizing 0.1.0 through the pipeline
  - extending the .changes fragment changelog scheme
source_iteration: v0.2
status: stable
---

# crates.io release pipeline on GitHub Workflows

## Context

v0.2 shipped a two-step, PR-driven release pipeline for `deepseek-harness-sdk` (`.github/workflows/release-prep.yml` + `release.yml`, `xtask` changelog tooling, `docs/release.md` SOP). The architecture follows 42ch/spoke (`release.yml` + `new-release.yml`); changelog rules follow omdsh-dev/dsh-llm-fallbacks (`.changes/` fragments). Both references were read in full during Phase 1 and several traps were found only by source-verification or live review — recorded here so the next pipeline (or the next iteration touching this one) does not rediscover them.

## Guidance

**Trusted Publishing (OIDC) for crates.io:**
- `rust-lang/crates-io-auth-action` (pinned `c6f97d42243bad5fab37ca0427f495c86d5b1a18` = v1.0.5) exchanges the workflow's OIDC id-token for a short-lived `CARGO_REGISTRY_TOKEN`; its `post:` step revokes it. Requires `permissions: id-token: write` on **only** the publish job.
- The trusted publisher binds to the **workflow filename** (`release.yml`) — the workflow must be top-level (no `workflow_call`), and the publisher entry (repo, workflow filename, allowed action `cargo publish`) is configured once in crates.io package settings. Missing entry → loud auth failure; re-run the publish job after configuring — the rest of the release (tag, GitHub Release) is idempotent.
- Zero registry secrets in the repo is achievable and worth it: the idempotent publish step (GET pre-check on `https://crates.io/api/v1/crates/<name>/<version>` + tolerate duplicate) makes job re-runs safe.

**Publish-race tolerance must match crates.io's actual error text.** crates.io returns HTTP 400 with `crate version \`X\` is already uploaded` (verified in rust-lang/crates.io `src/controllers/krate/publish.rs`), which cargo surfaces with that same phrase. A grep for a paraphrase like `'already exists on crates.io'` is **dead code** — it can never match. Grep for `is already uploaded`.

**PR-driven tags (spoke pattern):**
- `pull_request: closed` on main + exact `release` label → tag job creates the **annotated** tag from `Cargo.toml` version at the merge commit; the tag push re-enters via `push: tags: v*` for verify + release + publish.
- Label guards must use **exact array membership**: `contains(github.event.pull_request.labels.*.name, 'release')`. The common `contains(join(...labels.*.name, ','), 'release')` is a substring match — `pre-release` passes it (found in QC).
- Guard the tag job also on head-ref prefix `release/v` + PR-title ↔ `Cargo.toml` version cross-check (fallbacks pattern) so a hand-titled PR cannot tag.
- Annotated-only policy: `git cat-file -t` must be `tag`; existing annotated → continue (idempotent), lightweight → hard error.
- A tag push can only trigger workflows **present in the pushed tree** — backfilled tags on pre-pipeline commits cannot trigger anything (verified with `git ls-tree`).
- `gh release create` after the tag push; `--target` needs the **full 40-char SHA** (short SHAs get HTTP 422).

**Expression injection:** never splice `${{ github.* }}` into `run:` scripts (git refnames legally contain shell metacharacters). Pass via `env:` and reference quoted `"$VAR"` — apply to `ref_name`, `event_name`, `merge_commit_sha`, PR titles, all of them.

**Fragment changelogs (fallbacks scheme, Rust xtask implementation):**
- `.changes/unreleased/*.md` (frontmatter `category:`, ≥1 `- ` bullet) → assembled into `## [<version>] - <UTC date>]` under `## [Unreleased]` → archived to `.changes/archive/<version>/`. During development the crate version does not move — the version is resolved only when Release prep runs.
- Determinism rules that mattered: lexicographic byte-order filename sort; fixed canonical category order then first-seen; same-category fragments merge under one `###` heading; existing CHANGELOG without `## [Unreleased]` header = error (never guess insertion point).
- Refuse-empty lives in **three layers** on purpose: prepare refuses 0 fragments; `release-notes` exits 1 on missing section; workflows grep for `^[[:space:]]*- ` (header-only sections must be treated as empty — `release-notes` alone exits 0 on those).
- Auto-bump must stay on the prerelease line: `X.Y.Z-pre.N → X.Y.Z-pre.(N+1)` (numeric tail only; non-numeric tail → demand explicit version). Reject build metadata on explicit input (crates.io won't publish `+build` versions).
- Committed CHANGELOG sections should be byte-reproducible from the archived fragments — keep a regression test asserting it.

**Workflow/tooling details that bit us:** machine-global `tag.gpgSign=true` breaks git fixtures in tests (use `-c tag.gpgSign=false`); `cargo xtask` alias needs a committed `.cargo/config.toml`; a root-package workspace needs `default-members = [".", "xtask"]` for plain `cargo test` to cover xtask; `cargo publish -p <name>` only (never bare `--workspace`); keep xtask out of the crate `include` allowlist; rust-cache `shared-key` must match ci.yml for the verify jobs to reuse PR cache.

## Why This Matters

The next iteration (`runtime-bin-delivery` companion crate, or `0.1.0` stabilization) will publish through this pipeline; anyone adding a second crate or a release-lint job reuses these contracts. The traps above are invisible until a first live run — the expensive kind of discovery.

## When to Apply

- Adding/altering release workflows for any Rust crate in this org.
- Publishing a new companion crate (Trusted Publisher entry must be configured per-package; first publish of a brand-new package cannot use Trusted Publishing until it exists on crates.io — plan a bootstrap).
- Extending the changelog scheme (new categories, automation).

## Examples

- Working reference in-repo: `.github/workflows/release-prep.yml`, `.github/workflows/release.yml`, `xtask/`, `docs/release.md`, `CHANGELOG.md` + `.changes/`.
- Provenance: 42ch/spoke `.github/workflows/release.yml`/`new-release.yml` (entry/tag/verify/publish + crates-io-auth-action), omdsh-dev/dsh-llm-fallbacks `.changes/` + `scripts/prepare-release.ts` (fragment scheme, auto-bump, validate-before-output).
