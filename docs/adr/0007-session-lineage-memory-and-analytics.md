# ADR 0007: Durable session lineage, immutable worker loadouts, advisory memory, and analytics

- **Status:** Accepted
- **Date:** 2026-08-31

## Context

Command Governor's first architecture correctly minimized what the authoritative
control plane must persist in order to survive crashes without turning SQLite into
a transcript database. That safety boundary was introduced to prevent a raw Claude
provider stream, hook payload, terminal transcript, or arbitrary tool traffic from
becoming durable orchestration state.

That boundary is **not** a product decision that Command Governor is a
"privacy-first" product and it must not be misread as a prohibition on useful
analytics or long-horizon memory.

The Phase-1 schema already makes sessions and session incarnations durable, but it
does not yet model several properties needed before live worker adapters harden:

- parent/child session lineage;
- the exact capability/configuration profile under which a worker was launched;
- a fail-closed resume contract when that profile changes or cannot be recovered;
- recursive delegation policy for subagents;
- non-authoritative observational memory for long-horizon orientation;
- phase-aware cost/token/latency analytics across workers and memory jobs;
- an explicit data-retention model for optional prompt/transcript analytics.

Public Pi configurations reviewed on 2026-08-31 provide useful implementation
patterns. `pi-interactive-subagents` persists a name-to-session registry and a
resolved loadout snapshot so a resumed subagent keeps its original model, tool
allowlist, identity/configuration, spawn whitelist, and working setup rather than
silently returning with a broader default environment. It also distinguishes
standalone, lineage-only, and forked child sessions and parks children that require
input instead of treating them as complete.

`pi-observational-memory` uses parallel observer workers over fixed chunks,
watermarks their coverage, deterministically renders the active observation pool,
and consolidates older observations in the background into bounded long-term
memory. Its observer and consolidator work is separately costed.

Stanford's 2026 systems characterization of ten agent-memory systems argues that
memory must be selected and operated as a system-level workload rather than judged
only on recall accuracy. It recommends full-lifecycle cost accounting, background
construction with admission control, reuse/batching where possible, capability
floors for construction models, workload-aware construction/query tradeoffs,
freshness/latency feasibility checks, adaptive consolidation, bounded growth, and
worst-case latency controls.

Stanford's MemoryArena separately shows why recall-only testing is insufficient:
multi-session agents may remember facts yet still fail to use those memories to
make the correct later action in interdependent tasks.

Finally, 2026 compaction-safety research demonstrates that uniform generative
compaction can rapidly erase exact safety constraints. Command Governor therefore
must never depend on model compaction for authoritative lifecycle state,
capability restrictions, user-owned decisions, or safety policy.

Primary research is recorded in
[`../research/2026-08-31-session-memory-and-analytics-review.md`](../research/2026-08-31-session-memory-and-analytics-review.md).

## Decision

### 1. Command Governor is correctness-first, not transcript-minimization-first

The durable control plane remains deliberately narrow because correctness and
security improve when provider payloads cannot accidentally become lifecycle
truth. However, useful analytics are a first-class product capability.

Command Governor classifies persistent data into four classes instead of applying
one blanket "no content" rule:

1. **Authoritative control state** — mandatory lifecycle identities, events,
   obligations, fences, generations, delivery/claim state, capability-profile
   identity, lineage, and safe operational measurements. This is always on and is
   the sole lifecycle authority.
2. **Required review/configuration artifacts** — bounded final worker results and
   immutable managed configuration/loadout artifacts required to reproduce or
   review a managed run. These are private, integrity-checked, and governed by
   explicit retention rules.
3. **Operational analytics** — token usage, cost, model/provider, cache usage,
   durations, queue/wait time, retries, error/outcome classes, session/subagent
   relationships, memory construction/retrieval/generation phase costs,
   compaction/consolidation counts, and bounded tool-category/count telemetry.
   These are local and **enabled by default**. They are not lifecycle authority.
