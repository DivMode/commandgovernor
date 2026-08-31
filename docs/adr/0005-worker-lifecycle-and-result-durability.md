# ADR 0005: Native worker lifecycle, durable hook inbox, and result artifacts

- **Status:** Proposed
- **Date:** 2026-08-31

## Context

Terminal/process state can disagree with the worker's real lifecycle. The known
failure is concrete: Claude can reach a real interrupted/input boundary while
Herdr continues to report `working / idle:false`. If the control plane treats the
runtime label as authority, it can reject the exact continuation the worker needs
or open a duplicate worker.

There is a second durability problem: if a native lifecycle hook reports Stop only
to a live daemon, Command Governor can miss completion while restarting. And if
`completed_unprocessed` points only to a terminal session, closing the runtime can
erase the actual result while the obligation remains.

## Decision

### Native lifecycle is primary semantic evidence

For managed Claude turns, use the current documented native hooks. `Stop`,
`StopFailure`, blocking `AskUserQuestion`/permission evidence, and verified resume
signals are normalized into the worker state machine. Herdr remains the
process/session adapter and can report conflicts, but its stale `working` sample
cannot override a matching native event.

### Command Governor owns managed hook configuration

Do not edit personal Claude settings. Managed workers receive a private,
Command-Governor-owned settings file using the current supported Claude mechanism.
The file and hook command path are validated for ownership/mode/symlink safety
before launch.

### Hooks deposit durably before talking to the daemon

Lifecycle hooks first write a sanitized owner-private event envelope to an atomic
local hook inbox. The daemon ingests/deduplicates it into SQLite and then removes
the inbox file. This makes a Stop/input event recoverable even when the daemon is
offline at hook time.

The hook never generically persists the raw stdin payload. It extracts only
allowed identity/event/time fields. Prompt text, raw tool arguments, commands,
cwd, transcript path, terminal transcript, and secrets are forbidden.

### Prefer native deferred input where feasible

Current Claude Code supports a `PreToolUse` defer decision for
`AskUserQuestion` in non-interactive mode. Prefer same-session defer/resume where
conformance testing proves it reliable. Persist the durable `needs_input` identity
and native refs, not raw tool arguments. Retrieve current question detail
ephemerally from the provider/session when processing; if unavailable after
restart, preserve the obligation and report `input_detail_unavailable` rather than
inventing an answer.

### Durable final result artifact

Before publishing `completed_unprocessed`, write the bounded final worker result
to an immutable owner-private artifact store, fsync/rename it, then reference its
digest/size/opaque key in the same SQLite transaction that records terminal
lifecycle and creates/updates the obligation.

The artifact is not a terminal transcript. It is the bounded final result needed
for review; GitHub refs are additional durable engineering evidence.

An open obligation pins artifact retention.

### Watchdog is progress-based attention only

Native tool/lifecycle progress updates bounded `last_progress_at`. Lack of
verified progress beyond threshold creates `suspected_stall`; it never fabricates
completion/failure or opens a monitor-only worker.

## Stale Herdr reconciliation

When native lifecycle says blocked/terminal but Herdr says working:

1. trust/project native semantic state;
2. record `runtime_state_conflict`;
3. before any continuation write, use an explicit runtime reconciliation/clear
   operation, including one governor-authored interrupt if required;
4. verify transport safety;
5. if unresolved, keep the original obligation open and do not create a duplicate
   worker.

The exact reproduced stale-working condition is a deterministic regression test.

## Worker input delivery

Recording an answer is not evidence that Claude received it. Answer/resume uses a
separate external delivery record with accepted/failed/ambiguous semantics. Only
matching native resumed-turn evidence returns the obligation to `running`.

## Alternatives

### Herdr idle/working as completion truth

Rejected by reproduced real failure.

### Poll terminal text harder

Rejected. More screen heuristics do not turn inference into native lifecycle and
can create new false positives.

### Hook sends only live IPC/HTTP

Rejected because daemon restart can lose the event.

### Store full transcript in SQLite

Rejected for privacy, size, injection, and data-boundary reasons. Store only the
bounded final result artifact needed for an open obligation plus safe event
metadata.

### Monitor worker with another Claude session

Rejected. It wastes a worker, introduces duplicate ownership/state, and still does
not make the observer authoritative.

## Consequences

The Claude adapter must own a small hardened hook-deposit executable path and
provider-specific lifecycle conformance suite. In return, the control plane no
longer depends on PTY idleness for its most important worker facts, and completion
survives both daemon and runtime restarts.
