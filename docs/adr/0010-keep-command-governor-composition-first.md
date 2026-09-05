# ADR 0010: Keep Command Governor composition-first on Prime/Pi

- **Status:** Accepted
- **Date:** 2026-09-02
- **Refines:** ADR 0008 and ADR 0009
- **Research:**
  - [`../research/2026-09-02-command-governor-composition-deduplication-audit.md`](../research/2026-09-02-command-governor-composition-deduplication-audit.md)
  - [`../research/2026-09-01-deepseek-harness-architecture-donor-review.md`](../research/2026-09-01-deepseek-harness-architecture-donor-review.md)

## Context

ADR 0008 made the decisive architecture pivot: Command Governor stopped being a standalone Rust-first agent runtime and became a Pi-family harness/distribution. It explicitly required a composition-first order: use Pi core, compose reviewed packages, contribute generic gaps upstream, add Command-Governor-specific extensions only for real product-specific gaps, and fork core only as a last resort.

ADR 0009 then selected Prime Agent as the initial substrate candidate because Prime already contains the durable long-running runtime machinery Command Governor would otherwise need to build: resident workers, session leases, recovery, mutation journals, schedules, goals, RLM children, agent messaging, continual refinement, and ACP.

Despite those decisions, merged PR #18 grew a large external Prime adaptation layer. The code received extensive correctness review, but the review answered the wrong first question: **"is this implementation correct?"** before establishing **"should this implementation exist at all in the selected architecture?"**

A later de-duplication audit showed that Prime and the current Pi/Prime package ecosystem already cover more of Command Governor's intended surface than the implementation plan assumed. Existing packages now provide strong task/evidence gates, independent review semantics, PR review, multi-agent delegation, ChatGPT transports, memory, and governance workflows.

Draft PR #19 repeated the same risk with DeepSeek Harness: it contained valuable donor research, but translated the donor into another generic `governor/composition/*` implementation before proving those custom components were required.

The problem is therefore not a missing reliability invariant. It is an **implementation-boundary failure**: a correct architecture decision can still be defeated if later implementation work treats every discovered edge case or attractive donor pattern as justification for another permanent Governor subsystem.

This ADR makes the implementation boundary explicit and durable and records what we actually want to take from DeepSeek Harness without cloning its runtime topology.

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

## DeepSeek Harness donor decision

The original donor review inspected `deepseek-ai/deepseek-harness` at `4e84901e6471b79ec0338099867ebb4606d12bb5` (`0.1.2-alpha.4`). This ADR refreshes the donor against current `master` on 2026-09-02:

```text
49a606bc5b5934603f22a26957a07dc799ab0291
0.1.2-alpha.5 line
```

DeepSeek Harness remains a **first-class architecture donor**, not the Command Governor substrate. Prime remains the selected durability substrate candidate. We take DeepSeek's best structural rules only when they improve the Prime/Pi composition without creating a second generic runtime.

### 6. Take DeepSeek's capability-seam discipline — ADOPT THE RULE

DeepSeek Harness uses Cordis so model adapters, tools, session services, agent loops, workflows, approvals, credentials, and other behavior are plugins/services rather than privileged core patches. Registrations are reversible effects and profiles/bundles assemble the product from ordered layers.

Command Governor should adopt the structural rule:

- depend on narrow capability contracts rather than concrete runtimes;
- keep one active authority for singleton concerns;
- make resolved composition inspectable and versioned;
- prefer reversible plugin/package composition over core patches;
- treat runtime/package provenance as part of the resolved loadout.

**Do not import Cordis or rebuild its service framework inside Governor.** Prime/Pi extension/package surfaces are the first implementation vehicle. DeepSeek supplies design evidence, not permission to introduce another plugin kernel.

Primary donor sources: DeepSeek `docs/architecture.md`, profiles/bundles documentation, and generated Cordis service catalogs.

### 7. Take event/provenance ideas only for genuinely Governor-owned facts — ADAPT

