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
        │     ├── structured `claude -p` stream/result parser
        │     ├── managed lifecycle hooks
        │     ├── native session/turn correlation
        │     └── bounded final result capture
        │
        └── Runtime adapter (Herdr semantics)
              ├── process/session spawn/adopt
              ├── stdin/interrupt/close
              └── process existence / runtime observations
```

A runtime saying `working` cannot veto stronger fenced Claude protocol/process
evidence. Conversely, a hook callback is not automatically terminal merely
because its event name sounds terminal.

## Important correction: a Claude `Stop` hook firing is not final completion

Current Claude Code documentation says:

- all matching hooks run in parallel;
- a `Stop` hook can return `decision: "block"` and make Claude continue;
- `Stop` input includes `stop_hook_active` specifically to prevent continuation
  loops; and
- prompt/agent Stop hooks can likewise return a negative decision that continues
  the turn.

Therefore Command Governor must **not** equate "our Stop hook was invoked" with
"the managed turn definitely ended." Another user/project/plugin/managed Stop hook
may still veto the stop, and hook execution order is not a safe arbitration
mechanism.

For the preferred non-interactive V1 mode, successful turn completion is confirmed
from the programmatic Claude interface: the final structured `result` message from
`claude -p --output-format stream-json` (or the current equivalent) plus the
matching process/command exit receipt. Claude's current programmatic docs specify
that the final stream line is a `result` message carrying the final response and
session metadata, and that `claude -p` exits 0 on success/non-zero on failure.

`SessionEnd` is useful corroborating evidence because it cannot block session
termination, but it does not by itself say the work result was successful. A
`Stop` hook invocation is retained as `stop_candidate` evidence/progress; it is
promoted to terminal success only when the current managed execution protocol
proves the final result/exit.

This correction is mandatory even if a future launch mode successfully isolates
Command Governor's hooks from user/project hooks. Isolation improves determinism;
it does not justify weaker semantics.

## Current Claude events and structured signals to use

The implementation must re-verify the live Claude Code schema at the pinned
implementation commit. As of the 2026-08-31 research, relevant documented hook
events/signals include:

- `UserPromptSubmit`
- `PreToolUse`
- `PermissionRequest`
- `PermissionDenied`
- `PostToolUse`
- `PostToolUseFailure`
- `PostToolBatch`
- `Notification`
- `SubagentStart` / `SubagentStop`
- `TaskCreated` / `TaskCompleted`
- `TeammateIdle`
- `Stop`
- `StopFailure`
- `SessionStart` / `SessionEnd`
- `Elicitation` / `ElicitationResult`
- the non-interactive `system/init` and final `result` stream records.

Primary sources:

- <https://code.claude.com/docs/en/hooks>
- <https://code.claude.com/docs/en/cli-reference>
- <https://code.claude.com/docs/en/headless>

### Authority classification

| Signal | V1 use | Authority |
| --- | --- | --- |
| final `claude -p` structured `result` + matching command/process exit | successful managed turn completion/result | authoritative terminal success for fenced managed turn |
| non-zero managed command exit + structured failure/result | managed turn/process failure | authoritative terminal process outcome, interpreted with native failure evidence |
| `Stop` hook invocation | Claude proposed ending a response | **candidate only**; another parallel Stop hook can block and continue |
| `StopFailure` | turn ended because of Claude API error | strong native failure evidence; reconcile with managed command exit/result |
| `SessionEnd` | Claude session/process termination | strong non-blockable session-end evidence, not proof of successful result |
| `PreToolUse` `defer` for a fenced tool call | durable non-interactive pause/input boundary | authoritative when our hook returned the documented defer decision and later structured result confirms deferred stop |
| `PermissionRequest` | Claude reached a permission decision point | authoritative observation that permission was requested; not by itself a durable pause if another hook/policy may answer it |
| `PermissionDenied` | auto-mode classifier denied a tool call | authoritative denial observation for that call; not necessarily terminal |
| `UserPromptSubmit` / structured resumed-turn start | new/continued turn accepted | authoritative start evidence when correlation matches |
| `PostToolUse` / `PostToolBatch` | progress heartbeat | bounded liveness evidence only |
| `PostToolUseFailure` | progress/error evidence | not terminal unless the current managed protocol terminates afterward |
| `Notification.idle_prompt` | interactive Claude says it is waiting | corroborating idle evidence, not V1 completion authority |
| `Notification.agent_needs_input` / `agent_completed` | background-agent observation | corroborating only; current docs limit these to agent view being open |
| Herdr `working`, PTY idle/repaint | runtime transport observation | lower authority than fenced native/structured evidence |

Do not use event-name intuition as precedence. The adapter defines an explicit
accepted-evidence table and conformance tests for the exact Claude version/capabilities.

## Preferred Claude V1 execution mode

Prefer managed non-interactive turns through Claude Code's programmatic interface:

```text
claude -p ... --output-format stream-json --verbose
```

and enable only the additional structured stream features needed by the adapter.
Reasons:

- the stream has an explicit final `result` record rather than requiring PTY text
  inference;
- `system/init` exposes session metadata and a feature-detectable `capabilities`
  array in current Claude releases;
- current `AskUserQuestion` can be deferred from `PreToolUse` in non-interactive
  mode and resumed later;
- `claude -p` has documented success/failure exit behavior;
- final result can be captured explicitly and durably;
- process lifetime and Claude logical session are separate, matching Command
  Governor's session-incarnation/turn model.

Do not compare only Claude version strings when the stream exposes a capability;
feature-detect capabilities and reject an unsupported execution contract.

Herdr may still host/own the process/session layer. Command Governor consumes the
Claude structured protocol as semantic evidence rather than trusting a PTY status
label.

Interactive Claude remains a later/fallback adapter mode and must define a
separate evidence contract. It cannot inherit the `claude -p` completion rule by
analogy.

## Configuration isolation

Command Governor must not edit the user's personal `~/.claude/settings.json`.
Managed turns use a Command Governor-owned settings file passed with current
supported CLI settings controls.

Current Claude also exposes:

- `--setting-sources` to select user/project/local settings sources;
- `--settings` for an explicit settings JSON/file;
- `--bare`, which skips auto-discovered hooks/customizations but also deliberately
  does not use the user's subscription OAuth/system keychain; and
- managed-only `allowManagedHooksOnly`, unavailable as an ordinary application
  setting.

Therefore V1 must **not assume** that `--settings` alone removes user/project/plugin
hooks. Current docs say matching hooks can coexist and run in parallel. The live
Claude conformance gate must prove the exact invocation's active settings sources
and hook set. If safe isolation without changing the user's authentication model
cannot be proven, Command Governor continues to treat Stop as a candidate and
relies on structured final result/process evidence for completion.

The Command Governor settings file:

- lives under the private Command Governor state root;
- is a regular owner-owned file, not a symlink;
- is not group/world writable;
- references a stable packaged command such as `command-governor hook claude ...`;
- is validated before every managed spawn/adoption that relies on it;
- is versioned with a hook-contract epoch.

A stable installed command path is preferable to embedding a transient source or
build-output path.

## Durable worker-host/spool boundary

The structured programmatic result must survive the **Command Governor daemon**
restarting while Claude continues. Relying only on the daemon's live stdout reader
would recreate the original lost-completion bug.

V1 should therefore introduce a small Rust worker-host mode, part of the same
`command-governor` binary/runtime adapter rather than a second orchestration
daemon:

```text
Herdr/session runtime
  -> command-governor worker-host claude <opaque-turn-id>
       -> launches/resumes `claude -p`
       -> captures the structured stream in an owner-private bounded spool
       -> writes a sanitized exit receipt / lifecycle inbox record
       -> exits