4. **Content analytics** — raw or near-raw prompts, transcript/session rendering,
   full-text search, prompt-pattern mining, or other content-derived records that
   can reproduce source code, credentials, customer data, or arbitrary pasted
   text. These use a separate local content store and an explicit retention mode.
   They never enter the authoritative event ledger merely for analytics.

The Phase-1 prohibition on raw provider streams, generic hook payloads, browser
secrets, credentials, and arbitrary tool input/output in the authoritative store
remains. This ADR does **not** weaken those guarantees.

### 2. Operational analytics are default-on

Users should not need to sacrifice session/cost visibility to use the product.
Command Governor will collect bounded safe operational analytics by default,
including at minimum:

- session, incarnation, turn, parent/child, and role identities;
- worker/runtime/provider/model profile identity;
- input/output/cache-read/cache-write token counts when the provider exposes them;
- actual or provider-reported cost when available, otherwise explicitly marked
  estimates;
- wall-clock and queue/wait durations;
- completion/failure/reconciliation/interrupt outcome classes;
- subagent, observer, consolidator, reviewer, and foreman resource usage;
- context-window/compaction/consolidation events and before/after token counts;
- tool activity as bounded categories/counts when available without persisting raw
  tool arguments/results;
- memory construction, retrieval, and generation costs as separate phases.

Metrics must carry provenance indicating whether a value is provider-reported,
measured locally, derived, or estimated. Cost never rolls backward when a provider
session is forked or a UI branch is changed; actual resource consumption is an
append-only accounting fact.

A future setting may disable nonessential metric collection/export, but metrics
required for correctness, reconciliation, security auditing, or truthful resource
accounting remain available to the local owner.

### 3. Content analytics are a separate retention capability

Prompt-pattern mining and session search are useful features and are not rejected.
They require a different storage contract because prompts and transcripts can
contain arbitrary sensitive material.

The content-analytics subsystem therefore:

- is physically/logically separate from the authoritative SQLite control ledger;
- is local-first and excluded from lifecycle decisions;
- has explicit retention policy (`off`, bounded retention, or retained-until-user
  deletion; exact product names may change);
- can be deleted/rebuilt without changing task/session/obligation correctness;
- never becomes a required source for ACK, delivery, capability authorization, or
  worker completion;
- must clearly distinguish raw retained content from derived indexes/features;
- must avoid silently exporting content to a remote analytics service;
- must support scoped deletion by session/project and eventually export/backup
  policy appropriate to the selected mode.

The initial public V1 may ship operational analytics before content analytics. That
sequencing is an implementation scope decision, not a rejection of the feature.

### 4. A logical worker session has immutable lineage and a resolved loadout

A session name, runtime pane, PID, or provider session string remains display or
external-reference data, never the identity fence.

Before a live worker is spawned, Command Governor durably creates the logical
session/turn/delegation records and binds them to a **resolved loadout**. The
loadout is immutable for that logical session revision and includes safe identities
or digests for at least:

```text
loadout_id
worker_kind
runtime_kind
role
model_policy_ref
capability_profile_ref
capability_profile_digest
delegation_policy_ref
managed_config_ref / managed_config_digest
hook_contract_epoch
resume_policy
created_event_seq
```

Exact managed configuration needed to reproduce a launch may live as an
owner-private immutable configuration artifact rather than generic SQLite text.
The authoritative store records its identity, digest, schema/contract epoch, and
safe metadata.

Resume is fail-closed:

- same validated loadout -> continuation may proceed under the normal worker
  delivery/ambiguity contract;
- missing/corrupt/unverifiable loadout -> reconciliation/input attention, never an
  unrestricted resume;
- requested capability/model/configuration change -> explicit new loadout
  revision/incarnation/turn transition according to adapter policy, never a
  silent mutation of the old sandbox.

### 5. Session lineage is a durable graph

Command Governor records parent/child relationships independently of the runtime's
pane/session tree.

A conceptual relation is:

