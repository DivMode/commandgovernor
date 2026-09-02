# DeepSeek Harness architecture donor review — 2026-09-01

Status: **source-grounded architecture research; adoption slice implemented on `feat/deepseek-pattern-adoption`; runtime donor bake-off still required**.

Related decisions:

- [ADR 0008 — Adopt Pi as the Command Governor harness substrate](../adr/0008-adopt-pi-native-command-governor-harness.md)
- [ADR 0009 — Select Prime Agent as the initial substrate and ACP v1 as the public agent-client boundary](../adr/0009-prime-agent-substrate-and-acp-boundary.md)
- [ADR 0010 — Adopt DeepSeek Harness patterns without weakening Prime durability](../adr/0010-deepseek-pattern-adoption.md)

## Reviewed source

Repository: `deepseek-ai/deepseek-harness`

Exact revision reviewed:

```text
4e84901e6471b79ec0338099867ebb4606d12bb5
```

This was the observed `master` head during the review and the release commit for the `0.1.2-alpha.4` line. The repository is MIT licensed. DeepSeek explicitly labels Harness **developer preview**, says compatibility-breaking changes are expected, says it has not undergone a security audit, and says its sandbox must not be treated as the sole security boundary.

Primary source material reviewed at that revision:

- `README.md`, `SAFETY.md`
- `docs/architecture.md`
- `docs/subsystems/README.md`
- `docs/subsystems/session.md`
- `docs/subsystems/persistence.md`
- `docs/subsystems/subagent.md`
- `.agents/notes/implemented/feature/2026-07-28-continuable-subagent-conversations.md`
- `docs/subsystems/agent-team.md`
- `docs/subsystems/workflow.md`
- `packages/core/tools/README.md`
- `docs/subsystems/sandbox.md`
- `docs/subsystems/schedule.md`
- `packages/acp/acp/README.md`
- `docs/subsystems/compaction.md`
- `docs/subsystems/goal.md`
- `docs/subsystems/jobs.md`
- `docs/subsystems/approval.md`
- `docs/subsystems/credentials.md`
- `docs/subsystems/spill.md`
- `docs/subsystems/invariants.md`
- `docs/subsystems/session-query.md`
- `docs/subsystems/session-reference.md`
- `docs/subsystems/session-telemetry.md`
- `docs/subsystems/extensions.md`

The purpose of this review is not to copy DeepSeek Harness or switch substrates because of popularity. It is to identify architecture that Command Governor can clean-room adopt while preserving the stronger durability contract already selected from Prime Agent.

---

# Executive conclusion

**DeepSeek Harness should become a first-class Command Governor architecture donor and ACP-compatible specialist-worker candidate. It should not replace Prime Agent as the durability substrate today.**

DeepSeek Harness is exceptionally strong at:

1. **capability composition** — model, tools, session log, loop, persistence, sandbox, workflows, subagents, settings and transports are replaceable services/plugins rather than privileged core patches;
2. **event-sourced session truth** — append-only typed events, model history as a projection, explicit source relationships, strict reconstruction and fail-closed unknown required events;
3. **provider-neutral delegation** — one subagent service can host in-process, fork, ACP, Codex, Claude Code and DSH providers with explicit capability negotiation;
4. **programmatic orchestration** — workflow scripts and PTC/`run_code` reduce model/tool ping-pong and allow parallel composition outside conversational context;
5. **policy seams** — approvals, credentials, sandboxing, tools, settings and telemetry are explicit capabilities with narrow contracts;
6. **mechanical architecture discipline** — generated catalogs, package-owned invariants, typed IDs, exact revisions and explicit model/token/KV-cache impact documentation.

Prime Agent remains stronger at the failure semantics that are Command Governor's central product requirement:

- detached supervisor and resident root workers;
- process-safe session leases and single-writer fencing;
- generation-aware reconnect/replay;
- durable mutation journals;
- explicit uncertain-effect outcomes and non-replay;
- long-running work independent of an interactive client;
- durable scheduling semantics designed around crash boundaries.

The correct synthesis is therefore:

```text
Prime durability substrate
        │
        ├── Command Governor exact policy / obligation / event layer
        │       ├── typed append-only facts + projections
        │       ├── durable mailbox + task/revision correlation
        │       ├── capability ownership registry
        │       ├── workflow IR / orchestration
        │       └── sandbox + component admission contracts
        │
        ├── Prime/RLM workers
        ├── DeepSeek Harness over ACP as a specialist worker
        ├── OMP/other ACP workers where useful
        └── portable skills/tools
```

