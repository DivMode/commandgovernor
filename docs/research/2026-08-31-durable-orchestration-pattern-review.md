# Durable orchestration pattern review — 2026-08-31

Status: **implementation input for Phase 1; no third-party code copied**.

This review compares Command Governor's accepted invariants with three public
implementations that independently solve adjacent durability problems. The goal is
not to choose a framework. It is to identify concrete mechanisms that are already
battle-tested enough to influence our first Rust types, transactions, and crash
tests before those interfaces harden.

Pinned sources:

| Project | Revision | License | Primary relevance |
| --- | --- | --- | --- |
| `joseym/salvor` | `dd9eb49f6bf854dc1c96b1b1ad7accbc509807b0` | Apache-2.0 | pure Rust replay, effect classes, write-ahead intent, dangling-write reconciliation |
| `PrimeIntellect-ai/prime-agent` | `9f5edc192cfe3d4737205a2f551d2b6b6e34fe09` | MIT | daemon command journal, uncertain-result no-replay, leases/generations |
| `ralphkrauss/agent-orchestrator` | `8b2f3b967e90877c3abac07061dbb2b1e67d2035` | MIT | daemon-owned orchestration, durable notification/ACK, request IDs, short-lived reviewer turns |

## Executive conclusion

Command Governor should **not fork any of these projects wholesale**. Its unique
contract is the composition:

```text
confirmed worker result/input
  -> durable obligation
  -> at-most-once browser wake
  -> exact bound ChatGPT conversation
  -> MCP resume/claim
  -> independent foreman review/action
  -> explicit fenced semantic ACK
  -> obligation closure
```

No reviewed project implements that entire chain. However, all three independently
converge on the same key rule that Command Governor already adopted:

> Record enough intent before a consequential external effect to detect an
> uncertain crash window, and never turn uncertainty into an automatic replay.

That convergence is strong evidence for the architecture and should change how
Phase 1 is implemented.

## Salvor

### What the code actually does

Salvor's `salvor-replay` crate is deliberately IO-free. Its `ReplayCursor` consumes
an in-memory append-only event log and returns either a recorded outcome or a typed
live permit. A caller cannot execute an uncovered operation without first reaching
the live side of that state machine.

For tool calls the live path persists `ToolCallRequested` **before** execution and
then records `ToolCallCompleted` afterward. The event itself records an explicit
effect class. A completed event is replayed as data and the tool is never called
again.

If replay finds a `Write` intent without a completion, it returns
`ReplayError::NeedsReconciliation` carrying the recorded position, tool, input, and
idempotency key. The runtime refuses both silent retry and silent skip. The
repository includes a deterministic reconciliation fixture that fsyncs the real
write and then deliberately kills the process before completion is recorded,
proving resume does not duplicate the effect.

### ADAPT

- pure reducer/replay code with no clock, randomness, network, process, or database
  access;
- event schema as a durability contract rather than incidental serialization;
- explicit effect classification at the external-I/O boundary;
- intent persisted before consequential I/O;
- typed `reconciliation_required` when a non-idempotent effect has unknown fate;
- replay divergence fails loudly rather than silently switching to a different
  history;
- deterministic kill/failpoint fixtures targeted at the exact ambiguity window.

### REJECT

- using Salvor as Command Governor's orchestration runtime;
- persisting generic tool inputs/results/model traffic in the Governor ledger;
- assuming Salvor's run model can replace worker/foreman/browser/binding
  identities;
- copying code instead of independently implementing the selected semantics.

Command Governor's privacy contract is intentionally stricter: generic prompt,
tool-argument, command, cwd, transcript, and provider-stream material remains
forbidden durable control-plane data.

## Prime Agent

### What the code actually does

