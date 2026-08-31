# ADR 0005: Structured Claude completion, durable worker transport, and result artifacts

- **Status:** Proposed
- **Date:** 2026-08-31

## Context

Terminal/process state can disagree with the worker's real lifecycle. The known
failure is concrete: Claude can reach a real interrupted/input boundary while
Herdr continues to report `working / idle:false`. If the control plane treats the
runtime label as semantic authority, it can reject the continuation the worker
needs or open a duplicate worker.

An initial architecture draft made the opposite mistake: it treated invocation of
Claude's `Stop` hook as authoritative completion. Current Claude Code documentation
shows that is too strong. All matching hooks can run in parallel, and a `Stop` hook
can return a blocking decision that makes Claude continue. Our hook observing a
Stop candidate therefore does not prove that the response actually ended.

There is also a durability problem at the process boundary. If Command Governor is
restarting while Claude finishes, a completion signal or final structured result
cannot exist only in the daemon's volatile stdout reader. And if
`completed_unprocessed` points only at a terminal/session, closing that runtime can
erase the result while the obligation remains.

## Decision

### Managed Claude V1 uses the programmatic structured result as terminal evidence

Prefer managed non-interactive Claude turns through the current programmatic CLI
interface (`claude -p` with structured streaming output, subject to conformance at
implementation time).

For this mode, successful terminal evidence is the combination of:

1. the final structured Claude programmatic `result` record for the exact fenced
   session/turn; and
2. the matching child-command/process completion receipt.

A `Stop` hook invocation is stored only as bounded `stop_candidate` evidence. It
cannot by itself create `completed_unprocessed`, because another matching Stop
hook may veto that stop and Claude may continue.

`StopFailure`, `SessionEnd`, structured failure output, process exit, interrupt,
and transport loss are separate evidence classes. The adapter normalizes their
allowed combinations explicitly instead of using event-name intuition or "newest
status wins."

### A small Rust worker-host preserves the structured result across daemon restart

A managed Claude process is launched/resumed through a narrow Rust transport mode
owned by the Command Governor binary, conceptually:

```text
Herdr / session runtime
  -> command-governor worker-host claude <opaque-turn-id>
       -> launches/resumes `claude -p`
       -> captures the structured stream in an owner-private bounded spool
       -> writes a sanitized child-exit receipt
       -> exits
```

The worker-host is **not** a second orchestration daemon. It owns no tasks,
obligations, browser state, foreman state, or lifecycle projections. Its only job
is to make the provider transport/result recoverable when the authoritative daemon
is temporarily absent.

The sensitive transport spool is separate from SQLite and routine logs. The daemon
later validates the fenced stream/exit receipt, extracts only the bounded final
worker result needed for review, and commits that result to the immutable result
artifact store.

### Command Governor owns managed hook configuration without assuming hook isolation

Do not edit the user's personal Claude settings. Managed workers receive a private
Command-Governor-owned settings file through the current supported Claude CLI
configuration path. The file and hook command are validated for ownership,
permissions, symlink safety, and contract epoch before launch.

Current Claude settings can merge hooks/customizations from multiple scopes, and
`--settings` alone is not assumed to remove every user/project/plugin hook. The
live adapter conformance suite must prove the exact active settings/hook sources
for the chosen invocation. The design remains correct even when another Stop hook
can veto ours because Stop itself is only candidate evidence.

### Hooks use a durable sanitized inbox for progress/input/native observations

Managed hooks first write a sanitized owner-private event envelope to an atomic
local hook inbox and then return to Claude. The daemon imports/deduplicates the
inbox when alive or after restart.

The inbox never generically persists raw hook stdin. It contains only allowed
opaque identities, event class, safe classification, and timestamps. Prompt text,
raw tool arguments, shell commands, cwd, transcript path, terminal transcript,
provider stream bodies, and credentials are forbidden.

### Prefer confirmed `PreToolUse` deferral for durable out-of-band input