```

The worker-host owns **no task/obligation truth**. It is a crash-surviving transport
shim. Its sensitive spool can contain provider protocol/result content and is
therefore separate from SQLite/general logs. The daemon later extracts only the
bounded final worker result into the immutable result-artifact store and deletes
or retains the transport spool according to policy.

This gives Command Governor an evidence source that can survive daemon restart
without requiring Herdr's stale `working` flag to become semantic truth.

The exact spool format/size ceiling is an implementation detail, but it must:

- be owner-private;
- be allocated by Command Governor, never a worker-provided path;
- be append-only/atomic enough to distinguish a complete final stream record from
  a truncated crash;
- persist a separate sanitized exit receipt only after child-process completion;
- never be included in routine diagnostics;
- be integrity-checked before result extraction.

## Hook durability when the daemon is down

Hooks remain valuable for input/progress/native observations that happen before
the final process result. A hook that only POSTs to the daemon can lose those
signals while Command Governor is restarting.

Therefore managed hooks first deposit a **sanitized durable envelope** into a
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
2. validate event class and Command Governor environment fences;
3. extract only allowed identifiers/safe fields;
4. derive/dedupe source identity without hashing secret payload text;
5. write owner-only temp envelope;
6. fsync file;
7. atomic rename into inbox;
8. sync directory as required;
9. emit only the exact hook-control JSON required by Claude, if this hook has a
   control decision;
10. exit according to the Claude hook contract.

Daemon ingestion inserts/deduplicates the sanitized event in SQLite and only then
removes/archives the inbox file. A crash after DB commit but before cleanup is
idempotent through source-event uniqueness.

The hook inbox contains **lifecycle envelopes, not transcripts or final result
text**. Sensitive structured provider output belongs only in the worker-host
transport spool/result-artifact boundary.

## Hook identity environment

Every managed turn receives non-secret Command Governor correlation values through
environment inherited by hooks/worker-host, for example:

```text
COMMAND_GOVERNOR_SESSION_ID
COMMAND_GOVERNOR_SESSION_INCARNATION_ID
COMMAND_GOVERNOR_TURN_ID
COMMAND_GOVERNOR_HOOK_EPOCH
COMMAND_GOVERNOR_STATE_ROOT
```

These are opaque IDs, not cwd or prompt material. The adapter records the native
Claude session ID as a safe external identity when exposed by the structured
protocol/hook schema.

If an event cannot prove it belongs to the current session incarnation/turn, it is
quarantined as an orphan observation and cannot mutate the active projection.

## Forbidden hook/ledger persistence

Hook code and the safe event ledger must never persist or log raw:

- prompt text;
- tool arguments;
- shell commands;
- cwd;
- transcript path;
- terminal transcript;
- environment secrets;
- browser/GitHub credentials;
- full Claude structured stream/provider result bodies.

For progress, persist only event class, fenced identity, source identity, and time.
The sensitive worker-host spool/result artifact has a separate explicit privacy and
retention boundary.

For a blocking input request, the database stores a safe opaque input identity and
classification, **not raw tool arguments**. Current question/permission detail is
obtained ephemerally from the native session/protocol when the foreman claims it.
If detail cannot be recovered after restart, the durable `needs_input` obligation
remains open with `input_detail_unavailable`; the system does not invent an
answer.

## `AskUserQuestion` defer/resume

Current Claude Code documentation supports a `defer` decision from `PreToolUse`.
In non-interactive mode a deferred tool call ends the current `-p` run and remains
pending for a later resume. The current docs use `AskUserQuestion` as the key
example.

V1 flow:

```text
Claude calls AskUserQuestion
  -> Command Governor PreToolUse hook recognizes the fenced tool call
  -> hook durably deposits safe needs_input envelope
  -> hook returns current documented DEFER decision
  -> final structured `result` confirms the managed run stopped/deferred
  -> obligation = needs_input
  -> browser wakes exact foreman
  -> foreman_resume claims input obligation
  -> adapter retrieves current question ephemerally from native session/provider
  -> foreman_answer_input records structured answer if authorized
  -> worker resume delivery is claimed/fenced
  -> same Claude session is resumed with current supported answer mechanism
  -> structured resumed-turn/session evidence arrives
  -> obligation returns to running
