# Session, memory, compaction, and analytics research review — 2026-08-31

Status: **architecture input for ADR 0007; no third-party implementation code copied**.

This review examines Pi configuration/session patterns and recent agent-memory
research for mechanisms that can improve Command Governor before its live
Claude/Herdr worker adapter and long-horizon session model harden.

The goal is not to adopt Pi as Command Governor's runtime. Pi is a productive
coding-agent environment; Command Governor is a Rust-first durable orchestration
control plane with stronger lifecycle/fencing requirements. The useful work is to
identify mechanisms worth independently implementing under Governor's invariants.

## Sources reviewed

### Pi repositories

| Project | Revision | License | Relevance |
| --- | --- | --- | --- |
| `amosblomqvist/pi-config` | `f82da563ab05d66729492d64c7ed4e96db3663f3` | repository/config reference | session analytics, role/tool extensions, integration map |
| `amosblomqvist/pi-interactive-subagents` | `c3e8b53c0754ae5ccc19fdab5a7481ec039bc2f7` | MIT | async children, persistent names, loadout snapshots, lineage, recursive tool policy |
| `amosblomqvist/pi-observational-memory` | `78a1efcfdd46332253fb289724f05b26dfc7769e` | MIT | observers, coverage watermarks, deterministic compaction, consolidation, cost accounting |
| `amosblomqvist/pi-dictate` | `3208b563e3adfd070ac7b256a09ba9fc7b869f50` | MIT | operator UX; not V1 control-plane priority |
| `amosblomqvist/learn` | `7cfd8942f82ab9476e63572387e1fe9bcea5082c` | repository reference | specialist researcher/visual-agent composition |

Primary URLs:

- <https://github.com/amosblomqvist/pi-config>
- <https://github.com/amosblomqvist/pi-interactive-subagents>
- <https://github.com/amosblomqvist/pi-observational-memory>
- <https://github.com/amosblomqvist/pi-dictate>
- <https://github.com/amosblomqvist/learn>

### Research

1. **Agent Memory: Characterization and System Implications of Stateful
   Long-Horizon Workloads** — Yasmine Omri et al., 2026-06-04,
   arXiv:2606.06448. The paper characterizes ten representative agent-memory
   systems with a phase-aware construction/retrieval/generation cost model and
   derives ten system recommendations covering construction scheduling,
   capability floors, amortization, freshness/latency, footprint growth, and
   worst-case latency.
   <https://arxiv.org/abs/2606.06448>
2. **MemoryArena: Benchmarking Agent Memory in Interdependent Multi-Session
   Agentic Tasks** — Zexue He et al., 2026-02-18, arXiv:2602.16313; also listed by
   Stanford Digital Economy Lab. It evaluates memory in tasks where earlier
   experience must drive later action and finds a large gap between traditional
   recall benchmarks and multi-session agentic performance.
   <https://arxiv.org/abs/2602.16313>
   <https://digitaleconomy.stanford.edu/publication/memoryarena-benchmarking-agent-memory-in-interdependent-multi-session-agentic-tasks/>
3. **The Compaction Cliff in Long-Running AI Agent Memory** — Saber Zerhoudi,
   Jelena Mitrovic, Michael Granitzer, 2026-08-24, arXiv:2608.22752. It reports
   rapid loss of exact safety rules under uniform agent compaction and proposes
   type-aware retention/compaction rather than treating all remembered content as
   equally compressible.
   <https://arxiv.org/abs/2608.22752>

Research papers are evidence for design/test direction, not normative
specifications. Command Governor's acceptance behavior remains defined by its ADRs
and executable tests.

## Pi-config: what is actually useful

The six-month Pi config explicitly calls out four larger extensions/systems:
interactive subagents, observational memory, dictation, and a learning system. It
also includes an `analyze-sessions` skill whose scripts inspect prior session JSONL
and provide:

- total and per-day cost;
- per-project cost;
- per-model cost;
- most-expensive-session ranking;
- subagent cost inclusion;
- session rendering;
- full-session search;
- user-prompt extraction and recurring-prompt-pattern mining;
- filtering by time/project/model/provider/error/minimum cost/message count.

### ADAPT

