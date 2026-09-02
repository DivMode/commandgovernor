# ADR 0010: Keep Command Governor composition-first on Prime/Pi

- **Status:** Accepted
- **Date:** 2026-09-02
- **Refines:** ADR 0008 and ADR 0009
- **Research:** [`../research/2026-09-02-command-governor-composition-deduplication-audit.md`](../research/2026-09-02-command-governor-composition-deduplication-audit.md)

## Context

ADR 0008 made the decisive architecture pivot: Command Governor stopped being a standalone Rust-first agent runtime and became a Pi-family harness/distribution. It explicitly required a composition-first order: use Pi core, compose reviewed packages, contribute generic gaps upstream, add Command-Governor-specific extensions only for real product-specific gaps, and fork core only as a last resort.

ADR 0009 then selected Prime Agent as the initial substrate candidate because Prime already contains the durable long-running runtime machinery Command Governor would otherwise need to build: resident workers, session leases, recovery, mutation journals, schedules, goals, RLM children, agent messaging, and ACP.

Despite those decisions, merged PR #18 grew a large external Prime adaptation layer. The code received extensive correctness review, but the review answered the wrong first question: **"is this implementation correct?"** before establishing **"should this implementation exist at all in the selected architecture?"**

A later de-duplication audit showed that Prime and the current Pi/Prime package ecosystem already cover more of Command Governor's intended surface than the implementation plan assumed. Existing packages now provide strong task/evidence gates, independent review semantics, PR review, multi-agent delegation, ChatGPT transports, memory, and governance workflows. Draft PR #19 repeated the same risk by translating useful DeepSeek Harness donor patterns into another generic `governor/composition/*` layer before proving those custom components were required.

The problem is therefore not a missing reliability invariant. It is an **implementation-boundary failure**: a correct architecture decision can still be defeated if later implementation work treats every discovered edge case as justification for another permanent Governor subsystem.

This ADR makes the implementation boundary explicit and durable.

## Decision

### 1. Command Governor is a Prime/Pi package distribution, not a parallel control plane

The default product topology is:

```text
Prime Agent
  + selected reviewed Pi/Prime packages
  + @commandgovernor/harness
      - small Command-Governor-specific extensions
      - roles
      - skills
      - prompts/policy
      - configuration
      - focused conformance tests
      - temporary compatibility shims only when proven necessary
```

Command Governor differentiates through **policy, curation, integration, review semantics, and conformance**. It does not own a second general-purpose agent runtime or orchestration control plane beside Prime.

Prime owns the general runtime concerns it already implements, including worker/session lifecycle, resident workers, session persistence, recovery, leases, schedules, goals, RLM, agent messaging, compaction, and continual-harness refinement.

### 2. Every custom capability must pass an existence test before implementation or retention

Before new Command Governor production code is written, or existing custom code is retained during a refactor, the capability must be classified as exactly one of:

1. **USE EXISTING** — Prime, Pi, or a reviewed existing package already satisfies the required behavior.
2. **PLUGIN** — the behavior is genuinely Command-Governor-specific and belongs in the smallest practical Prime/Pi extension/package.
3. **TEMP WORKAROUND** — a demonstrated upstream defect affects the actual product path and cannot yet be fixed or safely avoided upstream. The workaround must have a removal condition.
4. **DELETE / DO NOT BUILD** — the custom implementation is redundant, speculative, or unnecessary.

There is no fifth category for "a custom Governor subsystem would be more rigorous." Stronger local engineering is not sufficient justification for duplicating an existing authority.

### 3. Necessity review precedes correctness review

Implementation and review order is mandatory:

```text
Does this capability need to exist in Command Governor?
        ↓ yes
What existing Prime/Pi/package capability was evaluated?
        ↓ genuine gap proven
What is the smallest owning surface?
        ↓
Only then review implementation correctness.
```

A large test suite, green CI, independent code review, or substantial sunk work does not establish architectural necessity.

A reviewer must reject or pause a change when the existence proof is missing, even if the implementation appears correct.

### 4. Architecture changes are recorded before implementation expands into them

When a proposed change would alter the ownership boundary established by ADRs 0008–0010, the architecture decision must be updated or superseded **before** implementation turns the new boundary into a large code surface.

This does not require an ADR for ordinary implementation details. It does require one for changes such as:

- introducing a new general-purpose runtime/service authority;
- replacing Prime as substrate;
- making a custom Governor store authoritative for a concern Prime currently owns;
- introducing a new mandatory transport/control-plane topology;
- adopting a second generic task/workflow/subagent/memory authority.

### 5. Merged code is not automatically permanent architecture

PR #18 is merged and remains valid evidence of real Prime edge cases, but its implementation is subject to the same four-way classification as new code.

In particular, the external D1/D2/D8 adaptation must be tested through the actual package/plugin product path. If the product no longer acts as an external raw daemon mutation/re-dispatch authority, custom mutation ledgers or recovery machinery that only protect that obsolete topology must be removed while preserving useful regression evidence.

