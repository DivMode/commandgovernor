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

There is also a durability/privacy problem at the process boundary. If Command
Governor is restarting while Claude finishes, a completion signal or final result
cannot exist only in the daemon's volatile stdout reader. But persisting Claude's
entire structured stream is also unacceptable: current stream-json output can
include tool-use/tool-result records, so a raw spool can persist tool arguments,
commands, results, prompts, or other transcript-like content that Command Governor
explicitly forbids from durable control-plane storage.

## Decision

### Managed Claude V1 uses the structured programmatic result as terminal evidence

Prefer managed non-interactive Claude turns through the current programmatic CLI
interface (`claude -p` with structured output, subject to conformance at
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

### A small Rust worker-host preserves only the reviewable result and sanitized receipts

A managed Claude process is launched/resumed through a narrow Rust transport mode
owned by the Command Governor binary, conceptually:

```text
Herdr / session runtime
  -> command-governor worker-host claude <opaque-turn-id>
       -> launches/resumes `claude -p`
       -> parses structured stdout online
       -> durably writes one bounded final-result candidate when complete
       -> durably writes sanitized run/child-exit receipts
       -> exits
```

The worker-host is **not** a second orchestration daemon. It owns no tasks,
obligations, browser state, foreman state, or lifecycle projections. Its only job
is to make the final reviewable provider result recoverable when the authoritative
daemon is temporarily absent.

The worker-host **does not persist the complete structured provider stream**.
Intermediate provider records are processed in memory and discarded after the
minimum safe lifecycle evidence is extracted. Durable run receipts may contain
only allowlisted opaque IDs, safe event/outcome classes, completeness flags,
timestamps, and sanitized child-exit metadata. They may not contain prompt text,
raw tool arguments/results, shell commands, cwd, transcript paths, terminal text,
secrets, or generic provider JSON.

When a complete final result arrives, the worker-host writes only the bounded final
assistant result needed for independent review to an owner-private candidate file.
The daemon later validates the exact run/result/exit fences and promotes/copies it
through the immutable result-artifact publication protocol.

### Command Governor owns managed hook configuration without assuming hook isolation

Do not edit the user's personal Claude settings. Managed workers receive a private
Command-Governor-owned settings file through the current supported Claude CLI
configuration path. The file and hook command are validated for ownership,
permissions, symlink safety, and contract epoch before launch.

Current Claude settings can merge hooks/customizations from multiple scopes, and
`--settings` alone is not assumed to remove every user/project/plugin hook. The
live adapter conformance suite must prove the actual active settings sources/hook
behavior for the chosen invocation. The completion rule remains correct even when
another Stop hook exists because Stop itself is only candidate evidence.

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
3. the managed structured run confirms that execution actually stopped with
   `tool_deferred`/equivalent and the tool call pending;
4. only then does the control plane project `needs_input`.

Current Claude documentation states that non-interactive `defer` is ignored when
Claude emits **multiple tool calls together**. That is a hard V1 constraint: only
a proven single-tool defer may become clean `needs_input`; multi-tool defer shapes
become reconciliation/manual attention rather than a fake pause.

### `PermissionRequest` is available in non-interactive sessions but is not the durable pause identity

The review corrected an earlier false assumption. Current Claude hook documentation
says `PermissionRequest` hooks **can run in sessions that cannot show a prompt,
including non-interactive/background contexts**; if no hook decides, the tool is
denied.

V1 may therefore use `PermissionRequest` as a permission-decision signal. However,
its current hook input carries tool name/input without the same `tool_use_id`
correlation supplied to `PreToolUse`. For exact durable tool-call identity,
policy/defer fencing, and same-session resume, `PreToolUse` remains the preferred
boundary.

The managed policy is:

- already delegated ordinary engineering work may proceed only as current Claude
  permission/settings rules permit;
- exact out-of-band decisions use confirmed `PreToolUse` defer when the current
  single-tool shape supports it;
- `PermissionRequest` decisions are handled only under the correlation guarantees
  proven for the pinned Claude release and are not automatically projected as a
  resumable `needs_input` state;
- destructive, credential-sensitive, materially broader, or unknown actions remain
  user-owned and fail closed.

Current Claude docs also make clear that hook allow decisions do not simply erase
settings precedence/deny rules. Command Governor never treats a worker hook as an
entitlement to widen user/managed policy.

### Structured worker truth outranks stale Herdr runtime inference

Herdr remains the process/session transport. A stale `working` sample cannot
override a confirmed structured final result, confirmed deferred input boundary,
non-blockable session termination, or another stronger fenced worker fact.

When worker/runtime disagree:

1. project the stronger confirmed worker fact;
2. record `runtime_state_conflict`;
3. before any continuation write, reconcile process transport, including one
   governor-authored clear/interrupt if required;
4. verify transport safety;
5. if unresolved, keep the original obligation open and do not create a duplicate
   worker.

A lone Stop-hook candidate is not sufficient to win this conflict.

### Durable final result artifact

Before publishing `completed_unprocessed`, the daemon validates the bounded
worker-host final-result candidate and sanitized exit/run receipts, then writes the
bounded final worker result to the immutable owner-private artifact store. File
durability is established before the SQLite transaction references its
digest/size/opaque key and creates the obligation.

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

## Local trust boundary

The worker-host is architecturally stateless, but V1 does not claim that same-user
file permissions sandbox a malicious Claude/tool process. The OS user account is
the local trust root. Managed worker environments receive only narrow opaque
correlation values needed by hooks; Command Governor does not intentionally export
the general state-root path to Claude. Strong hostile-worker containment would
require a future separate OS identity/sandbox/broker.

## Alternatives

### Treat Herdr `working`/idle as semantic completion truth

Rejected by the reproduced stale-working failure.

### Treat every Claude `Stop` hook invocation as completion

Rejected because current Stop hooks can block stopping and all matching hooks may
run in parallel. The callback is a stop candidate, not proof of final settlement.

### Treat `PermissionRequest` as unavailable in `claude -p`

Rejected by current Claude documentation. It can run in non-interactive contexts.
The architectural distinction is instead that `PermissionRequest` is a permission
decision signal with weaker exact tool-call correlation than `PreToolUse`, not the
preferred durable defer/resume identity.

### Poll terminal text harder

Rejected. More PTY heuristics do not become a structured provider protocol.

### Persist the complete `claude -p` stream

Rejected. Current structured streams can contain tool-use/tool-result records;
persisting them would violate the explicit no-prompt/no-tool-args/no-command
storage boundary. The worker-host parses online and durably retains only sanitized
receipts and the bounded final-result candidate.

### Capture `claude -p` only in the authoritative daemon's live stdout reader

Rejected because daemon restart could lose the final result while the worker
continues. The worker-host is a transport durability shim, not a second control
plane.

### Hook sends only live IPC/HTTP

Rejected because daemon restart can lose progress/input/native observations.

### Store the complete provider stream/transcript in SQLite

Rejected for privacy, size, prompt-injection, and data-boundary reasons.

### Monitor a worker with another Claude session

Rejected. It creates another worker/owner and still does not make the observer
semantically authoritative.

## Consequences

The Claude adapter must own a small worker-host staging path, parse the current
structured programmatic protocol online, validate settings/hook behavior,
implement permission/input policy at the proven current boundaries, and maintain a
provider conformance suite.

In return, completion survives daemon/runtime restart without a raw transcript
spool, a parallel Stop hook cannot produce false terminal state, input/permission
decisions use current provider semantics, and stale Herdr state cannot veto a
confirmed worker result or pause.
