# Worker lifecycle, input, and watchdog contract

V1 starts with Claude Code as the primary implementation worker, while keeping the
control-plane state machine worker-neutral. Herdr (or another runtime) owns
process/session mechanics; it does not define semantic completion.

## Boundary

The worker integration is split deliberately:

```text
Command Governor core
        │
        ├── Worker adapter (Claude semantics)
        │     ├── launch/resume command construction
        │     ├── managed lifecycle hooks
        │     ├── native session/turn correlation
        │     └── bounded final result capture
        │
        └── Runtime adapter (Herdr semantics)
              ├── process/session spawn/adopt
              ├── stdin/interrupt/close
              └── process existence / runtime observations
```

A runtime saying `working` cannot veto a stronger native Claude event for the same
session incarnation and turn.

## Current Claude events to use

The implementation must re-verify the live Claude Code hook schema at the pinned
implementation commit. As of the 2026-08-31 research, relevant documented events
include:

- `UserPromptSubmit`
- `PreToolUse`
- `PermissionRequest`
- `PostToolUse`
- `PostToolUseFailure`
- `PostToolBatch`
- `Notification`
- task/subagent lifecycle events
- `Stop`
- `StopFailure`

The source is <https://code.claude.com/docs/en/hooks>.

### Authority classification

| Native event | V1 use | Authority |
| --- | --- | --- |
| `Stop` | end of Claude response | authoritative terminal-response evidence for fenced turn |
| `StopFailure` | API/turn failure | authoritative failure evidence for fenced turn |
| `PreToolUse` on `AskUserQuestion` | defer/block non-interactive question | authoritative input boundary when our hook returns current documented defer decision |
| `PermissionRequest` | permission boundary | authoritative `needs_input`/policy event |
| `UserPromptSubmit` / verified resumed turn | new turn accepted | authoritative evidence that a continuation/resume began |
| `PostToolUse` / batch | progress heartbeat | bounded liveness evidence only |
| `PostToolUseFailure` | progress/error evidence | not terminal unless current Claude contract says terminal |
| `Notification.agent_needs_input` | corroborating input evidence | useful but may be delayed; not sole detector |
| runtime `working` / PTY idle | process transport observation | lower authority than native lifecycle |

Do not infer completion from `Notification.agent_completed` if a stronger direct
turn event exists; use the native event documented for the relevant lifecycle.

## Managed hook installation

Command Governor must not edit the user's personal `~/.claude/settings.json`.
Managed Claude turns use a Command Governor-owned settings file passed through the
supported Claude settings mechanism.

The settings file:

- lives under the private Command Governor state root;
- is a regular owner-owned file, not a symlink;
- is not group/world writable;
- references a stable packaged command such as `command-governor hook claude ...`;
- is validated before every managed spawn/adoption that relies on it;
- is versioned with a hook-contract epoch.

The exact command path must survive normal package/Nix updates. A stable installed
binary name is preferable to embedding a build-output/source path.

## Hook durability when the daemon is down

A hook that only POSTs to the daemon can lose the most important event at exactly
the wrong time: Claude finishes while Command Governor is restarting.

Therefore lifecycle hooks first deposit a **sanitized durable envelope** into a
private hook inbox owned by Command Governor, then return to Claude. The daemon
watches/imports that inbox when alive and drains it on startup.

Conceptual layout:

```text
~/.command-governor/
  hook-inbox/          # 0700
    <event-id>.json    # 0600, atomic temp -> rename
```

Hook deposit sequence:

1. read Claude hook JSON from stdin without logging it;
2. validate hook event class and Command Governor environment fences;
3. extract only allowed identifiers/safe fields;
4. derive/dedupe a source event identity without hashing secret payload text;
5. write owner-only temp envelope;
6. fsync file;
7. atomic rename into inbox;
8. sync directory as required;
9. emit only the exact hook-control JSON required by Claude, if this hook has a
   control decision;
10. exit according to the Claude hook contract.

Daemon ingestion inserts/deduplicates the event in SQLite transactionally and
then removes/archives the inbox file. A crash after DB commit but before inbox
cleanup is harmless because source-event uniqueness deduplicates it.

This inbox contains **lifecycle envelopes, not transcripts**.