DeepSeek's Session is an append-only typed `SessionEvent` log and derives LLM history from it. Its persistence seam stores the same event vocabulary rather than maintaining a parallel stored-message type. Projections derive current state from the log; durable metadata such as cwd/lineage/version lives separately in the session header.

Useful rules:

- append-only facts beat mutable inferred state;
- derived/projection state must be reconstructible;
- generated facts should cite their source facts where relevant;
- unknown required persisted facts fail closed rather than silently disappearing;
- explicit flush/checkpoint points make durability/error boundaries observable;
- storage implementations should share one semantic contract.

But **Prime owns the generic worker/session transcript and persistence plane**. Command Governor must not create another generic event-sourced session log merely because the DeepSeek design is strong.

Apply the pattern only to a small set of exact Governor-owned product facts that cannot safely live in an existing Prime/Pi/package authority.

Primary donor sources: `docs/subsystems/session.md`, `docs/subsystems/persistence.md`, `packages/session/README.md`.

### 8. Take durable Session vs process-local Activation semantics — ADOPT THE CONCEPT

DeepSeek explicitly distinguishes a persistent Session from one process-local Activation/residency epoch for continuable subagents.

That distinction is useful for reasoning about restart-safe identity:

- durable session identity is not process identity;
- stale process-local handles do not authorize new mutations;
- lineage/provenance does not automatically grant live authority;
- restart may materialize a new Activation for the same durable Session.

Prime already owns its own generation/session/worker mechanics. Governor should reuse those mechanics and borrow the conceptual distinction where it improves policy/conformance vocabulary.

### 9. Take fail-loud provider capability negotiation — ADOPT

DeepSeek's subagent seam supports multiple providers behind one contract and checks provider capabilities before starting work. Unsupported persona/tool-filter/schema/depth features fail with typed errors rather than being accepted and silently ignored.

Command Governor should require the same behavior from selected Prime/Pi child/delegation paths:

- declare required capabilities before delegation;
- reject unsupported authority/safety/correctness requirements loudly;
- never silently downgrade a role's required tool restrictions, output contract, depth bound, or model/loadout requirement.

Do not implement another generic subagent registry if Prime or an existing package already supplies the needed provider-neutral delegation.

Primary donor source: `docs/subsystems/subagent.md`.

### 10. Steal the durable-mailbox invariant from DeepSeek Agent Teams, not the whole Team runtime — ADAPT

DeepSeek's experimental Agent Teams layer uses durable team/member/task/message identities and a persistent peer mailbox. The earlier donor review identified the strongest pattern as **persist-before-deliver and reconcile queued-minus-confirmed delivery**.

That is valuable wherever Command Governor has a true cross-agent/cross-process delivery obligation:

```text
durable admission
  -> dispatch attempt
  -> durable/observable target acceptance
  -> explicit delivery confirmation
  -> obligation closes
```

However, current DeepSeek continuable-subagent documentation also exposes an important boundary: `startContinuable()` can return after inbox acceptance before that message has necessarily reached durable Session persistence. That is exactly why we must **not** blindly treat DeepSeek core inbox acceptance as our durability definition.

Use the Team mailbox pattern as a conformance requirement when the selected Prime/Pi package path lacks equivalent durable admission/confirmation. Do not preemptively build a generic Governor mailbox.

Primary donor sources: `docs/subsystems/agent-team.md`, `docs/subsystems/subagent.md`.

### 11. Benchmark DeepSeek workflows; do not automatically build a Governor workflow engine — BENCHMARK / DONOR

DeepSeek's workflow seam runs a model-written orchestration script in a bounded worker-thread engine. Strong ideas include:

- metadata validated as plain data before script evaluation;
- engine-wide child-provider and total-agent ceilings the script cannot override;
- bounded cancellation and holder-owned disposal;
- child cleanup/quiescence before disposal completes;
- fatal orchestration-contract errors remain fatal instead of being converted into ordinary child failures;
- observer events receive cloned data snapshots rather than live mutation handles;
- durable presentation records describe starts/ends without becoming execution authority.

These are excellent orchestration design rules.

