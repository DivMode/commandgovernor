# ADR 0001: Local-first durable orchestration control plane

- **Status:** Accepted
- **Date:** 2026-08-31

## Context

AI coding workflows span browser conversations, worker agents, terminal/session
runtimes, and source-control systems. None of those components alone can reliably
answer what work is still owed after a disconnect or crash.

Process status is transient. Browser submission may be ambiguous. A worker may
finish after its foreman stops polling. Retrying an uncertain external action can
duplicate work, while refusing to retry without recording an obligation can lose
it. The system needs one durable lifecycle authority that treats these conditions
as explicit state.

The motivating failure is not merely "we missed a notification." The foreman may
disappear while the worker continues, the runtime may report stale `working`, a
completed result may never be consumed, or ChatGPT may settle a turn without
performing the required independent review.

## Central invariant

> Delegated work must not disappear merely because ChatGPT, a browser, a terminal
> runtime, a worker process, or Command Governor itself restarts.

Worker completion does **not** close the work. It creates/preserves a durable
attention obligation such as `completed_unprocessed`. Only explicit foreman
processing plus a fenced ACK/disposition closes the normal result obligation.

Browser delivery is not acknowledgement. ChatGPT assistant-turn settlement is not
acknowledgement. Runtime idle is not acknowledgement.

## Decision

Command Governor will be a clean new implementation of a local-first control
plane with these boundaries:

1. One authoritative daemon records immutable lifecycle events and durable
   obligations.
2. Materialized task/session/obligation state is derived from those events and can
   be rebuilt/validated.
3. Foremen, workers, source hosts, and session runtimes integrate through narrow
   adapters.
4. Browser delivery is bound to one exact conversation and uses at-most-once
   ambiguity semantics.
5. Ambiguous external effects are quarantined for reconciliation rather than
   retried automatically.
6. Worker results remain durable until an explicit review/consumption disposition
   closes their obligation and retention policy allows deletion.
7. Provider-specific status values are observations, not domain lifecycle truth.
8. Native worker lifecycle/input evidence outranks stale PTY/runtime inference for
   the same fenced turn.
9. No human completion-notification subsystem is required for correctness. The
   system wakes the bound ChatGPT foreman itself.
10. V1 has no conventional GUI. A CLI is a client/projection of daemon truth.

No third-party source code is imported or vendored by this decision. Future
dependencies and incorporated materials require license/security review and
provenance documentation.

## Required invariants

- A task has at most one active owner unless a recorded policy explicitly permits
  parallel attempts.
- A session name alone is never an identity fence.
- A terminal worker result cannot be deleted by session shutdown.
- A result/input/failure obligation survives foreman and application restart.
- The same accepted or ambiguous browser delivery is never automatically resent.
- An adapter cannot close an obligation without an accepted closing event.
- A stale foreman binding generation cannot ACK current work.
- Recovery does not convert missing observations into success.
- Browser accepted, ChatGPT settled, and foreman ACK remain three distinct facts.
- Every consequential external write records its project, actor/cause, and fenced
  destination identity.

## Consequences

### Positive

- Crash recovery is a core behavior rather than an adapter-specific patch.
- Results and blocked-input requests remain discoverable.
- Provider and runtime integrations can evolve without redefining lifecycle
  semantics.
- Ambiguous browser/worker operations fail visibly instead of producing silent
  duplicates.
- A future official foreman/wake API can replace the ChatGPT Web adapter without
  rewriting the durable kernel.

### Costs

- The event and obligation model adds complexity before visible automation.
- At-most-once delivery deliberately sacrifices liveness at an ambiguous boundary;
  a crash can leave zero sends and a durable reconciliation obligation.
- Adapters need rigorous conformance and crash-injection tests.
- Local durable state requires migrations, backup, privacy, artifact retention,
  and corruption handling from the beginning.
- Current ChatGPT product capabilities may make some account/plan combinations
  unsupported until write-capable MCP is available.

## Alternatives considered

### Treat the session runtime as authoritative

Rejected because process presence cannot prove task ownership, native worker
completion, result consumption, browser delivery, or review completion.

### Reconstruct state only from GitHub

Rejected because source control does not represent pending prompts, blocked input,
delivery ambiguity, or uncommitted worker results.

### Automatically retry every uncertain operation

Rejected because browser and some worker interfaces lack transactionally enforced
idempotency and can duplicate prompts/work.

### Use human notifications as the recovery loop

Rejected. The user should not need phone/email/Slack/ntfy alerts to discover that
a worker finished. The durable system must wake/reconnect the foreman itself while
keeping CLI status available for diagnosis.

### Make a hosted service authoritative from the start

Rejected for V1 because it adds remote trust, privacy, availability, and operating
dependencies before the lifecycle model is proven.

## Refining ADRs

- [ADR 0002](0002-rust-daemon-and-sqlite.md): Rust daemon/CLI and `rusqlite`
  single-writer authority.
- [ADR 0003](0003-chatgpt-browser-hybrid.md): browser-backed ChatGPT hybrid and
  at-most-once Send semantics.
- [ADR 0004](0004-foreman-mcp-and-binding.md): stable MCP ABI, binding generation,
  explicit ACK, and capability gate.
- [ADR 0005](0005-worker-lifecycle-and-result-durability.md): native worker
  lifecycle, durable hook inbox, input/watchdog, and result artifact boundary.
