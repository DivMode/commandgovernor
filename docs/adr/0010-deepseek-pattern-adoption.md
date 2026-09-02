# ADR 0010: Adopt DeepSeek Harness patterns without weakening Prime durability

- **Status:** Proposed — first implementation slice present; independent review and stacked-PR CI required
- **Date:** 2026-09-01
- **Refines:** ADR 0009
- **Research:** [`../research/2026-09-01-deepseek-harness-architecture-donor-review.md`](../research/2026-09-01-deepseek-harness-architecture-donor-review.md)
- **Reviewed DeepSeek Harness revision:** `4e84901e6471b79ec0338099867ebb4606d12bb5`

## Context

ADR 0008 changed Command Governor from a standalone general-purpose Rust agent runtime into a Pi-family harness/distribution. ADR 0009 then selected Prime Agent as the initial runtime substrate, pending S0/S1 real-machine gates, because Prime already implements the hardest Governor semantics: detached resident workers, process-safe session leases, generation-aware recovery, durable mutation journals, explicit uncertainty and non-replay, child continuity and durable scheduling.

A subsequent review of DeepSeek Harness found a second architecture with unusually strong and complementary properties:

- an everything-is-a-plugin capability/service model;
- append-only typed Session events as a source of truth with derived projections;
- strict reconstruction and fail-closed unknown required events;
- provider-neutral subagents and explicit capability negotiation;
- a durable Session versus process-local Activation distinction;
- dynamic workflow programs for parallel/pipeline multi-agent orchestration;
- PTC/`run_code` with generated typed tool SDKs;
- a guarded tool pipeline with monotonic policy guards;
- first-class sandbox, credentials, approval, query, spill, telemetry and extension seams;
- generated API/invariant/doc-sync discipline;
- standard ACP v1 automation with persistent session list/resume/close;
- an experimental Agent Teams layer with a durable queued-minus-delivered peer mailbox and revisioned task DAG.

DeepSeek Harness also documents failure semantics Command Governor must not inherit. Its general continuable-subagent Activation, inbox and ownership graph are process-local. Inbox acceptance can occur before the message reaches the durable Session log, so a process crash can lose work the caller was already told was accepted. The core continuation design supplies no durable mailbox or cross-process lease. The reviewed schedule is session-local and has a crash duplicate window. The project is developer-preview software with expected compatibility-breaking changes and no security audit.

The correct architecture is therefore not “Prime versus DeepSeek.” It is to preserve Prime as the durability authority while adopting DeepSeek's strongest **structural patterns** as portable Governor layers and using DSH over ACP as a potential specialist worker.

## Decision

### 1. Prime remains the initial root durability substrate

ADR 0009's Prime selection is unchanged unless the existing S0/S1 gates or a later Governor Bench overturn it.

Command Governor will not replace the following Prime mechanisms with weaker DSH process-local equivalents:

- detached supervisor / resident root workers;
- process-safe session leases and single-writer fencing;
- worker generations and reconnect/replay recovery;
- command IDs and mutation journals;
- explicit uncertain-effect state;
- uncertain-effect non-replay;
- durable schedule claim/delivery semantics;
- descendant recovery.

A future DSH root-substrate proposal must pass the DeepSeek-specific torture gate in this ADR before it can replace those mechanisms.

### 2. DeepSeek Harness becomes a first-class architecture donor

Governor will actively adopt DSH patterns when they improve composition, reconstruction, context efficiency, policy or testability **without weakening Governor reliability semantics**.

Adoption order:

1. clean-room implement the architectural contract in a substrate-neutral Governor layer where useful;
2. reuse a portable standard such as ACP where practical;
3. compose a reviewed DSH component only when its runtime authority is bounded and pinned;
4. upstream generic improvements when appropriate;
5. never import DSH process-local state as Governor durability proof.

Popularity is not an admission criterion.

### 3. Governor uses explicit capability seams with one authority owner

The following classes of functionality should be modeled as named replaceable capabilities where practical:

- event/projection store;
- child-agent providers;
- workflow engine;
- sandbox;
- memory/context strategy;
- tool execution/policy;
- foreman transport;
- artifact/spill storage;
- telemetry/export.

Singleton authorities have exactly one active owner. Loading two lifecycle/policy owners does not use first-wins or last-wins semantics; it fails loud.

