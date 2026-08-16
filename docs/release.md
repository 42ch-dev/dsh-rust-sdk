# Release process

How `deepseek-harness-sdk` ships: a fragment-driven changelog, one reviewable
release PR, and a tokenless publish via crates.io Trusted Publishing. A
non-ritualist can run a release without a local `cargo publish` and without a
registry token.

Reference provenance: the entry/tag/verify/publish architecture and crates.io
Trusted Publishing follow the `spoke` release workflows (`release.yml` /
`new-release.yml`); the fragment/changelog scheme follows `dsh-llm-fallbacks`
(`.changes/`, `prepare-release.ts`, its `docs/release.md`). This document is
the shipped SSOT for this repository — when it and the workflows disagree,
the workflows win.

## Release model

Two steps, PR-driven, zero secrets:

1. **Release prep** (manual `workflow_dispatch` on the **Release prep**
   workflow) resolves the version, assembles `.changes/unreleased/` fragments
   into `CHANGELOG.md`, bumps `Cargo.toml`, archives the consumed fragments,
   and opens or updates the `release v<version>` PR (label `release`).
2. **Merging that PR is what publishes.** The **Release** workflow then
   creates the annotated tag `v<version>`, runs the verify jobs, creates the
   GitHub Release from the changelog section, and publishes to crates.io via
   Trusted Publishing (OIDC).

There is deliberately **no `push: tags` human-publish path**: a manual
`git tag && git push --tags` does not publish, because the tag's tree must
contain `release.yml`, which only release-merged commits have. (The
`push: tags: v*` trigger exists so tags created by the PR path — or a
backfilled tag such as `v0.1.0-alpha.1` — run the full verify/release/publish
pass on their own.)

**Dev-time version discipline:** during development `Cargo.toml` does not
move. Changes accumulate as unreleased fragments; the version is resolved by
the Release prep run (auto bump or explicit input), not edited by hand.

## Fragments — how a change becomes a release note

Every user-visible change ships one fragment file under `.changes/unreleased/`:

````md
---
category: Added
---
- New crate consumer API: `Config::runtime_bin` override.
- Acquisition hints when the runtime is missing.
````

Rules (see also `.changes/unreleased/README.md`):

- One file per user-visible change group; slug filename ending `.md`.
  `README.md` and dotfiles (`.gitkeep`) in the directory are ignored.
- Frontmatter (optional): a single `category:` key fenced by `---`;
  defaults to `Changed`. Recommended values: `Added`, `Changed`,
  `Deprecated`, `Removed`, `Fixed`, `Security`; other values render after
  the canonical six in first-seen filename order.
- Body: English, **crate-consumer facing** — what a consumer can do or must
  know. At least one `- ` bullet line is required. Process trivia (test
  counts, QC rounds, internal git ranges) does not belong in fragments.
- Assembly: a `## [<version>] - <UTC date>` section is inserted directly
  under `## [Unreleased]`; categories render in canonical order; fragments
  sharing a category merge under one heading, ordered by filename
  (lexicographic byte order). Fragment body lines render verbatim.

## Once: one-time setup (owner)

Before the first pipeline publish of `0.1.0-alpha.2`:

1. **crates.io Trusted Publisher** — the crate already exists on crates.io,
   so Settings is available. crates.io → `deepseek-harness-sdk` → Settings →
   **Trusted Publishing** → add a GitHub Actions publisher:
   - Repository: `42ch-dev/dsh-rust-sdk` (owner `42ch-dev`, name
     `dsh-rust-sdk` if the UI splits the fields)
   - Workflow filename: `release.yml` (filename only, including `.yml`; it
     must live under `.github/workflows/`)
   - Environment name: leave empty (this iteration does not use GitHub
     Environment protection)
   - Allowed action: `cargo publish`
   - Save. This creates **no token** — auth is pure OIDC from the workflow's
     `id-token: write`.
2. **GitHub org: Actions may create PRs** — enable the org setting
   "Allow GitHub Actions to create and approve pull requests" so the
   built-in `GITHUB_TOKEN` can open the release PR. The happy path adds no
   secret. If the org blocks it, the documented last resort is a PAT secret
   used only to open the release PR — it is **not** a crates.io credential
   and is **not** pre-created.

## Per release: the ritual

1. **Write fragments** — one file per user-visible change under
   `.changes/unreleased/` (format above). Do not edit the `Cargo.toml`
   version. At least one collectable fragment is mandatory or Release prep
   refuses.