A DeepSeek pattern is adopted when it improves structure **without replacing a stronger Governor/Prime reliability invariant**.

---

# What Command Governor should take

## 1. Capability seams instead of core accretion — ADOPT NOW

DeepSeek Harness's most important architectural idea is not any individual tool. Cordis treats product behavior as plugins contributing services, typed events and reversible effects. Even the model adapter, tool registry, session log and agent loop are replaceable.

Governor should use the same *shape* without importing Cordis as a second runtime:

- define narrow service/capability contracts;
- name the authority each capability owns;
- permit one active owner for singleton authorities;
- fail loudly on overlapping lifecycle owners;
- registration returns an explicit disposer;
- consumers depend on the capability contract, not the concrete Prime/DSH/OMP provider.

Implemented now in `governor/composition/capabilities.ts`.

Governor deliberately adds a stricter rule than generic plugin composition: two plugins may not silently become competing authorities for workflow execution, lifecycle truth, memory, sandboxing or foreman transport.

## 2. Profiles, bundles and layered configuration — ADAPT

DSH boot is an ordered plugin tree built from profiles, bundles and patch layers. This is useful for Command Governor distributions and role loadouts.

Adopt the principles:

- reproducible base profile plus ordered overlays;
- explicit role/session loadout identity;
- inspectable resolved composition;
- package/version/hash provenance;
- no hidden mutation of a running work owner's authority.

Do **not** copy live hot-reload indiscriminately. DSH itself avoids live dependency replacement for one-shot/stdio applications once they own work. Governor should be at least as strict: changing an authority-bearing component requires generation/loadout fencing and a new activation/revision where needed.

## 3. Append-only typed event spine + projections — ADOPT NOW

DSH `Session` is an append-only typed `SessionEvent` log. Model history is derived from that log; replay is re-derivation. Its projection registry gives host consumers typed current state without inventing a second authority store.

Governor should adopt the pattern for **Governor-owned exact facts**, not copy the DSH transcript format:

```text
committed Governor facts
       │
       ├── lifecycle projection
       ├── pending mailbox projection
       ├── workflow projection
       ├── review/foreman projection
       └── observability projections
```

Implemented now in `governor/composition/events.ts`.

Important difference: model transcripts, summaries and memory are not the Governor's lifecycle authority. Governor event payloads should use bounded facts, digests and artifact references when raw prompt/tool content is unnecessary.

## 4. Unknown required event = refuse reconstruction — ADOPT NOW

DSH permits an unknown event to be skipped only when it is explicitly marked informational/ignorable. Unknown required events fail reconstruction instead of silently producing a gutted session.

Governor should use the same fail-closed upgrade rule. An upgrade that over-refuses is repairable; silently dropping an unknown policy/lifecycle event can corrupt authority.

Implemented and tested in `projectStoredEvents()`.

## 5. “Model-visible means logged” — ADAPT, with a privacy boundary

DSH asserts that anything entering a model request must be reconstructable from its session log, including request headers/tool schemas.

Governor should adopt the **reconstructability principle** at the worker/session-content layer, but should not duplicate raw model content into the Governor authority ledger just to satisfy it.

Rules:

- exact lifecycle/policy decisions must always be durably reconstructable;
- exact model-visible content belongs to the selected worker/session substrate or bounded artifact store;
- Governor authority events cite content by digest/reference when raw text is not needed;
- secrets, credentials and unnecessary raw provider output do not enter the Governor event spine.

## 6. Durable event sources and derivation links — ADAPT

DSH surface events can cite `sourceEventSeqs`, and query tooling can trace replacement and derivation chains. Compaction records cite the events they shadow.

Governor should adopt explicit provenance for generated/derived facts:

- result derived from exact child revision;
- review derived from exact implementation evidence;
- compaction/memory derived from exact source range;
- foreman action derived from exact task/revision/delivery.

A generated summary must never be the only path back to exact source evidence.

## 7. Persistence as a seam; flush/checkpoint semantics — ADAPT

DSH separates the logical event model from persistence. It batches writes, exposes explicit flush checkpoints, detects interrupted turns, preserves already-durable mid-turn history and has provider contract tests.