Current Claude Code supports a `defer` decision from `PreToolUse` in
non-interactive mode. For `AskUserQuestion` and policy-gated tool calls, V1 should
prefer that boundary when conformance proves it reliable.

The durable sequence is:

1. Command Governor identifies the exact fenced tool call;
2. its hook records safe defer intent and returns the current documented defer
   decision;
3. the managed structured run confirms that execution actually stopped with the
   tool call pending;
4. only then does the control plane project `needs_input`.

A hook merely attempting to defer is not lifecycle truth. Unsupported multi-tool
shapes or missing confirmation become reconciliation attention.

`PermissionRequest` is important native evidence that Claude wants a permission
decision, but it is not assumed to be a generic durable pause/resume primitive.
For an action that requires out-of-band authorization, prefer a policy
`PreToolUse` defer before execution. High-risk, destructive, credential-sensitive,
materially broader, or unknown requests remain user-owned by default.

### Native/structured worker truth outranks stale Herdr runtime inference

Herdr remains the process/session transport. A stale `working` sample cannot
override a confirmed structured final result, confirmed deferred input boundary,
non-blockable session termination, or another stronger fenced worker fact.

When the worker/runtime disagree:

1. project the stronger confirmed worker fact;
2. record `runtime_state_conflict`;
3. before any continuation write, reconcile the process transport, including one
   governor-authored clear/interrupt if required;
4. verify transport safety;
5. if unresolved, keep the original obligation open and do not create a duplicate
   worker.

A lone Stop-hook candidate is not sufficient to win this conflict.

### Durable final result artifact

Before publishing `completed_unprocessed`, the daemon validates the worker-host
structured result/exit receipt and writes the bounded final worker result to an
immutable owner-private artifact store. File durability is established before the
SQLite transaction references its digest/size/opaque key and creates the
obligation.

The artifact is not a terminal transcript or complete provider stream. It is the
bounded final result needed by the foreman; GitHub commit/PR refs are additional
engineering evidence. Open obligations pin artifact retention.

### Watchdog is progress-based attention only

Structured/native tool progress updates bounded `last_progress_at`. Lack of
verified progress beyond threshold creates `suspected_stall`; it never fabricates
completion, failure, interruption, or a monitor worker.

## Worker input delivery

Recording a foreman answer does not prove Claude received it. Answer/resume is a
separate external delivery with accepted/failed/ambiguous semantics. Matching
structured/native resumed-turn evidence is required before returning the
obligation to `running`.

## Alternatives

### Treat Herdr `working`/idle as semantic completion truth

Rejected by the reproduced stale-working failure.

### Treat every Claude `Stop` hook invocation as completion

Rejected because current Stop hooks can block stopping and all matching hooks may
run in parallel. The callback is a stop candidate, not proof of final settlement.

### Poll terminal text harder

Rejected. More PTY heuristics do not become a structured provider protocol.

### Capture `claude -p` only in the authoritative daemon's live stdout reader

Rejected because daemon restart could lose the final result while the worker
continues. The worker-host/spool is a transport durability shim, not a second
control plane.

### Hook sends only live IPC/HTTP

Rejected because daemon restart can lose progress/input/native observations.

### Store the complete provider stream/transcript in SQLite

Rejected for privacy, size, prompt-injection, and data-boundary reasons. Sensitive
transport spool and bounded final result artifact have explicit private retention
boundaries; the event ledger remains sanitized.

### Monitor a worker with another Claude session

Rejected. It creates another worker/owner and still does not make the observer
semantically authoritative.

## Consequences

The Claude adapter is more disciplined than the original Stop-hook design: it must
own a small worker-host/spool path, parse the current structured programmatic
protocol, validate hook/settings behavior, and maintain a provider conformance
suite.

In return, completion survives daemon/runtime restart, a parallel hook cannot
produce a false terminal result, input pauses are proven rather than inferred, and
stale Herdr state cannot veto a confirmed worker boundary.