Command Governor should first compare them against Prime RLM, existing Pi workflow packages, and ordinary model/tool orchestration. Only a demonstrated product gap justifies a custom workflow engine.

Primary donor source: `docs/subsystems/workflow.md`.

### 12. Benchmark PTC / `run_code` against Prime RLM and native tools — BENCHMARK

DeepSeek's PTC mode replaces a large visible native-tool schema with a `run_code` transport plus a generated typed SDK. Sub-calls re-enter the normal tool pipeline, successful values stay typed/structured, and intermediate values can remain outside the model conversation.

Potential benefit: fewer model/tool round trips and less schema/context churn for tool-heavy work.

Command Governor should benchmark:

```text
native Prime/Pi tool calls
vs Prime persistent RLM
vs DeepSeek-style PTC/run_code
vs a hybrid
```

Measure correctness, fresh/cached/total tokens, latency, retries, tool count, persistence semantics, and attack surface. Do not select PTC from theoretical token savings alone.

Primary donor sources: DeepSeek PTC Agent Notes (`2026-06-15-ptc.md`, `2026-07-20-ptc-typed-tool-returns.md`) and `packages/core/tools`.

### 13. Take fail-closed one-shot approval semantics — ADAPT

DeepSeek's approval seam has a small closed outcome vocabulary. Only `allowed-once` grants the asked-about action; rejected/cancelled/unavailable fail closed. Missing or throwing answerers do not silently allow work. Approval audit pairs are durable in the requesting Session, while tool arguments are not duplicated into the approval record.

Command Governor should preserve these principles for any genuinely user-owned decision surface:

- approval binds to one exact action/revision;
- no answer is not approval;
- answerer failure is not approval;
- a later plugin cannot widen an authoritative denial;
- audit identity must correlate request and decision without duplicating sensitive payloads.

Use an existing Prime/Pi approval/permission primitive first when it satisfies the contract.

Primary donor source: `docs/subsystems/approval.md`.

### 14. Take credential references and per-operation resolution — ADAPT

DeepSeek keeps credential references in config and secret values in a separate provider. Consumers re-resolve credentials per operation, configuration surfaces can inspect configured/source/writable state without receiving the secret, and serialized read-modify-write protects rotating credential records.

Useful rules:

- configuration carries secret references, not secret values;
- read/status surfaces never return secrets;
- credentials are resolved at the operation boundary rather than copied into long-lived model/session state;
- writes that would be shadowed by a higher-priority source fail instead of pretending to succeed;
- token refresh/rotation uses serialized read-modify-write.

Apply through existing OS/Nix/Prime/Pi secret surfaces where possible. Do not create a Governor secret manager merely to mimic DeepSeek.

Primary donor source: `docs/subsystems/credentials.md`.

### 15. Take model-visible/cost/component discipline — ADAPT

DeepSeek's architecture repeatedly separates model-visible state from host-only state and documents which plugins affect prompts/tool schemas/session logs. Its generated service catalogs and component grouping make composition consequences inspectable.

Command Governor should preserve a lightweight form of this discipline for selected components:

- know whether a component changes model-visible prompt/tool context;
- know its provider/model/runtime authority;
- know its persistent stores and external I/O;
- measure token/cache/cost impact before making a component default.

This belongs in package selection/conformance metadata, not in another generic runtime registry unless one is actually required.

### 16. DeepSeek remains a specialist-worker/interoperability candidate, not the durability authority

DeepSeek Harness supports ACP and provider-neutral subagent backends and may be useful later as an ACP-compatible specialist worker. It does not replace Prime merely because its architecture is elegant or popular.

Prime remains preferred for the currently selected core durability semantics. A substrate change requires a future ADR and real failure/recovery evidence.

### 17. Existing package and transport capabilities are evaluated before bespoke replacements

The de-duplication audit identifies current candidates including `pi-squad`, `pi-tasks`, `pi-subagents`, `pi-pr-review`, `pi-governance-pipeline`, `pi-oracle`, `pi-gpt`, and `pi-observational-memory`.

These names are not permanently blessed dependencies. They are evidence that Command Governor must search the current ecosystem before recreating those capability classes.

