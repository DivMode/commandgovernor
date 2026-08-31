# Roadmap

Command Governor is currently in the architecture phase. Milestones describe
outcomes, not release dates.

## Phase 0: Foundation

- Establish project governance, security reporting, and contribution policy.
- Specify lifecycle states, events, obligations, and invariants.
- Record architecture decisions before selecting implementation dependencies.
- Define a failure-injection test strategy.

## Phase 1: Durable lifecycle kernel

- Implement a local transactional event store and rebuildable projections.
- Model projects, tasks, attempts, sessions, results, and obligations.
- Enforce versioned transitions and exclusive leases.
- Add deterministic restart and corruption-recovery tests.

Exit criterion: a task lifecycle can be replayed exactly from durable events,
and an unconsumed result remains visible after a forced restart.

## Phase 2: First worker and runtime adapters

- Add one coding-agent worker adapter and one session-runtime adapter.
- Separate runtime observations from authoritative task state.
- Persist progress, blocked-input requests, terminal outcomes, and result
  artifacts.
- Detect and prevent duplicate ownership of a task.

Exit criterion: a worker can finish after the foreman disconnects and its result
is recovered without opening a duplicate worker.

## Phase 3: Conversation-bound foreman delivery

- Persist exact conversation bindings and delivery keys.
- Verify the active destination before browser submission.
- Implement at-most-once submission and `reconciliation_required` handling.
- Surface blocked workers and unconsumed results to the foreman.

Exit criterion: failure injection at every delivery boundary produces zero or
one submissions and never an automatic duplicate.

## Phase 4: End-to-end recovery and review

- Reconcile stored state with browser, runtime, worker, and source-host state.
- Restore safe leases and obligations after application or machine restart.
- Track review decisions and source-control artifacts through completion.
- Add audit views and operator diagnostics.

Exit criterion: an interrupted workflow resumes without losing results,
duplicating dispatch, or silently closing obligations.

## Phase 5: Multiple providers and runtimes

- Stabilize provider-neutral adapter contracts.
- Add additional foreman, worker, source-host, and session-runtime adapters.
- Publish compatibility and capability matrices.
- Add migration and backup tooling for local state.

Exit criterion: lifecycle behavior remains consistent across at least two
independent worker providers and two session runtimes.

## Later considerations

- Multi-machine coordination without surrendering local ownership.
- Policy engines for approvals and resource limits.
- Signed audit exports and portable project state.
- Optional team collaboration with explicit privacy boundaries.