```text
session_edges(
  parent_session_id,
  child_session_id,
  parent_turn_id,
  relation_kind,
  created_event_seq
)
```

Initial relation kinds should cover at least:

- `delegated_worker`
- `scout`
- `researcher`
- `reviewer`
- `observer`
- `consolidator`
- `provider_fork`

Provider/UI lineage may be richer, but these are Governor semantics rather than
copied provider labels.

The runtime may lose its process tree and still not erase this lineage.

### 6. Delegation is capability-whitelist based and recursive

Each worker role resolves to an explicit capability profile. A child receives only
the capabilities listed by that profile. A worker may spawn only roles allowed by
its delegation policy. Omitting a child role cannot mean "give it the default/full
profile."

For example, a read-only scout can inspect but not mutate; a researcher can use
approved research tools; a reviewer can read evidence without publishing; an
implementer may edit within its delegated scope; destructive, credential-sensitive,
or materially broader actions remain separately governed.

The loadout and delegation-policy identity survive resume. This is a correctness
and least-authority property, not merely UI configuration.

### 7. Child completion creates durable parent-facing work

Asynchronous subagents are useful, but a child's final message cannot silently
close the parent task.

The conceptual flow is:

```text
parent delegates
  -> durable child session + lineage + child obligation
  -> external spawn authorized only after durable intent
  -> child executes asynchronously
  -> confirmed final result + durable result artifact
  -> parent/foreman-facing result obligation
  -> explicit processing/disposition
```

A child that needs input parks in a durable input state and routes the request
according to policy (parent worker, foreman, or user-owned decision). Parent death,
runtime restart, or UI closure cannot strand or erase that request.

### 8. Observational memory is useful but never authoritative

Command Governor may provide an optional memory plane with three distinct layers:

```text
A. authoritative control state
   immutable events + projections + obligations + fences

B. deterministic control capsule
   bounded exact rendering of current control facts from A

C. observational memory
   model/algorithm-generated orientation, episodic/topic memory, and journey data
```

Layer A is never model-summarized or compacted away.

Layer B is deterministic and disposable: it can always be regenerated from Layer
A. It contains the exact open-work/control facts a worker or foreman needs after a
context reset without asking a model to remember them.

Layer C is advisory. Observer/consolidator output may improve orientation and
cross-session performance, but it cannot:

- close or ACK an obligation;
- authorize a capability;
- replace a current source/version/generation fence;
- reconstruct secret possession/correlation values;
- override a user-owned decision;
- convert silence or stale runtime state into terminal truth.

### 9. Observer/consolidator work is background, bounded, and watermarked

Memory workers run under explicit admission control and their own capability
profiles. Each observation records its source coverage, for example:

```text
memory_observation_id
session_id
source_event_seq_start
source_event_seq_end
observer_profile_ref
observer_revision
constructed_at
cost_record_ref
```

Out-of-order observer completion is acceptable when coverage ranges are explicit.
A memory view states the highest authoritative event sequence it covers, so a
consumer can distinguish "memory is current through N" from current control truth
at N+k.

Construction/consolidation must have bounded concurrency, wall-time/token/cost
budgets, and backpressure. It must not delay a latency-sensitive ACK, input answer,
or other control-path mutation solely to make advisory memory fresh.

Consolidation is triggered by measured marginal cost/size/backlog policy rather
than an assumption that one fixed interval is optimal for every memory strategy.

### 10. Compaction is type-aware; control facts are pinned

Uniform model summarization is forbidden for correctness-critical facts.

At minimum, Command Governor treats these as non-generatively-compacted/pinned:

- lifecycle/obligation state;
- identity/version/generation/source fences;
- capability/delegation policy;
- user-owned decisions and explicit requirements;
- accepted external-effect ambiguity state;
- current result/input artifact identities;
- exact safety/security policy needed to authorize an action.

Narrative orientation, redundant history, and advisory observations may be
summarized/consolidated according to memory policy. If an external provider's own
context compaction cannot guarantee preservation of these exact facts, Governor
re-injects the deterministic control capsule/loadout contract rather than trusting
the provider summary.