Governor should retain Prime as the primary runtime/session durability owner, but use these persistence design rules for Governor-specific sidecars/artifacts:

- append/commit has an explicit durability point;
- checkpoint/flush errors are surfaced as uncertainty rather than guessed success;
- interrupted brackets are recovered as interrupted, not falsely completed;
- persistence providers share one conformance suite;
- publication of newly prepared state is transactional.

## 8. Reject “no migration path” as a Governor format policy — REJECT

The reviewed DSH alpha currently refuses session-format versions it cannot read and explicitly ships no upgrader chain for older versions.

That is acceptable for developer preview. It is not a suitable long-term Governor policy.

Governor durable formats need one of:

- a tested migration chain;
- a stable compatibility reader; or
- an explicit export/re-import transition with preserved authoritative evidence.

We may fail closed on unknown formats, but we should not casually strand durable obligations across normal upgrades.

## 9. Durable Session vs process-local Activation — ADOPT NOW

DSH usefully distinguishes a persistent Session from a live Activation/residency epoch.

Governor adopts that vocabulary:

- **Session** = durable identity, lineage, loadout and evidence;
- **Activation** = one process/generation residency of that Session;
- process-local handles never become durable identity or authorization by accident.

Implemented in `governor/composition/lifecycle.ts`, strengthened with substrate generation/cursor fields.

## 10. Core continuable inbox durability gap — REJECT THE GAP

The DSH continuable-subagent design explicitly says:

- Activation state is process-local;
- inbox contents are process-local;
- the ownership graph is process-local;
- `startContinuable()` may return after inbox acceptance before the message reaches the durable Session log;
- a crash can therefore lose an accepted prompt/follow-up;
- the core design supplies no durable mailbox, cross-process lease or automatic replay of accepted-but-unlogged work.

Governor must **never** define process-local inbox acceptance as durable acceptance.

Implemented rule: `admitChildMessage()` returns only after a Governor durable event commit.

## 11. Generalize DSH Agent Teams' durable mailbox — ADOPT/ADAPT

The experimental DSH Agent Teams layer contains a stronger design than the core continuable path:

1. the Lead Session stores the complete queued peer message first;
2. the target acknowledges receipt only after its pending inbox item or user message is durable;
3. `queued - delivered` is the recovery mailbox;
4. a globally unique message id is retained on the target side as the deduplication key.

This is exactly the pattern Governor should generalize beyond “teams”:

```text
Governor durable message admission
        -> dispatch attempt
        -> target/provider durable receipt observed
        -> delivery confirmation
        -> mailbox item may close
```

Prime's session leases/generation fences still supply cross-process single-writer authority. The DSH Team mailbox does not replace them.

Initial commit-before-acceptance contract is implemented in `governor/composition/child.ts`; full queued-minus-confirmed recovery is a next implementation slice.

## 12. Provider-neutral subagent registry — ADOPT NOW

DSH places multiple providers behind one subagent contract: in-process spawn/fork, ACP, Codex, Claude Code and DSH SDK.

Governor should likewise expose a provider-neutral child contract whose durable identity/correlation semantics are Governor-owned while execution may be:

- Prime/RLM child;
- another Prime session;
- DeepSeek Harness ACP;
- OMP ACP;
- Claude Code/Codex adapter where justified;
- future ACP agent.

Implemented in `governor/composition/child.ts`.

## 13. Fail-loud capability negotiation — ADOPT NOW

DSH validates provider capabilities before starting a child. Unsupported persona/tool-filter/structured-output/depth requests fail with a typed error rather than being accepted and silently ignored.

Governor adopts this rule. Silent degradation is forbidden for authority-, safety- or correctness-relevant child requirements.

Implemented in `assertChildProviderSupports()`.

## 14. Direct-parent authority and lineage — ADAPT

DSH continuable messaging uses the exact live sender plus durable parent-session lineage; recorded sender metadata is provenance, not authorization.

Governor should retain the same distinction:

- durable IDs/provenance do not themselves grant runtime authority;
- live authority is fenced by the current session/incarnation/generation;
- stale handles cannot authorize work;
- task/revision identity accompanies every cross-agent message.

## 15. Durable delegation depth and loadout/preset — ADOPT

DSH persists `delegationDepth` and `agentPreset` because resume under a different capability set would change what replayed history can act upon.