Registration/activation must have an explicit teardown path. Authority-bearing replacement is fenced to a new loadout/generation where required.

Initial implementation: `governor/composition/capabilities.ts`.

### 4. Governor adds a typed append-only event spine for Governor-owned facts

Governor-specific exact state is expressed as append-only typed facts with projections/read models.

```text
Governor durable facts
  -> task/lifecycle projection
  -> child-mailbox projection
  -> workflow projection
  -> foreman/review projection
  -> diagnostics/search projections
```

Rules:

- `append()` success means the implementation's durable commit boundary was crossed;
- event sequence is contiguous within its authority stream;
- unknown required event types fail reconstruction;
- only explicitly informational/ignorable events may be skipped;
- projections are derived state, not a second authority;
- model summaries/memory are not lifecycle authority;
- raw prompts/secrets/provider responses are not duplicated into the Governor ledger when a digest/artifact reference is sufficient.

Initial implementation: `governor/composition/events.ts`.

This is **not** a second general-purpose transcript/session runtime and does not replace Prime's session persistence.

### 5. Session identity and Activation identity are separate

Governor adopts DSH's useful distinction:

- `Session` = durable identity, lineage and loadout;
- `Activation` = one live process/generation residency epoch.

Governor strengthens it by carrying substrate generation/cursor fences. A process-local handle is never enough to authorize or prove durable work.

Initial implementation: `governor/composition/lifecycle.ts`.

### 6. Child delegation is provider-neutral and fail-loud

Governor exposes a child-provider contract rather than hard-coding one subagent runtime.

Potential providers include:

- Prime/RLM children;
- Prime sessions;
- DeepSeek Harness via ACP;
- OMP or other ACP agents;
- narrowly justified Claude Code/Codex transports.

Each provider advertises exact supported capabilities. A request requiring persona, tool restriction, depth, structured output, continuation or other semantics is rejected before start if unsupported. Silent degradation is forbidden.

Initial implementation: `governor/composition/child.ts`.

### 7. Child-message acceptance is durable-before-return

Command Governor explicitly rejects DSH core continuable semantics where inbox acceptance can precede durability.

Governor's definition is:

```text
accepted by Governor
  = durable Governor mailbox/admission record committed
```

not:

```text
accepted by Governor
  = an in-memory Agent inbox returned a MessageId
```

Transport delivery is a later state transition.

### 8. Generalize the DSH Agent Teams durable mailbox

The experimental DSH Agent Teams layer contains the correct recovery shape and is adopted as a design donor:

```text
queued durable message
  -> transport attempt
  -> target/provider durable receipt or exact reconstruction proof
  -> delivery-confirmed durable event
  -> item leaves pending mailbox
```

Message identity is stable and used for target/provider deduplication. `queued - delivery-confirmed` is the recovery set.

Governor adds Prime's cross-process lease/generation fencing around this mailbox. A transport response that is lost after a possibly-effectful send becomes ambiguous/uncertain and must be reconciled; it is not automatically replayed.

### 9. Programmatic workflows become a Governor orchestration layer

Governor adopts DSH's insight that multi-agent orchestration should not require an LLM round trip for every child start/result.

The initial public internal representation is a bounded declarative workflow IR:

- `delegate`;
- `sequence`;
- `parallel`;
- `pipeline`;
- `phase`.

Resource limits are validated before execution: maximum depth, total delegates, parallel width and node count.

Initial implementation: `governor/composition/workflow.ts`.

A model-written JavaScript/Python workflow executor is **not** made default by this ADR. It must pass sandbox, bounded-cancellation, quiescence and Governor Bench gates first.

### 10. Workflow engines must settle boundedly and quiescently

Borrowing DSH's workflow-run contract, any Governor workflow executor must:

- have exact run identity;
- fail loud on invalid orchestration semantics;
- distinguish orchestration failure from child failure;
- stop child admission during cancellation;
- await/verify descendant cleanup;
- settle within a configured bound or classify the result uncertain;
- emit observation snapshots, not live authority handles.

### 11. DSH PTC/`run_code` is benchmarked against Prime RLM

DSH PTC generates a typed SDK over mounted tools and lets one model program call them while keeping intermediate values out of conversational context. Prime RLM uses persistent programmatic state across calls and compaction.

