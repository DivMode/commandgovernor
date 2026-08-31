# Architecture

## Purpose

Command Governor is a local-first control plane for coordinating AI coding
agents across conversational foremen, worker processes, source control, and
session runtimes. Its central responsibility is to preserve lifecycle truth and
outstanding obligations when any individual UI, process, adapter, or machine
session disappears.

This document describes the intended architecture. No implementation exists
yet.

## System roles

| Role | Responsibility | Authoritative for |
| --- | --- | --- |
| Foreman | Plans work, delegates, reviews results, and makes policy decisions | Human-visible decisions and review intent |
| Worker | Executes a bounded task and emits progress, questions, and results | Its own execution output, not global lifecycle state |
| Source host | Stores issues, commits, reviews, and pull requests | Durable engineering artifacts |
| Session runtime | Owns terminal and process execution | Process presence and runtime observations |
| Command Governor | Persists events, derives lifecycle state, and closes obligations | Orchestration lifecycle truth |

A single product may fill more than one role, but the boundaries remain
explicit.

## Core domain model

- **Project:** a configured engineering boundary and its adapters.
- **Actor:** a human, foreman, worker, or system component with a stable identity.
- **Task:** a requested outcome with policy, ownership, and terminal criteria.
- **Attempt:** one bounded execution of a task by a worker.
- **Session:** the runtime container associated with an attempt.
- **Event:** an immutable fact observed or accepted by the control plane.
- **Result:** a durable worker outcome and its artifact references.
- **Obligation:** work that remains owed, such as consuming a result, answering a
  question, reconciling a delivery, or reviewing a change.
- **Conversation binding:** the exact foreman conversation authorized to receive
  a browser delivery.
- **Delivery attempt:** the durable record of one proposed external submission.

Events are append-only. Current state is a projection that can be rebuilt from
accepted events and checked against materialized views.

## Lifecycle model

A task and its worker attempt are related but distinct. Initial task states are:

- `queued`
- `dispatching`
- `active`
- `blocked`
- `completed`
- `failed`
- `cancelled`

Completion does not imply consumption. A completed task can retain an open
`consume_result` or `review_result` obligation until the foreman records that
the result was handled. Runtime labels such as `working` or `idle` are evidence
used during reconciliation; they never overwrite durable lifecycle state by
themselves.

Every transition records its cause, actor, time, prior version, and relevant
external identifiers. Conflicting transitions fail closed and create an
attention obligation.

## Durable obligations

Obligations make unfinished coordination explicit. Each obligation has a stable
identifier, owner or routing policy, status, due or retry policy when relevant,
and the event that closes it.

Examples include:

- deliver a task to a worker;
- surface a worker's blocked-input request;
- consume and review a completed result;
- reconcile an ambiguous browser submission;
- verify a source-control artifact; and
- recover a session whose runtime state disagrees with stored state.

An obligation is never closed merely because a polling loop stopped.

## Browser delivery semantics

Browser delivery is bound to one exact ChatGPT conversation and uses at-most-once
submission semantics.

1. Persist a delivery intent with a stable delivery key and conversation binding.
2. Acquire an exclusive, expiring lease for that intent.
3. Verify that the visible browser destination matches the stored binding.
4. Submit at most once for that delivery key.
5. Persist positive acknowledgement when it can be observed.
6. If submission may have occurred but acknowledgement is unavailable, move the
   delivery to `reconciliation_required` and stop automatic retries.

This prevents an ambiguous browser event from becoming a duplicate prompt. It
cannot guarantee that every delivery occurs: a crash at the submission boundary
may leave zero or one submissions. The unresolved durable obligation makes that
tradeoff visible and recoverable by inspection.

Changing the bound conversation creates a new, explicit delivery intent; it
never mutates an in-flight destination silently.

## Worker dispatch and result handling

Worker adapters translate provider-specific events into the common lifecycle
model. Dispatch uses stable task and attempt identities. Where an external API
supports idempotency keys, the adapter must use them. Where it does not, an
ambiguous dispatch follows the same reconciliation rule as browser delivery.

Worker output is captured durably before it is announced as available. A
terminal completion event creates a result-consumption obligation. Session
shutdown cannot erase the result or satisfy that obligation.

## Crash and restart recovery

On startup, the recovery loop:

1. verifies the durable store and schema version;
2. rebuilds or validates lifecycle projections;
3. expires abandoned leases;
4. compares runtime observations with stored sessions;
5. resumes safe, idempotent obligations; and
6. quarantines ambiguous external side effects for reconciliation.

Recovery never infers that silence means completion, failure, or successful
delivery.

## Adapter boundaries

Adapters will expose capability-oriented interfaces for:

- conversational foremen and browser control;
- coding-agent workers;
- terminal and process session runtimes;
- source-control hosts; and
- local persistence and secret providers.

Provider identifiers and raw payloads may be retained for audit, but domain
state must not depend on one provider's status vocabulary.

## Security and privacy

- Local state is private by default and must have explicit retention controls.
- Credentials remain in platform-native secret stores and are referenced, not
  copied into events.
- Logs and exports redact prompts, source, tokens, and conversation content by
  default.
- Adapters receive the narrowest capabilities required for their role.
- Conversation and project bindings are verified before external writes.
- Every consequential external action has an auditable actor and cause.

## Initial non-goals

- Hosting a general-purpose cloud agent service.
- Replacing GitHub as the durable engineering record.
- Executing untrusted code without an isolated runtime.
- Claiming exactly-once semantics where an external interface lacks an
  idempotency or transactional acknowledgement mechanism.
- Choosing a permanent implementation stack before lifecycle invariants are
  testable.