For ChatGPT Web specifically, existing Pi-native transports are tested before any new Chrome/CDP/browser transport is built.

### 18. Generic upstream gaps should move upstream when practical

If a missing primitive is generic to Prime/Pi rather than unique to Command Governor, prefer an upstream fix or contribution. A local compatibility shim may bridge the gap only when the actual product path requires it.

A TEMP WORKAROUND must state:

- the exact upstream defect;
- the reproducer proving impact on Command Governor's actual path;
- why an existing public surface cannot avoid it;
- the minimum local delta;
- the condition under which the workaround is removed.

### 19. Sandboxing is optional hardening for the trusted-local product

Command Governor does not require sandboxing merely to operate in the trusted-local use case. Isolation remains useful and may be required for intentionally untrusted repositories, plugins, skills, or other workloads, but it is not a prerequisite for the core package topology.

This refines ADR 0009's broader sandbox language. Security hardening must not force a second runtime architecture where none is needed.

### 20. Repository state must make pivots unambiguous

When an accepted architecture change supersedes active implementation work:

- useful research/evidence is preserved on `main`;
- stale implementation PRs are closed or explicitly retargeted;
- stale roadmap language is corrected;
- README/ADRs/current research clearly identify the current direction;
- future sessions must not infer current direction from an old open branch or a previously green PR.

`main` plus accepted ADRs are the architecture authority. Open branches and old implementation plans are not.

Global Claude Code and Codex behavior/instructions are managed declaratively by `DivMode/nix-config`; this repository must not duplicate that global policy in repo-local `CLAUDE.md` or `AGENTS.md` files.

## Immediate implications

The next implementation work should minimize the repository toward the target package topology rather than extend the merged adaptation layer by default.

For the current repository:

- audit PR #18 production modules under USE EXISTING / PLUGIN / TEMP WORKAROUND / DELETE;
- build the smallest viable `@commandgovernor/harness` package path;
- run the D1/D2/D8 failure cases through that actual path;
- bake existing task/review/ChatGPT/memory/workflow candidates before implementing equivalents;
- include DeepSeek donor patterns in those bake-offs, especially capability seams, fail-loud negotiation, durable mailboxes, workflow bounds, PTC, approvals, and credential references;
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
- strong DeepSeek design patterns can improve Governor without importing another runtime;
- correctness review and architectural necessity become separate questions;
- expensive reviewed work can be salvaged without becoming permanent through sunk-cost reasoning;
- stale PRs and old roadmaps cannot silently become the next session's plan.

### Costs

- some previously reviewed custom code may be removed;
- package evaluation and upstream source inspection are required before adding generic capabilities;
- package churn means evidence occasionally needs refreshing;
- a narrow workaround may need to be maintained until an upstream defect is fixed;
- donor ideas require measurement and integration work instead of direct copying.

These costs are preferable to maintaining another general-purpose agent/control-plane stack beside Prime.

## Review invariant

For every meaningful production change, reviewers must answer this question before evaluating implementation quality:

> **Should this code exist in Command Governor at all?**

If the answer is not demonstrated from the current architecture and current capability evidence, the change is not ready to merge.
## Outcome (2026-09-04)

The existence test was applied to every component on `main` and to every
donor idea above, with the actual product path exercised through stock
Prime clients ([`../research/2026-09-04-zero-custom-code-proof.md`](../research/2026-09-04-zero-custom-code-proof.md)).
Result: no `PLUGIN` and no `TEMP WORKAROUND` survived; every merged custom
subsystem was `DELETE`; the package selection is `USE EXISTING` for task
evidence, delegation, GitHub review and the ChatGPT foreman transport
(`pi-gpt`, adopted on the user's explicit risk decision, ADR 0008 §8
amendment); independent acceptance is the foreman's correlated reply in its
ChatGPT thread, with GitHub merge after it; and the one concern with no
owner (tool gating) is an upstream gap that no Command Governor extension
could close. Command Governor ships zero custom production code.
This ADR remains the controlling rule for any future proposal to add some.