Governor Bench will compare:

- native tool calls;
- fresh-run PTC/`run_code`;
- persistent Prime RLM;
- hybrid approaches.

Defaults are selected on correctness, token/cache behavior, latency, replayability and security — not theory or author benchmarks.

### 12. Tool policy is monotonic toward less authority

Governor adopts the useful DSH tool-pipeline property that authoritative guards cannot be undone by a later plugin.

A denial remains a denial. Widening authority requires a distinct explicit escalation/approval event rather than another listener returning `allow`.

Independent safe reads may be parallelized where the resource classifier proves it safe; mutating/external-effect calls remain serialized/fenced as required by their resource identities.

### 13. Sandbox is a replaceable capability with explicit boundary facts

DSH's sandbox seam and `full`/`partial` enforcement reporting are adopted, but Governor extends the vocabulary beyond DSH's filesystem-only mode.

Every execution profile can report separately:

- filesystem enforcement;
- network boundary;
- process boundary;
- credential boundary;
- backend identity.

A filesystem-only macOS Seatbelt result does not satisfy a network/credential isolation requirement.

Initial implementation: `governor/composition/sandbox.ts`.

ADR 0009 Gate S3 remains required; DSH itself warns that its sandbox is not a sole trust boundary.

### 14. Approval and credential patterns are adopted into the security plane

Governor should use DSH's one-shot approval/audit mechanics where user approval is required:

- exact approval request identity;
- durable requested/decided pair;
- only the exact one-shot grant authorizes the requested action;
- missing/throwing/unavailable answerer fails closed;
- no unlogged grant.

Credentials are references, not durable config values. Secret values are resolved at the operation boundary and are never returned by describe/list surfaces or stored in model memory/telemetry by default.

### 15. Autonomous goals and task mutations use revision/CAS semantics

DSH's goal and Agent-Team task domains validate Governor's existing task-revision rule.

Any Governor autonomous goal/shared task mutation must use exact revision identity so stale agents cannot close or rewrite newer work. Blocker graphs must remain acyclic and continuation rounds are bounded.

This does not create a second obligation authority beside Governor's task/review state machine.

### 16. Large output uses bounded previews plus exact artifacts

Adopt DSH spill/reference architecture for oversized worker/tool/review output:

- full evidence persists in a private/integrity-checked artifact;
- model/foreman gets a bounded preview and opaque reference/digest;
- locators are opaque, not trusted filesystem paths;
- source task/tool/call identity is recorded;
- artifact storage failure cannot be reported as successful preservation.

### 17. Query/search/index state is a projection

DSH's session-query and source-relationship designs are useful for Governor lineage/evidence search.

Search indexes, semantic memory and query cursors are disposable read models. They may accelerate discovery but cannot mutate task truth or become the only copy of lifecycle evidence.

Cross-session references are explicitly untrusted model context, not authority.

### 18. Compaction records provenance but remains lossy/advisory

Governor adopts DSH's compaction transaction bracket and source-sequence provenance:

```text
started -> generated replacement + source refs -> ended
```

An interrupted bracket is detectable. Exact policy, user-owned decisions, task revisions and external-effect state remain outside lossy compaction. Generated summaries may point to exact evidence but may not replace it as authority.

### 19. DSH schedule data semantics are donors; DSH session-local delivery is not

Governor may reuse DSH's strong time handling:

- explicit UTC/IANA zone input;
- canonical UTC persisted targets;
- typed durable recurrence records;
- strict replay and bounded catch-up.

Governor does not adopt DSH's reviewed session-local live-delivery contract or its admission-before-dispatch crash duplicate window. Prime/Governor durable scheduler semantics remain authoritative.

### 20. DeepSeek Harness is an explicit ACP specialist-worker candidate

The reviewed DSH ACP v1 server supports persistent session create/list/resume/close, prompt/update/cancel, permissions, MCP attachment, model/reasoning options and context usage.

Governor will add a DSH ACP conformance lane. ACP remains an interoperability protocol, not the internal durability authority.

### 21. Executable extensions use immutable version + run identity

Governor adopts DSH's distinction between stable plugin identity, immutable package version and exact live run identity. Stale runs cannot invoke a newer/different component.