This strongly validates Governor's existing loadout/lineage rules. Resume must not silently broaden authority under newer defaults.

## 16. Model-written workflows — ADOPT THE SHAPE NOW; BENCHMARK ARBITRARY CODE

DSH's workflow seam lets a model write a compact orchestration program instead of repeatedly round-tripping through the model for every child start/result. It supports agent calls, parallel/pipeline composition, phases, logs and a bounded live run.

Governor should adopt programmatic orchestration because it can reduce token/tool churn and make multi-agent topology explicit.

The first Governor implementation is deliberately a **bounded declarative IR**, not arbitrary JavaScript:

- `delegate`
- `sequence`
- `parallel`
- `pipeline`
- `phase`
- hard limits on node count, depth, total delegates and parallel width.

Implemented in `governor/composition/workflow.ts`.

A model-script executor can be added only after sandbox/resource/error semantics pass Governor Bench.

## 17. Workflow bounded cancellation and quiescence — ADOPT

DSH's `WorkflowRun` owns cancellation/disposal, has bounded cancellation settlement, and waits for child cleanup. Fatal orchestration misuse is re-thrown rather than converted into an innocuous child `null`.

Governor workflow engines must likewise:

- have a holder-owned run identity;
- stop accepting new children before teardown;
- settle/cancel within a defined bound;
- distinguish child failure from orchestration-contract failure;
- record uncertain termination when external effects cannot be proven;
- reach descendant quiescence before claiming cleanup complete.

## 18. Observer events carry snapshots, not live authority handles — ADOPT

DSH workflow observers receive cloned data snapshots instead of a live `WorkflowRun`; observer failure is contained and cannot mutate execution state.

Governor telemetry/UI/review observers should follow the same rule. Read-only observation must not accidentally grant cancel/mutate/lifecycle authority.

## 19. PTC / `run_code` + generated typed tool SDK — BENCHMARK AGAINST PRIME RLM

DSH's PTC mode replaces a large native tool-schema surface with one `run_code` transport plus a generated typed SDK. Inner SDK calls re-enter the normal guarded tool pipeline. Independent safe calls can overlap; mutating calls serialize. Intermediate values stay outside the conversation and only the program's selected output is returned.

This is a major context-efficiency candidate.

Prime's persistent RLM takes a different approach: long-lived programmatic state survives turns/compaction.

Governor Bench must compare:

```text
native tool calls
vs
DSH-style fresh-run PTC
vs
Prime persistent RLM
vs
hybrid PTC/RLM approaches
```

Measure correctness, fresh/cache/total tokens, latency, tool count, retries, persistence/replay semantics and attack surface. Do not choose from theoretical token savings.

## 20. Tool pipeline + monotonic guards — ADAPT

DSH has a useful fixed pipeline:

```text
pre-execute allow/deny/ask
 -> monotonic owner guards
 -> execution wrappers
 -> post-execute inspection/replacement
 -> finalized result
 -> observe-only result event
```

Governor should adopt the key property: **a later plugin may not widen a denial made by an authoritative safety/policy guard**. Policy composition must be monotonic toward less authority unless an explicit user-approved escalation creates a new request.

## 21. Parallel-read / serialized-mutation execution — ADAPT

DSH PTC's executor can parallelize independent safe calls while serializing mutating calls in submission order.

Governor should benchmark the same scheduler classification, with a stricter mutation definition tied to external-effect uncertainty and resource identity. Parallelism must not create same-resource split brain.

## 22. Approval audit pair + fail-closed unavailable answerer — ADOPT/ADAPT

DSH's approval seam has a fresh request id and a durable `asked`/`decided` audit pair. Only `allowed-once` grants the exact action. Missing/throwing/nonconforming answerers become `unavailable`, which callers deny.

Governor already reserves high-risk decisions for the user. Adopt these mechanics:

- one-shot action-scoped approval identity;
- durable request and decision correlation;
- no unlogged grant;
- unavailable/ambiguous answerer fails closed;
- no plugin registered later can bypass a deterministic `never` policy.

## 23. Credentials are references, not config values — ADOPT

DSH settings/config carry credential references; providers own values. Consumers resolve once per operation rather than caching secrets across requests. UI-safe describe APIs have no field capable of carrying a secret value.

Governor should adopt this strongly:

- component manifests/config contain references only;
- secret brokers resolve at the operation boundary;
- credentials stay outside durable event/model-memory stores;
- read/describe surfaces cannot return values;
- concurrent secret rotation uses serialized read-modify-write when needed.

## 24. Goal revision/CAS semantics — ADAPT

DSH goals use `{id, revision}` compare-and-set identity; every durable mutation advances the revision. Durable phase is separate from process-local continuation activation. Continuation rounds are capped and attributed to the exact revision.

Governor already has task/revision correlation. Adopt the same discipline for autonomous goals:

- exact revision required for mutation;
- stale revisions cannot close newer objectives;
- bounded continuation rounds;
- blocked reasons have stable machine codes plus human explanation;
- activation/liveness is not durable objective phase.

## 25. Background-job preflight and first-wins settlement — ADAPT

DSH's jobs runtime contains several useful rules:

- preflight authorization/controller/admission before producer `run()`;
- once `run()` returns, registration cannot later fail;
- completion is announced only after terminal state commits;
- terminal settlement is first-wins;
- IDs are predictable; authorization, not secrecy, is the boundary;
- jobs cannot start for an owner that has no controller capable of collecting/stopping them.

Governor should use these rules for background tool/process tasks, but persist any job that carries a Governor obligation rather than adopting DSH's process-local job registry as authority.

## 26. Durable task DAG / CAS task snapshots from Agent Teams — ADAPT

DSH Agent Teams persists complete task snapshots with monotonically increasing revision, blocker edges and retained tombstones. `blockedBy` must remain acyclic; write scopes are advisory overlap warnings, not locks.

Governor can reuse the concepts for multi-agent project planning:

- complete revisioned task snapshot;
- explicit blocker DAG;
- tombstones instead of identity reuse;
- advisory write scopes for collision warnings;
- real repository/worktree locks or optimistic edit guards remain separate.

Independent review obligations remain a distinct Governor state machine and cannot be marked complete by the implementer.

## 27. Spill oversized outputs to private artifacts — ADOPT/ADAPT

DSH has a spill capability that stores full oversized output privately and returns an opaque locator plus a bounded preview/retrieval hint.

Governor should use the same pattern for large tool, review and worker outputs:

- durable full artifact stored once;
- model/foreman sees a bounded preview + opaque reference/digest;
- session/task ownership scopes storage;
- suggested filenames are hints, never trusted paths;
- artifact storage failure must not falsely report that the full output was preserved.

This complements Governor's existing artifact/evidence architecture and reduces context pollution.

## 28. Cross-session query as a read model, not authority — ADAPT

DSH supplies live-preferred logical session querying, full-text search, exact event windows, lineage traces and source/replacement relationships. Queries operate over validated session state and use opaque cursors tied to the query generation.

Governor should provide equivalent **read models** for:

- task/session lineage;
- evidence/source relationships;
- bounded event windows;
- search across prior work;
- stale cursor detection.

Search/index state is disposable/rebuildable. It never becomes task/lifecycle authority.

## 29. Structured cross-session references with explicit untrusted context — ADAPT

DSH separates host mention syntax from core identity, uses session IDs as authority, and prepares bounded referenced-session context as untrusted model input.

Governor memory/session reuse should similarly carry:

- exact source session/task identity;
- bounded snapshot/reference;
- explicit “untrusted advisory context” classification;
- self-reference/count/budget checks;
- no inference of execution authority from a mention/reference.

## 30. Compaction transaction brackets + provenance — ADAPT

DSH compaction logs start/summary/end brackets and source/shadowed event references. An unmatched start is detectable interrupted work instead of a false completion.

Governor should adopt the transaction/provenance pattern for lossy context transforms:

```text
compaction-start
  -> source selection
  -> generated compact view
  -> verification / source refs
compaction-end
```

But raw provider output and generated summaries should not be copied into the Governor lifecycle authority when an artifact/digest reference is sufficient. Exact policy/safety/task facts remain pinned outside compaction.

## 31. Sandbox as a capability with reported enforcement — ADOPT NOW, EXTEND

DSH supplies a replaceable sandbox seam and reports `full` vs `partial` filesystem enforcement. Its local provider includes macOS Seatbelt, Linux bwrap/Landlock and a Windows restricted-token/ACL path. It explicitly forbids silent unconfined passthrough for a confined request.