Command Governor should provide equivalent-or-better **operational** analytics in
its own schema/CLI rather than requiring users to parse provider session files:

- cost by session/project/model/provider/worker role;
- parent-vs-child/subagent cost;
- observer/consolidator/reviewer cost;
- token/cache usage where providers expose it;
- expensive/error-heavy session ranking;
- duration/queue/stall/retry/compaction metrics;
- machine-readable output for later UI/analysis.

This should be default-on local functionality. The existing Phase-1 privacy/data
boundary was intended to prevent arbitrary provider traffic from entering the
control ledger; it should not suppress useful numeric/typed operational metrics.

### ADAPT WITH A SEPARATE RETENTION CONTRACT

Pi's prompt/session full-text search and prompt-pattern mining are also useful.
However, implementing them by copying raw prompts/tool results into the
**authoritative** Governor event ledger would create a much larger security and
backup surface than lifecycle correctness requires.

Command Governor should keep this capability, but behind a separate local
content-analytics store with explicit retention/configuration and no lifecycle
authority. That design allows the full feature instead of rejecting it while
preserving a narrow correctness database.

### REJECT

- using provider transcript/session JSONL as Governor's authoritative history;
- treating a session file as proof of lifecycle state;
- making raw prompt retention necessary for ordinary cost/session reporting.

## Pi interactive subagents

### Persistent addressability

A child is assigned a logical/display name and the parent records a persistent
name-to-session mapping, allowing the same name to steer a live child or resume a
finished child after restart. Nested children use a registry associated with their
own parent session.

This solves a real operational problem: process/pane identity is temporary while
logical delegated work may outlive the process hosting it.

### Immutable resolved loadout

The strongest pattern is the persisted resolved loadout. At spawn time Pi records
what the child actually received, including its tool allowlist, model, thinking
level, system-prompt mode/identity, spawn whitelist, working/config setup, and
other role details. Resume replays that stored loadout rather than re-reading the
current agent definition.

The important security behavior is fail-closed: if the sidecar required for a
sandboxed resume is missing/unparseable, the safe behavior is refusal rather than
launching an old child with today's broader default tool environment.

### Session modes

Pi distinguishes:

- `standalone` — no lineage relationship;
- `lineage-only` — durable parent association without copying prior turns;
- `fork` — a child seeded with parent conversation history.

For Governor, **lineage** is the important concept. Governor should not copy raw
transcripts into its authoritative DB to emulate a fork. A provider-native fork can
be represented by a durable provenance edge while content remains in the worker
provider/session or optional content store.

### Recursive delegation policy

Pi grants tools with an allowlist and separately lists which child-agent roles a
subagent may spawn. There is no agentless path that silently grants the default
full toolset.

This maps naturally to Governor capability/delegation profiles.

### Child waiting/input

A child can ask its orchestrator a question and remain parked instead of exiting;
the parent can answer and continue the same child. A worker that still owns child
work or unanswered input does not auto-exit merely because its current turn ended.

This is adjacent to Governor's `needs_input` semantics and should become a durable,
routable parent/foreman/user input obligation rather than a pane-only feature.

### ADAPT

- logical child identity independent of runtime identity;
- durable parent/child lineage;
- resolved immutable loadout identity on spawn;
- exact-loadout resume/fail-closed mismatch behavior;
- explicit recursive delegation whitelist;
- asynchronous children with durable result/input obligations;
- role-specific scouts/researchers/reviewers/workers;
- status as observation (`starting/active/waiting/stalled`) rather than semantic
  completion truth.

### STRENGTHEN IN GOVERNOR

Pi's name registry and loadout sidecars are useful local mechanisms, but Governor
already has a single-writer event/SQLite authority and write-ahead external-effect
permits. The independent Rust implementation should therefore commit the logical
child/delegation/loadout **before** authorizing external spawn and fold all current
state from the durable event model.

### REJECT

- tmux as lifecycle authority;
- name-to-session files as a second authoritative database;
- provider/session transcript copying as a control-plane fork mechanism;
- child final text automatically closing parent work.

## Pi observational memory

### Pipeline

The reviewed design is approximately:

```text
fixed raw chunks
  -> parallel observer subprocesses
  -> atomic observations + coverage watermark
  -> active ledger/buffer
  -> deterministic compaction rendering
  -> oldest-overflow consolidator
  -> per-session topic files + bounded journey
```

