# Knowledge

Morning Star knowledge base: distilled implementation SSOT, reusable design decisions.

| Document | Source Plan | Description | Status |
|----------|-------------|-------------|--------|
| [api-design/dsh-sdk-wire-protocol-parity.md](api-design/dsh-sdk-wire-protocol-parity.md) | iteration:v0.1 (01+02); refreshed at iteration-close `003-upstream-launch-contract` (2026-09-08) | DSH wire-protocol **source-verification method** + the three corrected traps (inserted[].id receipt, skip-malformed-lines, stderr tail); wire and launch/env/config contract facts superseded by `.mstar/specs/dsh-sdk-wire-parity-surface.md` + `.mstar/specs/dsh-runtime-launch-contract.md` (pointers inside) | active |
| [best-practices/crates-io-github-release-pipeline.md](best-practices/crates-io-github-release-pipeline.md) | iteration:v0.2 (03); updated at iteration-close `003-upstream-launch-contract` (2026-09-08) | crates.io release pipeline: Trusted Publishing contracts, PR-driven annotated tags, injection/label/error-text traps, fragment changelog determinism rules + per-category/verbatim-rendering/consumer-facing fragment authoring lessons | active |
| [best-practices/dsh-upstream-drift-realignment.md](best-practices/dsh-upstream-drift-realignment.md) | iteration:003-upstream-launch-contract | Reusable discipline for DSH upstream drift: verify launch/wire contracts against the runtime source at a pinned ref, re-align in a tracked plan, compose `dsh --profile` + resolved `DSH_HOME`, keep `DSH_RUNTIME_BIN` as a product surface, document divergences, guard with a keyless handshake + exact-pin CI job | active |

- **Tracked**: yes — results are shared with the team across clones.
- **Authority**: `mstar-compound` skill (bug/knowledge tracks, overlap detection, CONCEPTS.md).