Prime Agent's supervisor identifies mutating daemon commands by `(clientId,
commandId)`. `CommandRecoveryJournal.begin()` appends and fsyncs a `received`
record before the mutation is dispatched. A result is appended before the response
is sent back to the client.

On retry:

- a completed identity returns the stored response;
- a received identity with no durable result returns
  `command_result_uncertain` and is **not replayed**;
- the client later sends `ack_result`, allowing the journal to compact old
  completed commands.

Its journal loader tolerates a final truncated append. Compaction writes a new file,
fsyncs it, renames it, and fsyncs the containing directory.

Prime Agent also protects session ownership with a canonical-path lease containing
a random token, PID, process-start identity, active session identity, and canonical
resource path. The process-start identity prevents a reused PID from impersonating
the old lease holder; release checks the random token before removing ownership.

### ADAPT

- stable mutation identity independent of transport reconnect;
- durable `received`/`completed` distinction before dispatch/response;
- repeat-completed => return recorded result;
- repeat-pending => uncertain, never redispatch;
- ACK used for retention/compaction rather than as evidence that an external
  mutation happened;
- generation/incarnation fences on supervisor-worker relationships;
- canonical resource identity plus lease token and process-incarnation evidence.

### REJECT

- the JSONL journal as a second source of truth: Command Governor already chose one
  authoritative SQLite writer;
- broad transcript/session persistence;
- treating runtime/session liveness as semantic completion truth;
- Prime Agent's product/runtime topology as the Governor domain model.

The important Prime Agent idea should become a **SQLite transaction protocol**, not
a port of the TypeScript class.

## Agent Orchestrator

### What the code actually does

Agent Orchestrator explicitly moved the orchestration state machine out of a
long-lived LLM supervisor and into the daemon. Orchestrator/reviewer/compactor
workers are short-lived turns that return structured decisions; the daemon owns
state transitions and action dispatch.

Its lifecycle event path persists a durable notification before emitting the
in-memory/advisory event. MCP documentation tells clients to reconcile advisory
push hints with durable `list_*_notifications` state and to ACK notifications only
after acting on them. Mutating entry points use stable `request_id` values so a
client retry returns the original object instead of duplicating work.

The reviewer path fails closed when a required reviewer is unavailable rather than
silently bypassing review.

### ADAPT

- daemon-owned orchestration truth; LLM turns propose/review but do not own state;
- short-lived structured reviewer turns rather than a forever-supervisor chat;
- durable attention record before ephemeral push/wake hints;
- `list/reconcile -> act -> ack` consumption pattern;
- stable request IDs for retryable mutations;
- read-only observation surfaces separated from state-changing decisions;
- fail-closed review gates.

### REJECT

- its broad generic workflow engine as a V1 requirement;
- per-run JSON-file persistence instead of the selected SQLite event/projection
  model;
- conflating notification ACK with Command Governor's stronger semantic foreman
  ACK;
- storing full prompts/results/action history in the Governor control ledger.

## The three ACK layers must remain separate

The external projects make it especially important not to overload `ACK`:

1. **Mutation command receipt ACK** — transport/client confirms it received a
   committed command result. This permits journal retention/compaction. It does
   not close engineering work.
2. **Attention/delivery ACK or claim** — a consumer has taken responsibility for a
   durable notification/obligation. It does not prove review is complete.
3. **Command Governor `foreman_ack`** — a semantic, fenced disposition after the
   bound foreman fetched the real artifact/input and performed the required
   independent review/action. This is the normal operation that closes the
   obligation.

Prime Agent mainly demonstrates layer 1. Agent Orchestrator demonstrates layer 2.
Command Governor must keep layer 3 stronger than either.

## Phase 1 Rust blueprint

### `governor-core`

Add typed primitives before adapters exist:

```text
ActorId
MutationCommandId
DaemonEpoch
ProcessIncarnation
ResourceIdentity
ResourceLeaseId
ExternalAttemptId
ExternalEffectClass = Read | IdempotentWrite { key } | NonIdempotentWrite
ExternalAttemptState = IntentRecorded | Completed | FailedBeforeEffect | Ambiguous
```

Provider-specific names do not enter these types.

Model consequential external effects as an intent/outcome pair. The pure reducer
may authorize an adapter to perform I/O only after the store has accepted the
intent and returned an execution permit/fence. A crash with intent but no proven
outcome projects `Ambiguous`/`reconciliation_required`; it never projects success.

For truly idempotent writes, policy may permit a retry only when the destination's
idempotency contract and exact key are both part of the recorded attempt. A generic
"probably safe" label is not sufficient.

### `governor-store-sqlite`

Keep one authority. Implement the Prime-Agent-style command journal inside SQLite,
not as another log file. Conceptual table:

```text
mutation_commands(
  actor_id,
  command_id,
  command_kind,
  status,              -- received | completed | uncertain | acked
  safe_result_kind,
  safe_result_blob_or_ref,
  daemon_epoch,
  created_at,
  completed_at,
  acked_at,
  PRIMARY KEY(actor_id, command_id)
)
```

The transaction protocol is:

```text
BEGIN IMMEDIATE
  insert unique (actor_id, command_id, received)