2. **Dispatch** — repository → Actions → **Release prep** → Run workflow.
   Leave the version input empty for an auto bump (current
   `0.1.0-alpha.1` → `0.1.0-alpha.2`), or type an explicit SemVer
   (`X.Y.Z` or `X.Y.Z-pre.N`). The run always prepares from `main` and never
   touches `main` directly.
3. **Review the one PR** titled `release v<version>` (label `release`):
   - [ ] `Cargo.toml` `[package] version` is the expected version
     (`Cargo.lock` too, if the bump moved it)
   - [ ] `CHANGELOG.md` has a `## [<version>] - <UTC date>` section directly
     under `## [Unreleased]` with correct English bullets
   - [ ] `.changes/unreleased/` collectable files moved to
     `.changes/archive/<version>/`
   - [ ] the diff is only version / changelog / archive (plus lockfile if
     needed) — no surprise product-code files
4. **Merge.** Merging is what publishes. After merge, watch the **Release**
   workflow: tag → verify (fmt, build, clippy, test) → GitHub Release →
   `cargo publish`.

## What failure looks like

Failures are loud; there is no silent skip and no token fallback.

| Failure | Operator-visible result | Recovery |
|---------|-------------------------|----------|
| Empty `.changes/unreleased/` (`README.md` / `.gitkeep` ignored) | Release prep fails; no PR opened | Add a fragment, re-dispatch |
| Invalid SemVer, version ≤ current `Cargo.toml`, or git tag `v<version>` already exists | Release prep fails; no PR | Fix the version input, re-dispatch |
| CHANGELOG section for the version is empty (no `- ` bullet) | Release prep refuses to open the PR; the Release workflow also fails **before** GitHub Release and before publish | Do not ship an empty release; fix fragments / re-run Release prep |
| crates.io Trusted Publisher missing or removed | `publish-crates` fails with an OIDC/auth error. Tag + verify + GitHub Release may already have succeeded. There is no `CARGO_REGISTRY_TOKEN` secret to fall back to | Configure the Trusted Publisher (once-step 1), retry the `publish-crates` job. **Accepted intermediate** for the first `0.1.0-alpha.2` run: this does not un-ship the iteration; the run is complete only when crates.io `max_version` is `0.1.0-alpha.2` |
| Existing **lightweight** tag `v<version>` | `tag` job errors (annotated-only policy) | Replace with an annotated tag, or pick a higher version |
| Existing **annotated** tag `v<version>` | `tag` job continues (idempotent) | Inspect; do not re-publish the same crates.io version (fix-forward) |

Release prep never pushes to `main`. A closed or unmerged release PR is the
rollback gate: nothing is published until merge.

## Rollback

| Situation | Action |
|-----------|--------|
| Release prep stage (PR open, not merged) | Close the PR, or re-run Release prep — it regenerates the branch and PR idempotently (force-with-lease push; a closed PR is reopened; a merged PR fails loudly). Nothing is published until merge |
| Publish succeeded, tag / GitHub Release steps failed | Re-run the failed jobs — the publish step is idempotent (crates.io pre-check + tolerates `already exists on crates.io`), so a re-run is safe and green — or manually `git tag -a v<version>` and create the GitHub Release from `cargo xtask release-notes <version>` |
| Published wrong content | crates.io forbids re-publishing a version; fix-forward to the next version |
| Wrong version merged | Treat as released if the publish ran; otherwise close the PR / re-run Release prep |
| Trusted Publisher missing after merge | Tag + GitHub Release may already exist; configure the Trusted Publisher and retry `publish-crates` only (accepted intermediate for `0.1.0-alpha.2`) |

## Local development

The xtask CLI mirrors what the workflows run, for local rehearsal:

```sh
# Resolve + bump + assemble + archive in the working tree (auto bump, or an
# explicit version). Mutates the tree in place; does NOT commit and does NOT
# refuse a dirty tree — commit the four paths like the workflow does:
#   git add Cargo.toml CHANGELOG.md .changes/ Cargo.lock
cargo xtask release-prepare --auto            # 0.1.0-alpha.1 -> 0.1.0-alpha.2
cargo xtask release-prepare 0.1.0-alpha.2     # explicit

# Validate a prepared release: tag format v+SemVer, Cargo.toml version == tag
# version, git tag absent, CHANGELOG section present.
cargo xtask release-validate v0.1.0-alpha.2

# Print a version's CHANGELOG section (header included); exit 1 when missing.
cargo xtask release-notes 0.1.0-alpha.1
```

Exit codes: 0 ok / 1 guard or validation failure / 2 usage. Run the full
local gate before merging anything release-related:
`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo build --all-targets && cargo test`.
