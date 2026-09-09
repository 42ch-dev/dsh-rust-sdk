---
module: deepseek-harness-sdk
date: 2026-09-08
problem_type: best_practice
category: best-practices
severity: high
tags:
  - upstream-drift
  - launch-contract
  - dsh
  - runtime
  - verification
  - dsh-home
  - wire-parity
applies_when:
  - verifying any wire or launch contract against the upstream deepseek-harness runtime
  - upstream replaces a contract the crate depends on
  - removing or renaming an env knob, config field, or bundled asset
  - building the deferred runtime-bin-delivery schemes (B/C)
---

# DSH upstream drift: verify against the runtime source, re-align in a tracked plan

## Context

The v0.1 crate shipped a launch/configuration contract upstream had already replaced. It spawned the runtime as a bare program with zero argv, injected `DSH_CORDIS_CONFIG` / `DSH_SESSION_ROOT` / `DSH_CWD` into the child environment, and bundled a default `cordis.yml` — none of which had a reader in the current upstream runtime, which had become the `dsh` CLI booted under a mandatory `--profile` with a resolved `DSH_HOME`. The break was entirely in the launch/configuration half: the wire protocol re-verified unchanged. Because the crate compiled and its surface tests passed, the failure was silent until a real runtime refused to boot.

The re-alignment was frozen in two repo-level specs that are now the contract SSOT:

- `.mstar/specs/dsh-runtime-launch-contract.md` — launch grammar, `DSH_HOME` resolution, child environment, removal list, `Config` / `RunResult` surface, error semantics.
- `.mstar/specs/dsh-sdk-wire-parity-surface.md` — wire surface, block vocabulary, parity tables, documented divergences.

This document captures the reusable discipline only; the specs own the facts.

## Guidance

1. **Verify contracts against the upstream runtime source, not only the SDK wrappers.** The Python and TypeScript SDKs are clients of the runtime, and clients carry their own choices: Python raises `ValueError` when no `DSH_HOME` is configured, while the runtime's own resolver falls back to `~/.dsh` and TypeScript relies on that default. A wrapper-derived fact can be a wrapper decision, so read the runtime source at a pinned ref and cite `path:line` — that is how every contract statement in the two specs is grounded at upstream ref `c389f96bf3` (launch spec §3.1, §3.3).

2. **When upstream replaces a contract, re-align in a tracked plan rather than absorbing the drift.** The launch-contract goal was a roadmap P0 and was executed end-to-end. The removal list is fixed in the spec with a replacement named per item, and the crate's divergence from Python (following the runtime's `~/.dsh` fallback) is normative: documented in the spec, the README pair, and the changelog fragment, never implied, and not to be "fixed" back without a superseding spec decision (launch spec §3.3, §5).

3. **Compose the launch model; do not spawn a bare program.** The launch is `dsh --profile <name>` argv plus a resolved `DSH_HOME` with precedence `Config::dsh_home` → non-empty `DSH_HOME` → `~/.dsh` (blank counts as unset), resolved to an absolute `~`-expanded path the caller can read back. The env knobs `DSH_CORDIS_CONFIG`, `DSH_SESSION_ROOT`, and `DSH_CWD` have no reader upstream and must never be written (launch spec §2–§4).

4. **Bring-your-own runtime is a supported product surface, not a compatibility shim.** `DSH_RUNTIME_BIN` survives the `runtime_bin` → `dsh_bin` rename as the crate's runtime-path override (`Config::dsh_bin` → non-empty `DSH_RUNTIME_BIN` → `Error::RuntimeNotFound`; empty counts as absent). It is deliberately excluded from the removal list and must not be deleted as dead code (launch spec §5, §8).

5. **Divergences must be documented, never implied.** The crate follows the runtime's `~/.dsh` fallback where the Python SDK raises. Documenting the divergence is what stops a future contributor from "fixing" the crate back into a wrapper's behavior (launch spec §3.3).

6. **Guard the launch contract with a keyless handshake plus an exact-pin CI job.** `tests/real_runtime.rs::real_runtime_handshake` resolves a real `dsh` (from `DSH_RUNTIME_BIN` or `PATH`), boots it with a temp `dsh_home`, and proves `start()` → `initialize` → `close()` with no API key; the live-turn tier stays behind `DEEPSEEK_API_KEY`. The CI job `keyless-runtime-handshake` (`.github/workflows/ci.yml`) installs one exact published version (`@deepseek-ai/dsh@0.1.3-alpha.2`, never a dist-tag) and its resolution step fails loudly when the binary is absent — an empty `DSH_RUNTIME_BIN` would otherwise make the test tier skip and the job go green without ever running the handshake. The keyless tier proves boot only, not a live turn; the README states that limit.

## Why This Matters

The v0.1 break was silent: the crate compiled, unit tests passed, and only a real runtime failed to boot, because the launch contract had drifted and nothing verified it. Wrapper-derived facts can be wrapper choices, so the runtime source is the only authority. A keyless, pinned handshake in CI turns the next upstream drift into a loud failure at merge time instead of a customer-reported boot failure, and the frozen specs give any re-alignment plan a fixed target.

## When to Apply

- Verifying or extending any contract this crate holds with the deepseek-harness runtime, wire or launch.
- Upstream advances past the pinned ref: re-run the specs' re-verification greps before restating any contract line (launch spec §10, wire spec §10).
- Removing or renaming an env knob, config field, or bundled asset: the removal must carry a named replacement and a deprecation note (AC5 pattern; launch spec §5).
- Building the deferred `runtime-bin-delivery` (schemes B/C): the launch grammar locked in the spec is the model both schemes must compose, and `DSH_RUNTIME_BIN` must keep working (launch spec §8).
- Adding a CI job that exercises a real runtime: pin an exact published version, cache npm, and fail loudly when the binary cannot be resolved.

## Examples

### What didn't work (v0.1) — and what replaced it

- **Bare-program spawn with zero argv**: `args: []` is not a valid `dsh` invocation — the child exits 1 before any JSON-RPC frame exists. Replaced by composed argv `--profile <name>` plus ordered `--patch <path>` pairs (launch spec §2).
- **Bundling a dead `cordis.yml`**: `assets/cordis.yml` mounted a package deleted upstream, and the `DEFAULT_CORDIS_YML` / `bundled_default_config_path` channel existed only to inject it. Replaced by the profile tree — no config file is passed to the runtime (launch spec §5).
- **Injecting env knobs with no upstream reader**: the `DSH_CORDIS_CONFIG` / `DSH_SESSION_ROOT` / `DSH_CWD` writes made `Config::session_root` *appear* to control where sessions land while they actually landed under `~/.dsh`. Replaced by never writing them: sessions live under the resolved `$DSH_HOME/sessions`, and the workspace reaches the runtime as `initialize.cwd` (launch spec §4.2).
- **Documenting the old world in the README**: the README pair claimed the interactive `dsh` CLI was "not this SDK's runtime" and advertised a 3-platform matrix, while the CLI *is* the runtime carrier and upstream publishes 5 targets. Replaced by a docs-truth pass restating every fact from the frozen specs.

### Working reference in-repo

- `tests/real_runtime.rs` — the two-tier real-runtime test (keyless handshake tier + gated live-turn tier).
- `.github/workflows/ci.yml` — job `keyless-runtime-handshake` (exact pin, npm cache, fail-loud resolution step).
- `README.md` — "How the SDK resolves the runtime" and the `DSH_HOME` observability section state the contract the crate ships.
