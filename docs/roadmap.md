# Command Governor roadmap

This roadmap follows ADRs 0008–0010. It replaces the pre-pivot standalone Rust control-plane roadmap.

The success metric is **a smaller, usable Prime/Pi distribution with strong product-level conformance**, not the amount of custom infrastructure built.

## Stage 0 — repository and test shrink

Status: in progress.

- retire the frozen standalone Rust workspace/oracle from the active tree and CI;
- preserve useful historical invariants in research/Git history rather than merge-gating an obsolete runtime;
- reduce TypeScript conformance to product/pin/policy checks, real pinned-Prime reproducers, and the minimum tests for named temporary workarounds;
- classify every assigned authority as `USE EXISTING`, `PLUGIN`, or `TEMP WORKAROUND`;
- give every temporary workaround an explicit deletion condition;
- update current testing/security/roadmap documents so old topology cannot become accidental instructions.

Acceptance: one active product architecture and one active merge-gating conformance strategy.

## Stage 1 — smallest installable Command Governor package

Build the minimum real `@commandgovernor/harness` package using Prime's documented package/extension mechanism.

Keep it intentionally boring:

- package manifest/config;
- roles/skills/prompts that are genuinely product-specific;
- pin/authority policy;
- no parallel daemon/control plane;
- no new generic task/review/subagent/memory/transport subsystem.

Acceptance: install/load the package on the selected Prime pin and run a harmless session through the package path.

## Stage 2 — package bake-offs, one authority at a time

Evaluate credible existing packages separately under Prime before custom implementation.

Priority lanes:

1. independent candidate acceptance (`pi-squad` or current equivalent);
2. durable task/evidence contract (`pi-tasks` or current equivalent);
3. GitHub PR review (`pi-pr-review` or current equivalent);
4. generic delegation/reviewer roles (`pi-subagents`/Prime-native alternatives);
5. exact existing ChatGPT thread transport (`pi-oracle` or current equivalent);
6. direct/private ChatGPT alternative (`pi-gpt` or current equivalent, capability-gated);
7. optional observational memory only if downstream-action testing shows a real gap.

Do not install overlapping authorities in the same first bake-off. A package that loses is removed; its experimental tests do not become permanent merge gates.

## Stage 3 — re-run D1/D2/D8 through the actual package path

This stage determines whether merged PR #18 adaptation code survives.

### D1

Kill/recover a resident root through the package product path.

If Prime/package APIs preserve the same logical session, reject stale attachment state, and avoid duplicate roots without `governor/session/registry.ts` + custom reopen authority, delete those custom owners and their tests.

### D2

Run the exact worker-loss external-effect reproducer through the package product path.

Key question:

> Does the intended package/plugin path duplicate an external effect after worker loss without the external Governor mutation/replacement authority?

- **No:** delete the custom mutation ledger/classifier/durable-FS/process-identity machinery and their tests.
- **Yes:** retain only the minimum compatibility shim proven necessary by the reproducer, with the upstream defect/removal gate recorded.

### D8

Prove every Governor-created session on the package path has the required explicit persistent identity/path. Reduce/delete standalone path machinery if the higher-level API already satisfies it.

Acceptance: every PR #18 production family ends as `USE EXISTING`, `PLUGIN`, `TEMP WORKAROUND`, or `DELETE`, with custom production LOC reduced from the post-#18 baseline.

## Stage 4 — independent review workflow

Using the selected existing task/review packages where possible, prove:

```text
implementer result
  -> independent review state
  -> reviewer inspects actual evidence/source
  -> PASS / REVISE
  -> implementer cannot self-satisfy acceptance
```

Only missing Command-Governor-specific policy becomes a plugin.

Acceptance: restart-safe review state and no implementer self-approval.

## Stage 5 — exact ChatGPT foreman closed loop

Use an existing Pi-native exact-thread transport first.

Prove:

```text
current candidate/review revision
  -> correlated foreman request
  -> exact user-selected ChatGPT conversation
  -> durable response retrieval
  -> stale/wrong correlation rejected
  -> current work affected exactly once
```

Inject failure before/during/after send and response retrieval. Ambiguous submission is reconciled, not blindly repeated.

Only then decide whether a tiny Command Governor foreman policy/adapter remains necessary.

## Stage 6 — optional capabilities

Only after the core closed loop works:

- ACP interoperability if a real client path needs it;
- memory/continual refinement selected by downstream-action quality;
- tooling/UX donors such as OMP/DeepSeek-derived ideas when they win measured bake-offs;
- sandbox profiles for workloads intentionally treated as untrusted;
- cost/cache/latency accounting for the assembled package set.

No optional capability may introduce a second generic owner for an existing concern.

## Release criteria

A usable initial release requires:

- reproducible Prime/package installation and pin verification;
- one owner per concern;
- package-shaped Command Governor distribution;
- independent review semantics;
- exact foreman correlation on the selected transport;
- fail-closed handling of ambiguous external effects on the actual product path;
- no obsolete parallel runtime/control-plane implementation in the active tree;
- focused CI that proves the assembled product rather than historical machinery.

## Explicitly not on the roadmap

Unless new evidence changes ADR 0010, do not build:

- another general-purpose daemon/runtime around Prime;
- another generic session/transcript authority;
- another generic scheduler/goals/subagent/workflow engine;
- another generic memory engine;
- another browser automation stack before existing Pi-native transports fail the required conformance;
- a permanent custom mutation/session control plane merely because the old implementation was heavily reviewed;
- large speculative test catalogs for features not yet selected.
