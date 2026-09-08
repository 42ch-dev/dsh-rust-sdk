# Specs

Repo-level normative specs (`{SPECS_DIR}` = `.mstar/specs/`): frozen, cross-iteration contracts. Iteration-scoped drafts belong in `.mstar/iterations/<iteration-id>/specs/`; implementation SSOT belongs in `.mstar/knowledge/`.

| Spec | Status | Scope | Owning plans | Iteration |
|------|--------|-------|--------------|-----------|
| [`dsh-runtime-launch-contract.md`](dsh-runtime-launch-contract.md) | Frozen (2026-09-08) | Launch grammar, `DSH_HOME` resolution + Python divergence, child environment, removal list with replacements, `Config` / `RunResult` field sets, error semantics, deferred `runtime-bin-delivery` interaction, verification obligations | 05-runtime-launch-contract (primary); 07-docs-and-live-verification (documents it) | `003-upstream-launch-contract` |
| [`dsh-sdk-wire-parity-surface.md`](dsh-sdk-wire-parity-surface.md) | Frozen (2026-09-08) | Request methods, notifications, session-tree semantics, content-block vocabulary incl. `file`, field-level Python-parity tables, `reasoningEffort` omit-when-unset, bounded `initialize`, notification callback, documented divergences | 06-wire-parity-additions (primary); 07-docs-and-live-verification (primary) | `003-upstream-launch-contract` |

## Reading rules

- Both specs share one upstream basis (`deepseek-harness` @ `c389f96bf3`) and one normative language (`MUST` / `MUST NOT` are contract; `SHOULD` is strong guidance that a deviation must justify; prose without these words is rationale).
- Where the two touch the same surface, `dsh-runtime-launch-contract.md` owns the field-by-field `Config` / `RunResult` contract (§6.1–§6.2) and `dsh-sdk-wire-parity-surface.md` owns the wire half and the parity verdicts (§6), cross-referencing the launch spec rather than restating it.
- A plan that disagrees with a spec is drift: the spec wins, and the plan is corrected before implement.
- Changing a clause requires a superseding spec decision; the re-verification rule at the end of each spec governs upstream citation drift.
