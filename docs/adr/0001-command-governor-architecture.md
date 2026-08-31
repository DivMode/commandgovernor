# ADR 0001: Local-first durable orchestration control plane

- **Status:** Accepted
- **Date:** 2026-08-31

## Context

AI coding workflows span browser conversations, worker agents, terminal/session
runtimes, and source-control systems. None of those components alone can
reliably answer what work is still owed after a disconnect or crash.

Process status is transient. Browser submission may be ambiguous. A worker may
finish after its foreman stops polling. Retrying an uncertain external action
can duplicate work, while refusing to retry without recording an obligation can
lose it. The system needs one durable lifecycle authority that treats these
conditions as explicit state.

## Decision

Command Governor will be a clean new implementation of a local-first control
plane with the following boundaries:

1. A transactional local store records immutable lifecycle events and durable
   obligations.
2. Materialized task and session state is derived from those records and can be
   rebuilt.
3. Foremen, workers, source hosts, and session runtimes integrate through narrow
   adapters.
4. Browser delivery is bound to an exact conversation and is at-most-once per
   durable delivery key.
5. Ambiguous external effects are quarantined for reconciliation rather than
   retried automatically.
6. Worker results remain durable until an explicit consumption or review event
   closes their obligation.
7. Provider-specific status values are observations, not domain lifecycle
   truth.

No third-party source code is imported or vendored by this decision or the
initial repository scaffold. Future dependencies and incorporated materials
must be reviewed and documented independently.

## Required invariants

- A task has at most one active owner unless a recorded policy explicitly
  permits parallel attempts.
- A terminal worker result cannot be deleted by session shutdown.
- A result-consumption obligation survives foreman and application restart.
- The same browser delivery key is never submitted automatically more than once.
- An adapter cannot close an obligation without an accepted closing event.
- Recovery does not convert missing observations into success.
- Every external write records its project, actor, cause, and destination.

## Consequences

### Positive

- Crash recovery is a core behavior rather than an adapter-specific patch.
- Results and blocked-input requests remain discoverable.
- Provider and runtime integrations can evolve without redefining lifecycle
  semantics.
- Ambiguous browser operations fail visibly instead of producing silent
  duplicates.

### Costs

- The event and obligation model adds complexity before visible automation.
- At-most-once delivery may require human reconciliation and can result in no
  submission when the boundary is ambiguous.
- Adapters need rigorous conformance and failure-injection tests.
- Local durable state requires migration, backup, privacy, and corruption
  handling from the beginning.

## Alternatives considered

### Treat the session runtime as authoritative

Rejected because process presence cannot prove task ownership, result
consumption, browser delivery, or review completion.

### Reconstruct state only from source control

Rejected because source control does not represent pending prompts, blocked
input, delivery ambiguity, or uncommitted worker results.

### Automatically retry every uncertain operation

Rejected because browser and some worker interfaces lack transactionally
enforced idempotency and can duplicate prompts or work.

### Make a hosted service authoritative from the start

Rejected for the initial architecture because it adds remote trust, privacy,
availability, and operating dependencies before the lifecycle model is proven.