```

Do not project `needs_input` solely because the PreToolUse hook *attempted* to
return defer; reconcile the resulting structured run state so a malformed/ignored
hook response cannot create false lifecycle truth.

Current docs note that when several tool calls are emitted together, already-run
siblings are not undone and pending-call deferral has limitations. The adapter
must detect unsupported multi-call shapes and preserve a reconciliation condition
rather than silently claiming a clean pause.

## Permission requests

`PermissionRequest` means Claude reached a permission decision point; its command
hook can allow or deny, but current docs do not make it a generic durable
"pause-and-resume-later" primitive.

V1 therefore prefers to establish the durable boundary **before** an out-of-band
permission decision: a `PreToolUse` policy hook classifies the fenced tool call
against Command Governor's recorded delegation. If the action requires foreman or
user-owned authorization, return the documented non-interactive defer decision and
create/reconcile `needs_input` from the resulting structured state.

`PermissionRequest` remains useful as:

- corroborating evidence that Claude's own permission layer requires a decision;
- a fail-closed signal when the pre-tool policy missed a call;
- a place to deny/allow according to already-recorded policy when current
  conformance proves the outcome deterministic.

Current `--permission-prompt-tool` is also a candidate integration point, but it
must not become V1 lifecycle authority until live testing proves what happens
across daemon/MCP disconnects and whether a pending permission survives safely.

Every permission request/action is classified:

- within already delegated ordinary engineering scope;
- materially broader/destructive/credential/security-sensitive and user-owned;
- unknown, therefore user-owned by default.

No worker-generated request widens authority. `foreman_answer_input` fails with
`user_authorization_required` outside recorded delegation.

## Other current input/blocking signals

Current Claude Code also documents:

- `Elicitation` / `ElicitationResult` for MCP-server forms/input;
- `Notification.permission_prompt`;
- `Notification.agent_needs_input` for background sessions (currently only while
  agent view is open);
- `PermissionDenied` for auto-mode classifier denials.

These are normalized only after adapter-specific semantics are proven. In
particular, notifications that depend on a UI/view being open cannot be the sole
source of durable truth.

## Progress heartbeat

`PostToolUse`/`PostToolBatch` and structured programmatic progress are natural
verified progress sources, but persist only bounded safe metadata:

```text
turn_id
source_event_id
event_class = "tool_progress"
occurred_at
```

No tool name/arguments/result is required by the watchdog. High-rate equivalent
progress may be deterministically coalesced while retaining enough timestamps to
prove the stall threshold.

## Watchdog

Watchdog input is the last **verified** progress/native/structured lifecycle
instant for a running turn. Screen repaint does not reset it.

If:

```text
now - last_verified_progress_at >= configured_threshold
```

and no verified terminal/input boundary exists, create one `suspected_stall`
health/attention record for the current turn generation.

A later verified progress event resolves that attention. It does not create a new
worker, auto-interrupt, or change the turn to failed/completed.

No monitor-only Claude session is opened.

## Stale Herdr `working` conflict

The reproduced failure class remains a required fixture:

```text
Claude structured/native state: run ended/deferred or needs input
Herdr observation: working / idle=false
```

Command Governor behavior:

1. accept the stronger fenced structured/native evidence;
2. project the corresponding terminal/input state;
3. record `runtime_state_conflict` because Herdr disagrees;
4. never reject the foreman's needed processing solely because Herdr says
   `working`;
5. before writing/resuming the worker, reconcile the process transport;
6. if necessary, issue one Command-Governor-authored runtime interrupt/clear
   operation to remove a stale busy condition;
7. verify transport safety before one fenced continuation;
8. if inconsistency remains, preserve the obligation and expose reconciliation
   failure instead of opening a duplicate worker.

A lone `Stop` hook candidate is **not** sufficient for step 1; use the structured
final/deferred/process evidence defined above.

## Interrupt

An interrupt has separate facts:

- Command Governor requested runtime interruption;
- the worker-host/runtime accepted or ambiguously accepted that external write;
- Claude structured/native lifecycle reported the consequence;
- the child process/session ended or continued.

The first never fabricates the later facts. Runtime interrupt delivery gets the
same accepted/failed/ambiguous fencing required of other consequential writes.

An interrupted turn may become `needs_input`, failed, or another current state. It
is not automatically task cancellation.

## Close

Closing a runtime session cannot delete:

- lifecycle events;
- result artifacts;
- input obligations;
- completed/failed unprocessed obligations;
- browser deliveries;
- foreman claims/history.

Before close, the daemon records why the session is being closed and whether the
current turn has a durable confirmed terminal/input state. Closing over unresolved
work creates reconciliation attention, not success.

## Result capture

For managed `claude -p`, the final structured `result` record is captured through
the private worker-host spool. The daemon validates the matching session/turn and
child exit receipt, extracts the bounded final response, and then commits it
through the result-artifact sequence in [data-model.md](data-model.md).

Only after the result artifact is durable and the terminal event/obligation
transaction commits may `completed_unprocessed` become visible to wake scheduling.

A `Stop` hook candidate can help explain how Claude approached the boundary but
cannot publish completion by itself.

"Final result" is not the whole stream/transcript. It is the bounded final worker
response plus stable engineering refs (commit/PR/etc.) needed for review. Large
source/diff evidence remains in GitHub.

## Failure capture

`StopFailure`, non-zero managed process outcome, truncated/no-final-result spool,
and explicit runtime termination are separate evidence classes. The adapter
normalizes them deterministically; it does not turn every non-zero/process loss
into the same failure cause.

Raw provider error bodies are not automatically persisted in the safe ledger. A
bounded sensitive diagnostic may live in the private worker transport/result
boundary when genuinely needed for foreman review.

## Session re-adoption and incarnation

After daemon/runtime restart, an existing native Claude session may be re-adopted
only when continuity can be proven from stable native/runtime identities and the
stored session fence. Otherwise create a new `session_incarnation_id`.

A delayed old-incarnation hook/spool receipt may be ingested for history but cannot
mutate the current incarnation unless its fences match.

## Deterministic fake worker

The testkit must simulate, in controlled order:

- structured `system/init` / capabilities;
- progress;
- Stop candidate allowed;
- Stop candidate blocked by another parallel hook, followed by more work and a
  second Stop candidate;
- final structured result + exit;
- truncated stream / process loss;
- StopFailure;
- AskUserQuestion defer attempt + confirmed deferred result;
- PermissionRequest / PermissionDenied;
- resumed turn;
- duplicate terminal result/event;
- late old-incarnation event;
- runtime `working` disagreement;
- daemon-offline hook inbox deposit;
- daemon-offline worker-host final-result/exit spool.

All core lifecycle correctness tests run against this fake without real Claude.
Real Claude is an adapter conformance suite, not the only place state-machine bugs
can be found.