Governor adopts that structure but extends the contract because DSH's `SandboxMode` governs filesystem effects only:

- filesystem enforcement;
- network boundary;
- process boundary;
- credential boundary;
- backend identity.

Implemented in `governor/composition/sandbox.ts`.

A Seatbelt filesystem sandbox must not satisfy a Governor requirement for network or credential isolation merely because its filesystem result says `full`.

## 32. Durable schedule record/time-zone handling — ADAPT; REJECT DSH DELIVERY GAP

DSH's schedule domain has good mechanics:

- durable typed records;
- explicit offset/IANA timezone boundary;
- canonical UTC persistence;
- fixed-rate recurrence and bounded catch-up;
- strict replay validation;
- persistence uncertainty reported instead of guessed.

But the reviewed delivery contract is session-local: the original Session must be live, no cold scheduler exists, and a crash after queue admission but before durable dispatch can duplicate a reminder.

Governor should borrow the record/time semantics but retain Prime's stronger detached scheduling/claim-before-delivery mechanics. Do not switch to DSH's best-effort at-least-once boundary.

## 33. ACP v1 as a real specialist-worker path — ADOPT

The current DSH ACP server is stronger than a minimal prompt bridge. It supports standard ACP v1 with persistent session create/list/resume/close, model/reasoning configuration, MCP attachment, prompt/cancel, semantic execution updates, permission requests and context usage. Multiple sessions can share one connection, and close waits for owned activity/descendants/persistence teardown.

That makes DeepSeek Harness a credible **Governor specialist worker over ACP** without making it the root durability authority.

Governor rule remains:

```text
ACP = portable interaction boundary
Governor/Prime = durability, obligation and ambiguity authority
```

## 34. Dynamic extensions: immutable package versions + run IDs — ADAPT/DEFER EXECUTION

DSH dynamic extensions distinguish stable plugin identity, immutable package version and exact active run identity. Stale client runs are rejected; new executable versions may require approval.

Governor should take the version/run fencing idea for executable skills/plugins. Arbitrary model-generated plugins remain deferred until sandbox and component-admission gates are strong enough.

## 35. Package-owned runtime invariants — ADOPT

DSH requires packages to own invariant companions and mechanically verifies registration/publication coverage. An invariant checks authoritative events/mutable relationships rather than vague service presence.

Governor should make every authority-bearing component answer:

- what invariant does this component own?
- what observable state proves it?
- what test/failure injection enforces it?

“No runtime invariant” must be an explicit justified result, not omission.

## 36. Generated catalogs and doc/source drift checks — ADOPT

DSH generates/validates capability/event/type catalogs from source and drift-checks documented type declarations.

Governor should generate or mechanically validate:

- component manifest;
- capability ownership map;
- event vocabulary/schema versions;
- ACP/additive metadata catalog;
- executable skill/plugin hashes;
- runtime conformance matrix.

Architecture docs should not quietly describe an API that source no longer implements.

## 37. Model Experience / Token effect / KV Cache effect — ADOPT NOW

DSH package documentation frequently states:

- what the model sees;
- token impact;
- KV-cache/prefix impact.

Governor's component admission manifest should require the same information, alongside authority/security facts, so “more plugins” does not silently become “more prompt bloat and worse cache behavior.”

Implemented in `governor/composition/component.ts` and its Tier-1 test.

## 38. Telemetry redaction seam + canonical-log separation — ADAPT

DSH session telemetry keeps canonical session state separate from outbound telemetry. A redaction waterfall transforms the outbound copy only; backend batching/retry/loss policy belongs to the telemetry backend. Receiver deduplication uses stable event identity where available.

Governor should use this pattern:

- telemetry is never authority;
- redact/export a detached copy;
- telemetry failure does not mutate canonical task state;
- disclosure says what is shared;
- exact event IDs allow dedupe;
- no secret/raw model content exported by default.

## 39. Attachments and resource locators — ADAPT

DSH uses durable attachment identity and verified reads rather than passing large inline payloads indefinitely. Together with spill storage, this supports bounded context and reproducibility.

Governor evidence should prefer content-addressed or integrity-checked artifact references for large logs/images/results.

## 40. Settings layering and credential-safe configuration — ADAPT

DSH settings distinguish defaults, composition base and user document layers, with owner scopes and hot commits.