Correct code may be deleted when the architecture no longer needs it.

### 6. DeepSeek Harness remains a donor, not permission to recreate its subsystems

The DeepSeek Harness research is retained as useful design evidence. Its capability seams, event/provenance patterns, workflow ideas, and provider-neutral composition can inform Command Governor.

But donor quality does not establish a missing product capability. Before implementing a DeepSeek-derived event spine, child runtime, mailbox, workflow engine, sandbox contract, or other generic subsystem, the same existence test applies against current Prime/Pi/packages.

### 7. Existing package and transport capabilities are evaluated before bespoke replacements

The de-duplication audit identifies current candidates including `pi-squad`, `pi-tasks`, `pi-subagents`, `pi-pr-review`, `pi-governance-pipeline`, `pi-oracle`, `pi-gpt`, and `pi-observational-memory`.

These names are not permanently blessed dependencies. They are evidence that Command Governor must search the current ecosystem before recreating those capability classes.

For ChatGPT Web specifically, existing Pi-native transports are tested before any new Chrome/CDP/browser transport is built.

### 8. Generic upstream gaps should move upstream when practical

If a missing primitive is generic to Prime/Pi rather than unique to Command Governor, prefer an upstream fix or contribution. A local compatibility shim may bridge the gap only when the actual product path requires it.

A TEMP WORKAROUND must state:

- the exact upstream defect;
- the reproducer proving impact on Command Governor's actual path;
- why an existing public surface cannot avoid it;
- the minimum local delta;
- the condition under which the workaround is removed.

### 9. Sandboxing is optional hardening for the trusted-local product

Command Governor does not require sandboxing merely to operate in the trusted-local use case. Isolation remains useful and may be required for intentionally untrusted repositories, plugins, skills, or other workloads, but it is not a prerequisite for the core package topology.

This refines ADR 0009's broader sandbox language. Security hardening must not force a second runtime architecture where none is needed.

### 10. Repository state must make pivots unambiguous

When an accepted architecture change supersedes active implementation work:

- useful research/evidence is preserved on `main`;
- stale implementation PRs are closed or explicitly retargeted;
- stale roadmap language is corrected;
- root agent instructions point to the current ADRs and research;
- future sessions must not infer current direction from an old open branch or a previously green PR.

`main` plus accepted ADRs are the architecture authority. Open branches and old implementation plans are not.

## Immediate implications

The next implementation work should minimize the repository toward the target package topology rather than extend the merged adaptation layer by default.

For the current repository:

- audit PR #18 production modules under USE EXISTING / PLUGIN / TEMP WORKAROUND / DELETE;
- build the smallest viable `@commandgovernor/harness` package path;
- run the D1/D2/D8 failure cases through that actual path;
- bake existing task/review/ChatGPT/memory candidates before implementing equivalents;
- retain focused conformance tests that protect real product invariants;
- remove tests whose only purpose is protecting a subsystem that is deleted;
- update ADR 0009 acceptance only from the final proven topology, not the superseded post-#18 S2/S3 roadmap.

## Relationship to earlier ADRs

### ADR 0008

Remains accepted and is strengthened. Its composition-first rule is controlling architecture.

### ADR 0009

Remains Proposed at the time this ADR is accepted. Prime remains the selected initial substrate candidate. This ADR refines how Command Governor is allowed to build on Prime and removes any implication that selecting Prime justifies a large external Governor control plane.

ADR 0009's statement that Command Governor must not recreate a competing worker/session control plane remains valid. Its sandbox language is refined by this ADR for the trusted-local product.

### ADRs 0001–0007

Their behavioral reliability requirements remain valuable where retained by ADR 0008. Their bespoke runtime/topology decisions remain superseded where they conflict with ADRs 0008–0010.

## Consequences

### Positive

- future sessions have a clear ownership boundary rather than reconstructing it from chat history;
- implementation size is constrained by demonstrated product need rather than by how many edge cases can be modeled locally;
- Prime/Pi ecosystem improvements can delete Command Governor code instead of creating permanent duplicate authorities;
- correctness review and architectural necessity become separate questions;
- expensive reviewed work can be salvaged without becoming permanent through sunk-cost reasoning;
- stale PRs and old roadmaps cannot silently become the next session's plan.

### Costs

- some previously reviewed custom code may be removed;
- package evaluation and upstream source inspection are required before adding generic capabilities;
- package churn means evidence occasionally needs refreshing;
- a narrow workaround may need to be maintained until an upstream defect is fixed.

These costs are preferable to maintaining another general-purpose agent/control-plane stack beside Prime.

## Review invariant

For every meaningful production change, reviewers must answer this question before evaluating implementation quality:

> **Should this code exist in Command Governor at all?**

If the answer is not demonstrated from the current architecture and current capability evidence, the change is not ready to merge.