Arbitrary model-generated plugins remain deferred until component admission and sandboxing are proven.

### 22. Every component documents authority and model/context cost

ADR 0009's component manifest is extended with DSH-style model-experience metadata:

```text
model-visible surface
token effect
KV/prefix-cache effect
```

These are required even when the answer is “none,” so default components can be measured and Governor Bench can detect prompt/cache regressions.

Initial implementation: `governor/composition/component.ts`.

### 23. Package/component invariants are explicit and mechanically testable

Authority-bearing Governor components must state and test the invariant they own. An empty invariant must be explicitly justified. Generated manifests/catalogs should mechanically check coverage and doc/source drift where practical.

### 24. Telemetry is an outbound redacted projection

Telemetry/export is not lifecycle authority. Redaction operates on detached outbound copies and never rewrites canonical task state. Telemetry failures do not change work success/failure. Secret/raw model content is excluded by default.

## Source-level adoption matrix

The detailed evidence and rationale live in the linked research document. Summary:

| DSH pattern | Governor decision |
| --- | --- |
| everything-is-a-plugin capability seams | **ADOPT NOW** |
| reversible effect/registration lifetime | **ADOPT NOW** |
| profiles/bundles/layered config | **ADAPT** |
| typed append-only event log + projections | **ADOPT NOW** |
| unknown required event fails reconstruction | **ADOPT NOW** |
| model-visible reconstructability | **ADAPT with privacy boundary** |
| persistence seam/flush/interruption repair | **ADAPT** |
| alpha no-migration posture | **REJECT** |
| Session vs Activation | **ADOPT NOW** |
| process-local inbox acceptance as durable success | **REJECT** |
| experimental Agent Teams durable mailbox | **ADOPT/GENERALIZE** |
| provider-neutral subagents | **ADOPT NOW** |
| fail-loud child capabilities | **ADOPT NOW** |
| model-written workflows | **ADOPT SHAPE; BENCHMARK CODE** |
| bounded workflow cancellation/quiescence | **ADOPT** |
| PTC / generated tool SDK | **BENCHMARK vs RLM** |
| monotonic tool guards | **ADAPT** |
| approval audit pair/fail closed | **ADAPT** |
| credential references/per-operation resolution | **ADOPT** |
| goal/task CAS revisions | **ADAPT** |
| process-local jobs as obligation authority | **REJECT** |
| job preflight/first-wins settlement | **ADAPT** |
| private output spill artifacts | **ADOPT/ADAPT** |
| session query/reference/lineage | **ADAPT as projection** |
| compaction brackets/source provenance | **ADAPT** |
| sandbox seam/full-vs-partial | **ADOPT NOW, EXTEND** |
| session-local schedule delivery | **REJECT as Governor scheduler** |
| ACP v1 automation | **ADOPT specialist-worker lane** |
| immutable extension package + run IDs | **ADAPT** |
| generated catalogs/runtime invariants | **ADOPT** |
| model/token/KV-cache impact docs | **ADOPT NOW** |
| telemetry redaction seam | **ADAPT** |
| webhook-created sessions | **DEFER** |

## Implementation slice in this ADR

This ADR is accompanied by substrate-neutral code rather than only prose:

```text
governor/composition/
  capabilities.ts
  child.ts
  component.ts
  events.ts
  lifecycle.ts
  sandbox.ts
  workflow.ts

conformance/tier1/deepseek-patterns.test.ts
```

The code intentionally does not implement a second supervisor/session runtime and does not replace PR #18's Prime D1/D2/D8 work.

## Additional bake-off gates

### Gate S1-D — DeepSeek root-substrate durability torture

A pinned DSH release must survive, at minimum:

- client/UI detach during work;
- DSH process death and restart;
- concurrent same-session open from two processes;
- shared-state writer races;
- accepted child message followed by crash before child-log append;
- recovery of queued-minus-delivered mailbox work;
- stale generation/reconnect state;
- external mutation admitted then connection/response lost;
- duplicate mutation/child-message/prompt delivery;
- schedule crashes before and after queue admission and durable dispatch;
- parent restart with descendants;
- torn-tail/checkpoint persistence failures;
- component/loadout generation changes during recovery;
- unknown required event/format reconstruction.