Observers are independent enough to finish out of order because each records the
source range it covers. The consolidator is serialized, drains older observations,
and bounds the active pool. A short descriptive journey supplies orientation. The
extension also records the actual cost of observer/consolidator subprocesses.

### ADAPT

- source-coverage watermarks rather than pretending asynchronous memory is always
  current;
- bounded observer concurrency;
- background construction separated from latency-sensitive interaction;
- one-at-a-time consolidation where concurrent mutation would conflict;
- deterministic/model-free rendering at the final compaction boundary where
  possible;
- per-session memory scope plus explicit fork/lineage behavior;
- cost accounting for memory workers;
- bounded active memory with durable longer-term advisory storage.

### STRENGTHEN IN GOVERNOR

Governor has an authority that Pi memory does not: the immutable orchestration
event ledger. That enables a safer three-layer design:

1. authoritative control state;
2. deterministic control capsule generated from current authority;
3. advisory observational memory.

No observer summary needs to carry exact obligation truth. If memory is stale,
the deterministic capsule still provides current exact state.

### REJECT

- model-generated observation as an identity/version/ACK/capability authority;
- correctness depending on successful observer/consolidator execution;
- a provider's generative compaction replacing Governor's event ledger;
- unlimited memory-worker fanout or unbounded construction cost.

## Stanford agent-memory systems implications

The systems characterization is especially relevant because Governor is building
not merely an agent prompt but a long-running multi-session service.

Its recommendations lead to the following Governor design requirements:

1. **System-level evaluation:** memory selection must consider accuracy,
   construction cost, serving/retrieval latency, and storage footprint.
2. **Full-lifecycle accounting:** observer/consolidator construction cost is a
   first-class metric, not hidden overhead.
3. **Background admission control:** memory construction is a throughput workload;
   it must be rate-limited/batched/deferred under foreground pressure.
4. **Reuse/batching:** fixed source ranges, chunk caching, prefix reuse, and
   batching should be exploited where provider/runtime interfaces permit it.
5. **Capability floor:** a cheap observer model is not useful if it cannot satisfy
   the observation schema reliably; capability conformance must precede use.
6. **Workload-aware selection:** stable histories with many queries can justify
   heavier construction; continuous-ingestion/sparse-query workloads should not
   pay that cost automatically.
7. **Freshness-latency feasibility:** if construction+retrieval cannot fit between
   dependent sessions, the system must expose staleness or change scheduling;
   pretending memory is fresh is invalid.
8. **Adaptive consolidation:** consolidating/mutating memory should watch marginal
   cost/backlog and compact/rebuild when rewrites become expensive.
9. **Bound growth:** per-session memory footprint must have explicit retention,
   consolidation, archival, or forgetting policy.
10. **Tail-latency bounds:** LLM-driven background jobs require iteration/time/token
    ceilings rather than relying on average latency.

Governor can improve on the general freshness problem by separating memory from
control truth: stale advisory memory is permissible when every consumer also gets
an exact current control capsule.

## MemoryArena implications

MemoryArena's central lesson for Governor is that recall QA is not the acceptance
criterion. A memory system can retain facts and still fail to apply them correctly
in a later action.

Governor tests should therefore create explicit cross-session dependencies, for
example:

- session A discovers a constraint;
- session B acts later after restart/compaction;
- the test verifies the external decision/action, not a text answer claiming to
  remember the constraint.

Other required shapes include prior failed approaches, user choices, artifact
versions, child results, loadout/capability rules, and current obligation fences.

## Compaction-cliff implications

The compaction study reports that exact safety constraints degrade rapidly when
all context items are fed through the same generative summarization process. Its
main architectural lesson is type-aware retention: exact policy and ephemeral
narrative history have different fidelity requirements.

Governor already has the right foundation for a stronger answer:

- exact control/safety/capability facts remain structured authority;
- a deterministic control capsule rehydrates them after provider compaction;
- only advisory/narrative memory is generatively compacted;
- acceptance tests repeatedly compact and verify the **action boundary** still
  respects the exact rule.

This avoids making any one provider's `/compact` prompt a correctness primitive.