### 11. Memory is evaluated by downstream action, not recall alone

The acceptance suite must include MemoryArena-style dependent-session tests. A
memory system does not pass because it can answer "what happened earlier?" It must
preserve the information needed to perform the correct later action.

Required scenario families include:

- parent delegates -> child finishes -> daemon/runtime restart -> parent correctly
  processes the child result exactly once;
- worker compacts/restarts -> prior user constraint remains enforceable;
- observer is stale by several events -> deterministic control capsule prevents a
  stale memory from authorizing an old action;
- provider fork -> lineage is preserved but resource cost does not roll back;
- loadout definition changes after spawn -> resume still uses the original
  validated profile or fails closed;
- repeated compaction -> control/capability/safety facts remain exact;
- memory worker crashes or exceeds budget -> control plane remains correct and
  memory backlog is observable rather than blocking lifecycle progress.

## Data-model direction

This ADR intentionally does not freeze final SQL before the implementation issue
is reviewed, but the next migration is expected to require projections for:

- immutable worker loadouts / managed configuration artifacts;
- session lineage edges;
- delegation/capability profile identity;
- resource-usage/cost events and rollups;
- memory construction jobs/observations/watermarks;
- memory health/backlog conditions;
- content-analytics configuration/retention references without placing raw content
  in the authoritative event payload.

Events remain first and projections remain replayable.

## Security and product consequences

This ADR deliberately corrects an over-broad interpretation of the Phase-1 data
boundary:

- **Retained:** the rule that generic provider streams, credentials, browser
  secrets, raw hook bodies, and arbitrary tool payloads do not leak into
  authoritative control state.
- **Added:** useful local operational analytics by default.
- **Added:** explicit private configuration artifacts needed for faithful resume.
- **Allowed as a product feature:** local prompt/transcript/session analytics under
  a separate content-retention contract.
- **Rejected:** making raw content or model memory authoritative merely because it
  is convenient to query.

## Provenance

The mechanisms are independently implemented in Rust. No Pi TypeScript source is
vendored or ported line-for-line. Exact reviewed revisions and research links are
recorded in the associated research document and `THIRD_PARTY_NOTICES.md`.

## Consequences

### Positive

- resumptions cannot silently widen a worker's sandbox;
- subagent relationships survive runtime/process loss;
- parallel specialists become first-class rather than ad-hoc terminal panes;
- cost/session/subagent analytics become a core product surface;
- memory can improve long-horizon efficiency without becoming a second lifecycle
  authority;
- exact control facts survive provider compaction;
- future content analytics remain possible instead of being accidentally banned by
  a Phase-1 persistence rule.

### Costs

- another migration and replay surface are required before live worker integration;
- exact-loadout/configuration artifacts need retention, integrity, and migration
  rules;
- memory introduces background scheduling/cost/backlog complexity;
- content analytics need a separate store/retention/security design;
- recursive delegation requires capability-profile conformance tests.

## Alternatives considered

### Keep the Phase-1 "no prompts" rule as a blanket product policy

Rejected. The original rule protects the authoritative control plane from raw
provider traffic; it should not prevent useful optional local analytics or managed
configuration artifacts.

### Store every transcript/provider event in the authoritative SQLite database

Rejected. It expands the correctness/security surface, mixes arbitrary content
with lifecycle authority, and makes migrations/diagnostics/backup carry data they
do not need.

### Resume workers from the current role definition instead of the launch snapshot

Rejected. A changed role definition could silently grant new tools, delegation
rights, or model/configuration behavior to an old logical session.

### Let observer memory replace the event ledger after compaction

Rejected. Model memory is lossy, may be stale, and is not a fenced authority.

### Evaluate memory only with recall QA

Rejected by the multi-session action gap demonstrated by MemoryArena. Downstream
action correctness is the relevant measure for Command Governor.