Governor should maintain explicit precedence and a resolved-composition view, but authority-bearing changes must be fenced to a new activation/loadout generation rather than silently mutate running work.

## 41. Plan/todo state as durable projections — ADAPT ONLY WHERE NON-DUPLICATIVE

DSH plans, todos and permission presets are event-backed projected state. Governor can reuse the pattern for user-visible work planning, but must not create parallel task/obligation authorities beside the Governor task state machine.

## 42. Webhook-created sessions — DEFER

DSH supports authenticated webhook delivery and Workspace Session creation. This is useful later for GitHub/CI/event-driven worker starts, but external event intake expands the security/identity surface. Defer until Governor's component admission, authentication and durable deduplication rules are complete.

---

# What Command Governor must NOT inherit

## Process-local acceptance as durable success

Rejected. Core DSH continuable inbox acceptance can outrun persistence.

## Process-local ownership as single-writer proof

Rejected. A process-local Activation/ownership graph is not a cross-process lease. Governor retains Prime session leases and generation fences.

## Blind replay of an ambiguous external effect

Rejected. Prime's explicit uncertain mutation state/non-replay remains mandatory.

## Session-local schedule delivery as the Governor scheduler

Rejected. A live UI/Session cannot be a requirement for a durable obligation.

## “Full sandbox” interpreted as full system isolation

Rejected. DSH's filesystem enforcement does not imply network/process/credential isolation.

## Developer-preview compatibility posture for Governor durable truth

Rejected. DSH explicitly expects breaking changes. Governor pins dependencies and requires migrations/conformance before a normal upgrade can strand obligations.

## Dynamic plugin accumulation without authority admission

Rejected. Every component declares authority, executable capabilities, provenance, model/token/cache impact and security boundary before becoming default.

---

# Revised source-level substrate score

This uses the same Command-Governor-specific weights as the existing harness landscape review. Scores are architecture judgments, **not benchmark results**.

| Candidate | Durability | Extensible | Sessions | Subagents | Context/RLM | Coding tools | Protocol | Security | Maintenance | Exit/ecosystem | Weighted |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Prime Agent | 5.0 | 4.0 | 4.5 | 5.0 | 5.0 | 3.0 | 5.0 | 2.0 | 4.0 | 3.5 | **85.7** |
| DeepSeek Harness | 2.5 | 5.0 | 4.5 | 5.0 | 4.0 | 4.0 | 4.5 | 3.5 | 2.5 | 4.0 | **77.5** |
| Oh My Pi | 2.5 | 4.0 | 5.0 | 4.5 | 4.0 | 5.0 | 5.0 | 3.5 | 3.0 | 3.0 | **77.2** |
| upstream Pi | 2.0 | 5.0 | 5.0 | 2.0 | 4.0 | 2.5 | 2.5 | 2.5 | 5.0 | 5.0 | **65.4** |

Why DSH does not beat Prime despite its stronger composition architecture: Governor gives durability/crash recovery the largest weight. DSH's event persistence is sophisticated, and the experimental Team mailbox is promising, but the general continuable path and session ownership model still do not establish Prime-equivalent detached-worker leases, mutation journals and uncertain-effect recovery.

---

# DeepSeek-specific Governor torture lane

Before DSH could replace Prime as root substrate, a pinned release must pass all of these against real processes and state roots:

1. start long-running work, kill/detach the interactive client;
2. kill the DSH process during a turn;
3. race two processes opening/mutating the same persisted session;
4. race shared-state writers and prove one durable authority;
5. accept a continuable-child message, crash before it enters the child log, then recover it exactly once;
6. crash after durable Governor mailbox admission but before transport submission;
7. lose the transport response after an external mutation may have executed;
8. reconnect from stale process/generation/cursor state;
9. crash a schedule before queue admission, after queue admission, before durable dispatch, and after durable dispatch;
10. restart a parent while descendants continue;
11. interrupt/torn-tail persistence at each write/checkpoint boundary;
12. resend duplicate child messages, commands, ACP prompts and foreman replies;
13. change component/loadout generation during recovery and prove stale authority cannot resume under widened defaults;
14. verify an unknown required event/format refuses reconstruction rather than silently degrading.

Acceptance:

- no accepted Governor obligation disappears;
- no same-session split brain occurs;
- no ambiguous external mutation is blindly replayed;
- duplicate delivery is idempotent or explicitly quarantined;
- recovery identity is exact across task/session/revision/generation;
- no process-local inbox/handle is treated as durable proof;
- completion is not announced before the terminal fact is durably committed.

Until DSH passes this lane, it remains an architecture donor and specialist worker, not the Governor root durability authority.

---

# Governor Bench additions

Add explicit benchmark lanes for:

### PTC vs RLM

- native tools;
- DSH-style fresh `run_code` + generated SDK;
- Prime persistent RLM;
- hybrid approaches.

Measure correctness, fresh/cache/total input, output/reasoning tokens, wall time, tool count, retries, parallelism, replayability and security exposure.

### Workflow engine

Compare conversational orchestration against bounded workflow IR and a sandboxed code-backed workflow engine. Include cancellation, child leak, resource-cap and malformed-workflow cases.

### DSH ACP specialist worker

Drive a pinned DSH ACP server from Governor and verify create/list/resume/close, prompt/update/cancel, permissions, MCP, persistence restart, task/revision correlation and child-result recovery.

### Sandbox

Compare macOS Seatbelt filesystem confinement, container/microVM alternatives and any Prime-compatible sandbox. Record filesystem, network, process and credential boundaries separately.

---

# Implemented in the first adoption slice

| Governor file | DeepSeek-derived pattern | Governor strengthening |
| --- | --- | --- |
| `governor/composition/events.ts` | append-only typed facts + projections | Governor facts are bounded authority events; unknown required facts fail closed |
| `governor/composition/capabilities.ts` | capability seams + reversible registration | duplicate authority owners are rejected |
| `governor/composition/lifecycle.ts` | Session vs Activation | activation carries explicit process generation/cursor and cannot substitute for durable identity |
| `governor/composition/child.ts` | provider-neutral children + capability checks | child acceptance requires durable Governor commit before return |
| `governor/composition/workflow.ts` | programmatic workflow orchestration | starts as bounded declarative IR rather than arbitrary model code |
| `governor/composition/sandbox.ts` | sandbox service + enforcement fact | separates filesystem/network/process/credential boundaries |
| `governor/composition/component.ts` | Model Experience/token/KV documentation | required component admission metadata tied to authority/executable classification |
| `conformance/tier1/deepseek-patterns.test.ts` | mechanical conformance | proves the above fail-closed contracts |

These modules do **not** recreate Prime's supervisor, leases, mutation journal, scheduler or worker recovery. They are portable Governor contracts above the substrate.

---

# Next implementation slices

1. **Durable mailbox projection** — add queued/dispatched/delivery-confirmed states and target/provider receipt/deduplication, generalizing DSH Agent Teams' mailbox beyond team-only messaging.
2. **Prime child-provider adapter** — bind the provider-neutral child contract to the already-pinned Prime substrate without replacing Prime child/session authority.
3. **DSH ACP adapter spike** — pin DSH separately as an optional test fixture/specialist worker; do not add it to the production root dependency graph yet.
4. **Workflow executor** — implement the bounded IR over the child provider interface, then benchmark a sandboxed code-backed executor.
5. **Component manifest integration** — merge `ComponentExperienceDescriptor` with ADR 0009's P0 source/hash/license/authority/security manifest.
6. **Sandbox S3** — select a real macOS isolation profile and prove network/credential boundaries rather than merely wrapping filesystem writes.
7. **Generated ownership/event catalogs** — derive docs/manifests from source so ADR/component descriptions cannot drift silently.
8. **Goal/task CAS** — reuse revision/CAS mechanics for autonomous continuation and shared task DAGs where they do not duplicate existing Governor obligation state.
9. **Artifact spill/reference lane** — bounded previews plus exact private evidence references for large worker/review/tool results.
10. **Telemetry redaction** — detached/redacted outbound projections with no authority or secret leakage.

---

# License and provenance

The reviewed DeepSeek Harness source is MIT licensed. This Command Governor slice uses **clean-room architectural adaptation**: the implementation files were written as Governor-specific contracts and do not copy DSH source text or its session/wire format. Source paths and the exact reviewed commit are recorded here so future reviewers can distinguish idea provenance from code provenance.

If a later change copies or ports source code rather than independently implementing an architectural pattern, that change must record the exact source file/revision/license and update third-party notices as required.