## Analytics policy correction

The original Phase-1 "forbidden persistence" rule should be read narrowly:

> do not make arbitrary provider/browser/tool payloads part of the authoritative
> lifecycle database just because it is convenient to spool them.

It should **not** be read as:

> Command Governor must avoid session analytics, cost analytics, memory analytics,
> or optional transcript/prompt analysis.

Recommended product split:

### Default operational analytics

Enabled locally by default:

- cost/tokens/cache by project/session/role/model/provider;
- subagent share and memory-worker share;
- duration/queue/stall/repair/retry metrics;
- result/input/failure counts;
- compaction/context-pressure metrics;
- memory construction/retrieval/generation phase attribution;
- expensive and error-heavy sessions;
- session lineage graph.

### Optional content analytics

Separate retention mode/store:

- prompt full-text search;
- session rendering;
- recurring-prompt-pattern mining;
- semantic indexing over retained conversations;
- user-requested long-term content memory.

The store can eventually support bounded retention, project/session deletion,
export/backup, and local encryption without placing arbitrary content in lifecycle
SQLite.

## Independent Rust implementation plan

Do not port the Pi TypeScript implementation. Implement the semantics against
Governor's existing pure core + single-writer store.

### Core types

Expected new concepts include:

```text
WorkerLoadoutId
CapabilityProfileId
DelegationPolicyId
MemoryObservationId
MemoryJobId
UsageRecordId
ContentRetentionPolicyId

SessionRelation =
  DelegatedWorker | Scout | Researcher | Reviewer |
  Observer | Consolidator | ProviderFork

UsagePhase =
  WorkerGeneration | MemoryConstruction | MemoryRetrieval |
  MemoryConsolidation | Foreman | Other
```

### Store/projections

Expected migration surfaces:

```text
worker_loadouts
session_edges
usage_records
memory_jobs
memory_observations
memory_health
content_analytics_config
```

Raw prompt/transcript content does not belong in those tables. A separate content
store is designed when that feature is implemented.

### Scheduler

Memory jobs use low/background priority with:

- concurrency limit;
- token/cost budget;
- wall-time/iteration timeout;
- backpressure;
- source-watermark dedupe;
- exact worker loadout/capability profile;
- no ability to close/mutate control obligations from observer output.

### CLI/inspection direction

Future surfaces should include equivalents of Pi's useful session analysis without
requiring transcript parsing, e.g.:

```text
command-governor sessions
command-governor sessions --tree
command-governor usage --by project|session|model|role|day
command-governor usage --include-subagents
command-governor memory status
command-governor memory cost
command-governor analytics status
```

Names are illustrative until the CLI contract is reviewed.

## Acceptance requirements

The implementation issue should include at least:

1. old logical session resumes only with the same validated loadout;
2. missing loadout/config artifact refuses resume rather than broadening tools;
3. parent/child lineage survives daemon + runtime restart;
4. child final result creates one durable processable obligation;
5. duplicate child completion creates no duplicate parent work;
6. child input remains open when parent/runtime disappears;
7. usage totals include children and memory workers exactly once;
8. provider/session fork never rolls real resource cost backward;
9. stale memory carries a visible source watermark;
10. stale observer output cannot override a newer control fact;
11. observer/consolidator crash cannot block control correctness;
12. budget exhaustion yields memory backlog/health, not fabricated memory success;
13. repeated compaction preserves exact capability/safety/control capsule facts;
14. dependent-session test verifies the later **action**, MemoryArena-style;
15. operational analytics work with content retention disabled;
16. deleting optional content analytics changes no lifecycle projection.

## Conclusion

Pi supplies several high-value mechanisms that complement rather than replace
Governor's existing architecture: immutable resolved worker loadouts, persistent
logical lineage, recursive allowlisted delegation, asynchronous child input/result
routing, and watermarked observational memory.

Recent memory research supplies the systems constraints needed to operate those
features responsibly: account for construction cost, keep it off the foreground
critical path, expose staleness, bound growth/tail latency, and evaluate downstream
action rather than recall alone.

The resulting Command Governor design is deliberately stronger than either source
in isolation: rich analytics and optional content memory are permitted, while the
fenced event/obligation authority remains exact and non-generatively-compacted.
