---
module: harness-process
date: 2026-10-17
problem_type: best_practice
category: best-practices
severity: medium
plan_id: 12-dsh-0.2.1-rc1-adaptation
tags:
  - plan-authoring
  - acceptance-criteria
  - review-findings
  - scope-boundaries
applies_when:
  - authoring a plan's acceptance criteria or a task's dispatch Assignment
  - a review round returns findings that look like the criterion's own wording rather than the work
  - writing a check whose expected result is a count, a "none anywhere" property, or a whole-corpus state
---

# Plan acceptance criteria must state their scope boundary

## Context

On a six-task plan, four of the six review rounds returned findings whose root cause was an acceptance criterion's wording rather than the implementation:

| Criterion as written | What the seat did | Effect |
|---|---|---|
| `grep -n "<version>" file` shows **exactly 3 hits** | satisfied it by splitting the tag literal so a fourth substring hit could not occur | the shipped prose was distorted to fit the check |
| Neither README contains `<version>` or **any `@<semver>` pin** | applied it literally and found pre-existing text the criterion had never meant to cover | a critical raised against untouched text |
| Any inventory row fails → **STOP** | hit an upstream enumeration that had gained members the crate never reads | a STOP rule that could not tell additive drift from a changed obligation |

The checks were reasonable in intent and under-specified in scope. Each had to be corrected after the round, in a recorded plan write-back — no implementer's work was at fault.

## Guidance

1. **State what the check covers and what it does not.** A criterion that verifies a substring count must say whether other occurrences of that substring are in scope; one that bans a pattern must say whether it applies to the whole file or only to the lines this change added. Unowned scope gets resolved by the reviewer — against you, because a reviewer executes the criterion literally.

2. **Never make a count the criterion when the quantity is not stable.** Identify each site by its own surrounding text rather than counting hits: a count silently binds unrelated text, and the cheapest way to satisfy it is to edit that text.

3. **Scope "nothing anywhere" claims to the diff.** A repository-wide negative property has no termination rule. Bound it to the changed files plus directly affected interfaces, or name the budget that ends the search.

4. **Carve out pre-existing state explicitly.** When a check could match text this change did not author, say so in the criterion and record why the pre-existing item is exempt. Without that, the reviewer either raises a false finding or has to guess your intent.

5. **Let a STOP rule separate additive drift from a changed obligation.** "Any row fails → stop" cannot: an upstream enumeration that gained members the crate never touches is a documentation refresh, while a changed method string or name literal is a plan-level decision. Name both branches and route each.

6. **Fix the criterion, not the artifact, when the criterion is wrong.** A finding that traces to a criterion's wording is a plan defect; the fix belongs in a recorded plan write-back, and the corrected criterion should be handed to the seat for revalidation. Rewriting an artifact to satisfy a bad check leaves the next reader with prose shaped by the check.

## Why This Matters

A review seat applies the criterion it was given, and a plan is the executor's only context. Under-specified scope turns a healthy round into a false finding — or into an artifact edited to fit a check. Either way a round is spent and the wrong lesson is learned: that the review is noise, when the criterion was.

## When to Apply

- Authoring or revising a plan's acceptance criteria, a task's `Evidence Required`, or a dispatch Assignment's `Acceptance Criteria`.
- Writing any check whose expected result is a count, a "none anywhere" property, or a whole-corpus state.
- Triaging a review round: before routing a fix to an implementer, ask whether the finding survives re-reading the criterion exactly as the reviewer had to read it.

## Examples

### Site checks that do not count (verified in CI pin work)

```sh
grep -n "npm-dsh-cli-0.2.1-rc.1" .github/workflows/ci.yml                                          # cache key
grep -n "npm install -g @deepseek-ai/dsh@0.2.1-rc.1" .github/workflows/ci.yml                     # install step
grep -n "not found on PATH after npm install -g @deepseek-ai/dsh@0.2.1-rc.1" .github/workflows/ci.yml  # fail-loud echo
grep -n "0.1.7-rc.1" .github/workflows/ci.yml                                                     # expect no hits (exit 1)
```

Each site is named by its own text, so the verified tag may appear in a comment without affecting any check.

### A STOP rule with both branches

- A re-verification row fails **and** the failure touches a method string, param key, result shape, name literal, or ordering the crate emits, reads, or depends on → stop; a plan write-back decides.
- A row fails only because a documented enumeration gained a member the crate never touches → record it as a delta and route it to the documentation task.