COMMIT
-> only now may adapter dispatch consequential I/O
-> commit completed safe result before replying
```

On startup or retry, `received` with no safe committed terminal result is
`uncertain`; it is never redispatched automatically. `completed` returns the
stored safe result for an exact retry. Retention/compaction requires the appropriate
receipt ACK plus policy age.

External attempts remain a separate domain table because command delivery and the
external side effect are different facts. Record effect class, destination fence,
idempotency key when applicable, source event, and attempt state.

### resource ownership

Where exclusive ownership is required, use canonical resource identity plus a
random lease token and process/daemon incarnation. A stale holder cannot release or
mutate a current lease merely because a PID or display/session name was reused.

For V1, the global daemon/state-root lock remains simpler than a distributed lease.
Use the richer lease pattern for session/runtime/browser resources only where a
second process legitimately participates.

### deterministic execution permit

Borrow Salvor's *shape*, not its API: distinguish a replayed decision from a live
permission to execute. Adapter code should not be able to reach consequential I/O
through the same return path used for already-recorded outcomes.

One possible internal shape:

```rust
enum EffectDecision<T> {
    Replayed(T),
    Execute(ExternalExecutionPermit),
    Reconcile(ReconciliationRequired),
}
```

`ExternalExecutionPermit` is created only after the intent transaction is durable
and contains the exact attempt/destination/source fences. It is not serializable
and carries no raw prompt/tool payload.

### mutation retry identity

Do not automatically add a generic public `request_id` parameter to every MCP tool
just because Agent Orchestrator has one. Command Governor already has strong domain
fences (`obligation_id/version`, `source_event`, `binding_generation`, `claim_id`,
accepted `delivery_id`).

Phase 1 should nevertheless implement an **internal stable mutation command
identity** for every daemon/IPC/MCP write. Phase 3 can decide whether the public ABI
needs an explicit client retry ID or whether the command identity can be derived
unambiguously from the existing fenced operation. The invariant is that transport
reconnect must not mint a new logical mutation identity by accident.

## Phase 2 application

Use the same external-attempt machinery for:

- worker launch/resume commands;
- delivery of a foreman answer back to a worker;
- Herdr clear/interrupt reconciliation where it is a consequential write;
- publication/promotion of a final-result candidate where file/DB ordering crosses
  a crash boundary.

The worker-host remains transport-only and retains only sanitized receipts plus the
bounded final-result candidate. The new journal pattern is not permission to spool
provider streams or raw tool payloads.

## Phase 3 application

The four-tool MCP ABI remains stable. Mutating tool handling should use the same
command-identity protocol internally so an exact retry can return the already
committed semantic result without executing the mutation twice.

Do not confuse this with browser `delivery_key`/`delivery_id`: browser wake identity,
MCP mutation identity, foreman claim identity, and semantic ACK are separate
fences.

## Acceptance tests to add before adapters

1. **intent-before-I/O** — fake adapter panics if invoked before the durable intent
   transaction is observable.
2. **kill after intent, before I/O** — restart yields ambiguous/reconciliation;
   zero automatic I/O.
3. **kill after I/O, before outcome commit** — restart yields the same
   ambiguous/reconciliation state; zero automatic replay.
4. **completed mutation retry** — same `(actor_id, command_id)` returns the recorded
   result without adapter invocation.
5. **pending mutation retry** — same identity returns typed uncertainty and never
   dispatches.
6. **different command ID** — is a genuinely new operation and must pass normal
   policy/fencing rather than being accidentally deduplicated.
7. **lease PID reuse** — same PID with different process-start identity cannot own
   or release the old lease.
8. **stale lease token/daemon epoch** — cannot release or mutate current ownership.
9. **exact receipt ACK** — permits command-journal retention only; cannot close a
   worker-result obligation.
10. **semantic ACK separation** — all transport receipts and notifications may be
    ACKed while `completed_unprocessed` remains open until valid `foreman_ack`.
11. **projection replay equivalence** — rebuild from source events yields identical
    attempt/obligation state.
12. **forbidden-data scan** — command journal and attempt tables never contain raw
    prompt, tool args/results, shell commands, cwd, transcript/provider stream, or
    credentials.

## Implementation order

The first executable PR should be deliberately small:

1. typed IDs/effect/attempt/command state in `governor-core`;
2. pure reducers plus table-driven state tests;
3. SQLite migrations and single-writer actor;
4. mutation-command receipt transaction and external-attempt transaction;
5. deterministic execution-permit seam;
6. resource-incarnation/lease primitives needed by the daemon skeleton;
7. failpoints for every intent/I/O/outcome boundary;
8. replay-equivalence and forbidden-data tests;
9. only then daemon IPC skeleton and later real adapters.

This order lets Command Governor prove the hard semantics without needing ChatGPT,
Claude, Herdr, Chrome, or GitHub credentials.

## Provenance rule

The reviewed projects are architecture and implementation-pattern references, not
source donors by default. No source is copied in this review. If a future PR copies
or materially adapts implementation code, it must record exact source file,
revision, license, local modifications, and required notices in
`THIRD_PARTY_NOTICES.md` before merge.