**Acceptance:** no accepted Governor obligation disappears; no same-session split brain; ambiguous external effects are never blindly replayed; duplicates are idempotent or quarantined; durable task/session/revision/generation identity remains exact.

Passing this gate is required before DSH can replace Prime as root substrate. It is not required for a narrowly scoped ACP specialist-worker adapter whose authority is bounded by Governor.

### Gate S2-D — DSH ACP specialist worker

Drive a pinned DSH ACP v1 server through a generic ACP client and verify:

- initialize/capability negotiation;
- session new/list/resume/close;
- prompt/stream/cancel;
- permissions;
- model/reasoning selection;
- MCP attachment;
- persistence across DSH restart;
- Governor task/revision/message correlation;
- duplicate request/reconnect behavior;
- clean teardown with no child/resource leak.

### Gate S4-D — PTC / RLM / workflow context bake-off

Compare native tools, DSH-style PTC, Prime RLM, bounded workflow IR and any hybrid using correctness, token/cache totals, latency, tool/retry count, crash recovery and security exposure.

### Gate S3-D — sandbox boundary truthfulness

For each candidate sandbox, independently prove filesystem, network, process and credential boundary claims. A backend reporting partial enforcement must fail a requirement for full enforcement.

## Relationship to ADR 0009

ADR 0009 remains the substrate decision. This ADR refines its surrounding architecture:

- Prime remains the initial root durability substrate;
- DeepSeek Harness is added beside OMP as a formal architecture donor and ACP worker candidate;
- Governor adopts explicit capability seams, a typed event/projection spine, durable child-mailbox semantics and bounded workflow IR;
- Governor Bench gains DSH-specific PTC/workflow/ACP/sandbox lanes;
- the component manifest gains model/token/cache impact metadata;
- a new S1-D gate is required before any future DSH substrate switch.

Where this ADR adds stricter rules than ADR 0009, the stricter rule governs the DeepSeek-derived component.

## Consequences

### Positive

- captures DSH's best architecture without abandoning Prime's strongest reliability mechanics;
- gives Governor a portable child-provider and workflow layer rather than hard-coding Prime everywhere;
- makes accepted child work durable before process-local dispatch;
- improves upgrade safety through fail-closed event reconstruction;
- provides a direct path to use DSH/OMP/other agents over ACP without substrate lock-in;
- creates an evidence-driven PTC-versus-RLM decision instead of guessing;
- makes sandbox boundaries explicit instead of treating a boolean “sandboxed” state as security;
- exposes plugin/context bloat through required token/KV-cache metadata;
- preserves clean source/license provenance through clean-room adaptation.

### Costs / risks

- more formal interfaces and tests before a component can be admitted;
- an event/projection layer can itself become a competing runtime if scope discipline is lost;
- child-provider portability may expose lowest-common-denominator pressure, so capability negotiation must remain fail-loud;
- arbitrary workflow/code execution remains a significant security surface and cannot be enabled casually;
- DSH is rapidly changing, so its ACP and donor concepts must be pinned and re-reviewed on upgrade;
- some DSH concepts duplicate existing Governor/Prime behavior and must be adapted rather than independently reimplemented.

## Alternatives considered

### Switch Command Governor from Prime to DeepSeek Harness now

Rejected. DSH has stronger composition architecture but does not yet prove Prime-equivalent cross-process session ownership, accepted-work crash recovery, mutation uncertainty/non-replay or detached scheduling. The project's own developer-preview and continuable-subagent documentation make those gaps visible.

### Ignore DSH because Prime already won the earlier bake-off

Rejected. DSH contains superior patterns for capability composition, event/projection discipline, provider-neutral children, programmatic workflows, PTC, sandbox contracts, approvals/credentials, invariant generation and context-cost documentation. Not adopting those ideas would recreate functionality less cleanly.

### Embed Cordis/DSH as a second in-process runtime beside Prime

Rejected as the default. That risks duplicate lifecycle/session/plugin authorities. Prefer clean-room Governor contracts and ACP boundaries; compose DSH itself only as a bounded worker or when a concrete component passes admission.

### Copy DSH source directly

Rejected for this slice. The architecture can be implemented more cleanly in Governor-specific contracts without creating source/version coupling. If later source code is ported, exact revision/license attribution is mandatory.