## Hook identity environment

Every managed turn receives non-secret Command Governor correlation values through
environment inherited by hooks, for example:

```text
COMMAND_GOVERNOR_SESSION_ID
COMMAND_GOVERNOR_SESSION_INCARNATION_ID
COMMAND_GOVERNOR_TURN_ID
COMMAND_GOVERNOR_HOOK_EPOCH
COMMAND_GOVERNOR_STATE_ROOT
```

These are opaque IDs, not cwd or prompt material. The adapter also records the
native Claude session ID as a safe external identity when the hook schema exposes
it.

If a hook event cannot prove that its correlation belongs to the currently known
session incarnation/turn, it is quarantined as an orphan observation and cannot
mutate the active turn.

## Forbidden hook persistence

Hook code must never persist or log raw:

- prompt text;
- tool arguments;
- shell commands;
- cwd;
- transcript path;
- terminal transcript;
- environment secrets;
- browser/GitHub credentials.

For progress, persist only event class, fenced identity, source event identity,
and time.

For a blocking input request, the database stores a safe opaque input identity and
classification, **not raw tool arguments**. The actual current question/permission
detail is obtained ephemerally from the native worker/session when the foreman
claims it. If the detail cannot be recovered after restart, the durable
`needs_input` obligation remains open and reports `input_detail_unavailable`; the
system does not invent an answer.

For Claude's current deferred `AskUserQuestion` path, the pending tool call remains
in Claude's own session. Command Governor stores the native session/tool-use
identity needed to locate/reconcile it, subject to current Claude capabilities,
without copying the raw arguments into its ledger.

## Preferred Claude V1 execution mode

Where current Claude behavior supports it reliably, prefer managed non-interactive
turns (`claude -p` or its current equivalent) with explicit same-session resume.
Reasons:

- native structured lifecycle is easier to fence than screen text;
- `AskUserQuestion` can currently be deferred by `PreToolUse` in non-interactive
  mode and resumed later;
- final result can be bounded and captured explicitly;
- process lifetime and Claude logical session become separate, which matches
  Command Governor's session-incarnation model.

Herdr may still host/own the process/session layer. The key point is that the
control plane speaks in native Claude turn identities rather than treating a PTY
as the semantic API.

Interactive Claude remains an adapter mode if a required workflow cannot use
non-interactive turns, but it receives the same native hook contract and lower
confidence for operations that require terminal inference.

## `AskUserQuestion` defer/resume

Current Claude Code documentation supports a `defer` decision from `PreToolUse`;
in non-interactive mode this can preserve a pending `AskUserQuestion` call for a
later resume of the same Claude session.

V1 flow:

```text
Claude calls AskUserQuestion
  -> Command Governor PreToolUse hook recognizes the tool class
  -> hook durably deposits safe needs_input envelope
  -> hook returns current documented DEFER decision
  -> Claude non-interactive turn stops with deferred pending tool call
  -> obligation = needs_input
  -> browser wakes exact foreman
  -> foreman_resume claims input obligation
  -> adapter retrieves current question ephemerally from native session/provider
  -> foreman_answer_input records structured answer if authorized
  -> worker resume delivery is claimed/fenced
  -> same Claude session is resumed with current supported answer mechanism
  -> native UserPromptSubmit/resumed-turn evidence arrives
  -> obligation returns to running
```

Current documentation notes limitations around deferral when multiple tool calls
are emitted in the same response. The implementation must detect unsupported
shapes and fail visibly into `needs_input`/manual reconciliation rather than
silently accepting an answer it cannot deliver.

## Permission requests

`PermissionRequest` becomes durable `needs_input` immediately. The event is not a
license for the ChatGPT foreman to grant arbitrary access.

Each request is classified against recorded project/user policy:

- already delegated, ordinary engineering permission: foreman may answer;
- materially broader/destructive/credential/security-sensitive action: user-owned;
- unknown classification: user-owned by default.

When user-owned, MCP returns `user_authorization_required`. No worker resume or
permission write occurs until a durable user-authorized decision exists.

## Progress heartbeat

`PostToolUse`/batch-equivalent events are a natural verified progress source, but
persist only bounded safe metadata:

```text
turn_id
source_event_id
event_class = "tool_progress"
occurred_at
```

No tool name/arguments/result is required for the watchdog. The adapter may
coalesce very high-rate progress deterministically (for example, at most one
stored heartbeat per configured short window) while retaining enough evidence to
prove the no-progress threshold.

## Watchdog

Watchdog input is the last **verified** progress/native lifecycle timestamp for a
running turn. Screen repaint does not reset it.

If:

```text
now - last_verified_progress_at >= configured_threshold
```

and there is no terminal/input event, create one `suspected_stall` health/attention
record for the current turn generation.

A later verified progress event resolves that attention. It does not create a new
worker, interrupt automatically, or change the turn to failed/completed.

No monitor-only Claude session is opened. `command-governor status` and native
runtime queries are enough to observe the worker.

## Stale Herdr `working` conflict

This exact reproduced failure is a required fixture:

```text
Claude native state: blocked/interrupted, needs input
Herdr observation: working / idle=false
```

Command Governor behavior:

1. accept native input/stop evidence for the fenced turn;
2. project `needs_input` (or the relevant native state) immediately;
3. record a `runtime_state_conflict` health event because Herdr disagrees;
4. never reject the foreman's needed answer solely because Herdr says `working`;
5. before writing/resuming the worker, reconcile the process transport;
6. if necessary, issue one Command-Governor-authored runtime interrupt/clear
   operation to remove the stale busy condition;
7. verify the runtime is safe for the next command;
8. if it remains inconsistent, preserve the input obligation and surface
   reconciliation failure instead of opening a duplicate worker.

The runtime adapter needs an explicit reconciliation/clear-busy capability; it
must not encode "Herdr working means send forbidden forever" as the domain rule.

## Interrupt

An interrupt has two separate facts:

- Command Governor requested runtime interruption;
- native worker lifecycle reported the consequence.

The first never fabricates the second. If the user/foreman interrupts a turn,
record a governor-authored interrupt intent, issue the runtime operation at most
once under its delivery fence, then wait for native/process evidence.

An interrupted turn may become `needs_input`, failed, or another current Claude
state. It is not automatically "cancelled" unless the task/obligation itself is
explicitly cancelled.

## Close

Closing a runtime session is a process operation. It cannot delete:

- lifecycle events;
- result artifacts;
- input obligations;
- completed/failed unprocessed obligations;
- browser deliveries;
- foreman claims/history.

Before close, the daemon records why the session is being closed and whether the
current turn has a durable terminal/input state. A runtime close over an unresolved
turn creates reconciliation attention, not success.

## Result capture

A `Stop` event says the response boundary happened. Before publishing
`completed_unprocessed`, the worker adapter must capture the bounded final result
needed by the foreman and commit it through the result-artifact sequence in
[data-model.md](data-model.md).

"Final result" is not the whole terminal transcript. It is the worker's bounded
terminal response plus stable engineering refs (commit/PR/etc.) needed for review.

If a result is too large, store a bounded artifact and stable source refs rather
than silently truncating the only evidence. MCP paging can deliver the artifact;
GitHub remains the place to inspect large diffs/source.

## Failure capture

`StopFailure` produces a safe failure classification and `failed` attention. Raw
provider error bodies are not automatically persisted. If the foreman requires
more diagnostic content, the adapter may expose current provider/runtime details
ephemerally under the same redaction policy.

## Session re-adoption and incarnation

After daemon/runtime restart, an existing native Claude session may be re-adopted
only when continuity can be proven from stable native/runtime identities and the
stored session fence. If continuity is uncertain, create a new
`session_incarnation_id`.

An old hook inbox event can still be ingested for history but cannot mutate the
new incarnation unless its fences match.

## Deterministic fake worker

The testkit must include a fake Claude/native lifecycle source that can emit, in
controlled order:

- progress;
- Stop;
- StopFailure;
- AskUserQuestion deferred;
- PermissionRequest;
- resumed turn;
- duplicate terminal event;
- late old-incarnation event;
- runtime `working` disagreement;
- daemon-offline hook inbox deposits.

All lifecycle correctness tests run against this fake without real Claude. Real
Claude is an adapter conformance suite, not the only place state-machine bugs can
be found.
