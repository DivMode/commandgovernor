# Worker lifecycle, input, and watchdog contract

V1 starts with Claude Code as the primary implementation worker while keeping the
control-plane state machine worker-neutral. Herdr (or another runtime) owns
process/session mechanics; it does not define semantic completion.

## Boundary

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

## A Claude `Stop` hook firing is not final completion

Current Claude Code documentation says:

- all matching hooks run in parallel;
- a `Stop` hook can return `decision: "block"` and make Claude continue; and
- `stop_hook_active` exists to prevent continuation loops.

Therefore Command Governor must **not** equate "our Stop hook was invoked" with
"the managed turn definitely ended." Another matching Stop hook may veto the stop.

For the preferred non-interactive V1 mode, successful turn completion is confirmed
from Claude's programmatic interface: the final structured `result` from
`claude -p --output-format stream-json` (or its current equivalent) plus the
matching child command/process exit receipt.

`SessionEnd` is useful session-termination evidence but does not itself prove a
successful result. `StopFailure` is strong failure evidence. A `Stop` callback is
retained as bounded `stop_candidate` evidence and is promoted to terminal success
only when the managed execution protocol proves the final result/exit.

## Current Claude events / structured signals

Re-verify the live Claude Code schema at the implementation commit. As of the
2026-08-31 review, relevant documented events/signals include:

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
- non-interactive `system/init` and final `result` stream records.

Primary sources:

- <https://code.claude.com/docs/en/hooks>
- <https://code.claude.com/docs/en/hooks-guide>
- <https://code.claude.com/docs/en/cli-usage>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/settings>

### Authority classification

| Signal | V1 use | Authority |
| --- | --- | --- |
| final `claude -p` structured `result` + matching child exit | successful managed turn/result | authoritative terminal success for fenced managed turn |
| non-zero managed command exit + structured failure/result | managed turn/process failure | authoritative process outcome, interpreted with provider evidence |
| `Stop` callback | Claude proposed ending a response | **candidate only**; another parallel Stop hook can block and continue |
| `StopFailure` | turn ended because of Claude API error | strong native failure evidence; reconcile with command exit/result |
| `SessionEnd` | Claude session termination | strong session-end evidence, not successful-result proof |
| `PreToolUse` `defer` + confirmed structured run end | durable non-interactive input boundary | authoritative after the provider confirms the deferred/pending stop |
| `PermissionRequest` | interactive permission dialog evidence | **not available in preferred `-p` mode**; do not depend on it for managed V1 |
| `PermissionDenied` | auto-mode classifier denial | denial observation; not necessarily terminal |
| `UserPromptSubmit` / structured resumed-turn start | continued/new turn accepted | authoritative start evidence when fenced correlation matches |
| `PostToolUse` / `PostToolBatch` | progress heartbeat | bounded liveness evidence only |
| `PostToolUseFailure` | progress/error evidence | not terminal unless the managed protocol subsequently terminates |
| `Notification.idle_prompt` | interactive waiting evidence | corroborating only |
| `Notification.agent_needs_input` / `agent_completed` | background-agent observation | corroborating only and view-dependent in current docs |
| Herdr `working`, PTY idle/repaint | runtime transport observation | lower authority than fenced structured/native evidence |

Do not use event-name intuition as precedence. The adapter has an explicit
accepted-evidence table and conformance tests for the exact Claude release.

## Preferred Claude V1 execution mode

Prefer managed non-interactive turns through Claude Code's programmatic interface:

```text
claude -p ... --output-format stream-json --verbose
```

Reasons:

- explicit structured final result rather than PTY text inference;
- `system/init` exposes session metadata/capabilities in current releases;
- current `PreToolUse` can defer a tool call in non-interactive mode;
- documented print-mode success/failure exit behavior;
- bounded final result can be captured explicitly;
- process lifetime and Claude logical session can be fenced separately.

Feature-detect capabilities where available rather than relying only on Claude
version strings.

Herdr may host the process/session layer, but Command Governor consumes Claude's
structured protocol as semantic evidence rather than treating PTY status as the
semantic API.

Interactive Claude is a separate/future adapter mode. It must define its own
evidence contract and may use interactive-only events such as `PermissionRequest`;
it cannot inherit `-p` semantics by analogy.

## Configuration isolation

Command Governor never edits personal `~/.claude/settings.json`. Managed turns
use a private Command-Governor-owned settings file through current supported CLI
settings controls.

Current Claude exposes controls including `--setting-sources` and `--settings`,
while `--bare` has different authentication/customization tradeoffs. Current
settings/hooks can merge across scopes, so `--settings` alone is **not assumed**
to remove user/project/plugin hooks.

The live Claude conformance gate must prove the exact invocation's active settings
sources/hook behavior. Current Claude's `/status` can show active settings sources
for an interactive inspection, but the automation contract must be testable from
our actual managed invocation/environment rather than relying on a human UI step.

The Command Governor settings file:

- lives under the private state root;
- is a regular owner-owned file, not a symlink;
- is not group/world writable;
- references a stable packaged `command-governor hook claude ...` command;
- is validated before each managed spawn/adoption that relies on it;
- has a hook-contract epoch.

## Durable worker-host / spool boundary

The structured programmatic result must survive the **authoritative daemon**
restarting while Claude continues. Relying only on the daemon's stdout reader would
recreate the original lost-completion failure.

V1 therefore uses a small Rust worker-host mode in the same product/runtime:

```text
Herdr/session runtime
  -> command-governor worker-host claude <opaque-turn-id>
       -> launches/resumes `claude -p`
       -> captures structured stream in owner-private bounded spool
       -> writes sanitized child-exit receipt
       -> exits
```

The worker-host owns **no task/obligation truth**. It is a crash-surviving transport
shim. Its sensitive spool is separate from SQLite/general logs. The daemon later
validates the stream/exit fence, extracts only the bounded final response into the
immutable result-artifact store, and applies transport-spool retention.

The spool must:

- be owner-private;
- be allocated by Command Governor, never a worker-supplied path;
- distinguish a complete final structured record from truncation;
- persist a sanitized exit receipt only after child completion;
- be excluded from routine diagnostics;
- be integrity-checked before result extraction;
- have an explicit size ceiling and retention policy.

## Durable sanitized hook inbox

Hooks remain useful for progress, defer intent, starts/stops-as-candidates, and
other native observations that happen before final process settlement. A hook that
only sends live IPC can lose exactly those observations during daemon restart.

Managed hooks therefore first deposit a **sanitized durable envelope** to a private
hook inbox, then return to Claude:

```text
~/.command-governor/
  hook-inbox/          # owner private
    <event-id>.json    # atomic temp -> rename
```

Deposit sequence:

1. read hook JSON from stdin without logging it;
2. validate event class and Command Governor correlation fences;
3. extract only allowed IDs/safe fields;
4. derive/dedupe source identity from stable non-secret facts;
5. write owner-only temp envelope;
6. fsync as required;
7. atomic rename;
8. sync directory as required;
9. emit only exact Claude hook-control JSON when this hook has a decision;
10. exit under Claude's documented hook contract.

Daemon ingestion deduplicates the event transactionally and only then removes or
archives the inbox file. The inbox contains lifecycle envelopes, **not transcript
or structured provider-result content**.

## Hook correlation

Managed turns pass only non-secret opaque Governor IDs needed for correlation, for
example:

```text
COMMAND_GOVERNOR_SESSION_ID
COMMAND_GOVERNOR_SESSION_INCARNATION_ID
COMMAND_GOVERNOR_TURN_ID
COMMAND_GOVERNOR_HOOK_EPOCH
COMMAND_GOVERNOR_STATE_ROOT
```

When Claude exposes a native session ID through its structured protocol/hook
schema, record it as a safe external identity. An event that cannot prove it
belongs to the current incarnation/turn is quarantined for history/reconciliation
and cannot mutate the current turn.

## Forbidden safe persistence

The hook inbox, SQLite event ledger, safe logs and diagnostics must never persist:

- prompt text;
- raw tool arguments;
- shell commands;
- cwd;
- transcript path;
- terminal transcript;
- environment secrets;
- browser/GitHub credentials;
- complete Claude structured stream/provider response bodies.

Progress persists only identity/time/safe class. Sensitive provider stream/result
content belongs only in the explicit worker-host/result-artifact privacy boundary.

For blocking input, SQLite stores safe opaque input identity/classification, not
raw tool arguments. Current question detail is obtained ephemerally from the native
session/protocol where available. If it cannot be recovered, keep the durable
`needs_input` obligation open with `input_detail_unavailable`; do not invent an
answer.

## `AskUserQuestion` / non-interactive defer

Current Claude docs support `defer` from `PreToolUse` in non-interactive `-p`; the
process can exit with the tool call preserved for later resume.

V1 flow:

```text
Claude calls AskUserQuestion
  -> CG PreToolUse hook identifies exact fenced tool call
  -> sanitized defer intent is durable
  -> hook returns current documented DEFER decision
  -> structured managed-run result confirms the call is pending/deferred
  -> obligation = needs_input
  -> exact ChatGPT foreman wakes and claims it
  -> current question is obtained ephemerally from native session/provider
  -> foreman_answer_input records authorized structured answer
  -> worker continuation delivery is claimed/fenced
  -> same Claude session resumes via current supported mechanism
  -> structured/native resumed-turn evidence arrives
  -> obligation returns to running
```

Do not project clean `needs_input` merely because the hook attempted defer. If the
provider ignores/misparses it or a multi-tool shape cannot be safely preserved,
create reconciliation attention instead.

## Permission handling in preferred `-p` mode

Current Claude hook guidance explicitly says **`PermissionRequest` hooks do not
fire in non-interactive mode (`-p`)** and directs automated permission decisions to
`PreToolUse`.

Therefore the preferred managed V1 path is:

1. classify the exact fenced tool call in `PreToolUse` before permission-mode
   execution;
2. for already delegated low-risk engineering work, return only the current
   provider-supported decision consistent with user/managed deny rules;
3. for a decision that must leave the worker (foreman/user), use confirmed
   non-interactive `defer` where the current tool/provider shape supports it;
4. for destructive, credential-sensitive, materially broader, or unknown actions,
   preserve user-owned authorization and fail closed.

Current docs also state that a PreToolUse allow cannot override deny/ask rules from
settings; hooks can tighten restrictions but cannot necessarily loosen them. The
adapter must model that explicitly.

`PermissionRequest` remains relevant only for an interactive/future adapter mode
or another provider mode where conformance proves it fires. It is **not** a source
of truth in the preferred `claude -p` V1 contract.

Current `--permission-prompt-tool` may be investigated, but it does not become V1
lifecycle authority until live testing proves disconnect/restart/pending-decision
semantics.

## Other current input signals

Claude also documents `Elicitation` / `ElicitationResult`,
`Notification.permission_prompt`, background-agent notifications, and
`PermissionDenied`. Normalize them only after adapter-specific semantics are
proven. View-dependent notifications cannot be the sole durable truth source.

## Progress heartbeat and watchdog

Use structured/native tool activity such as `PostToolUse`/`PostToolBatch` or the
managed programmatic stream to update only bounded safe progress metadata:

```text
turn_id
source_event_id
safe_event_class
occurred_at
```

No tool name/arguments/result is required for the watchdog. High-rate equivalent
progress may be deterministically coalesced.

If a running turn has no verified progress for the configured threshold and no
confirmed terminal/input boundary, create `suspected_stall`. A later verified
progress event resolves it. The watchdog never fabricates Stop/failure/completion,
never auto-interrupts by itself, and never opens a monitor-only Claude session.

## Stale Herdr `working` conflict

Required reproduced class:

```text
Claude structured/native state: confirmed final run or confirmed deferred input
Herdr observation: working / idle=false
```

Behavior:

1. project the stronger confirmed worker state;
2. record `runtime_state_conflict`;
3. never reject foreman processing solely because Herdr says `working`;
4. before continuation, reconcile process transport;
5. if needed, issue one fenced Command-Governor-authored interrupt/clear operation;
6. verify transport safety before one continuation;
7. if still inconsistent, preserve the obligation and expose reconciliation
   failure instead of creating a duplicate worker.

A Stop candidate alone is not sufficient for step 1.

## Interrupt / close

An interrupt has distinct facts: intent, external-delivery outcome, worker
structured/native consequence, and child/session consequence. The first never
fabricates the later facts. Interrupt delivery gets accepted/failed/ambiguous
fencing.

Closing a runtime session cannot delete lifecycle events, result artifacts, input
obligations, unprocessed results/failures, browser deliveries, or foreman claims.
Closing over unresolved work creates reconciliation attention, not success.

## Result capture

For managed `claude -p`, the worker-host private spool contains the provider
structured stream. The daemon validates the exact session/turn and child exit,
extracts the bounded final response, and commits it through the immutable
result-artifact sequence in [data-model.md](data-model.md).

Only after artifact durability and the terminal event/obligation transaction may
`completed_unprocessed` become visible to wake scheduling.

A Stop candidate cannot publish completion by itself. The final result artifact is
not the entire stream/transcript; large code/diff evidence remains in GitHub.

## Failure capture

`StopFailure`, non-zero child outcome, truncated/no-final-result spool, explicit
interrupt, and transport loss are separate evidence classes. The adapter maps them
deterministically and does not persist raw provider error bodies into the safe
ledger.

## Session re-adoption / incarnation

After restart, re-adopt a Claude session only when continuity is proven from stable
native/runtime identities and stored fences. Otherwise create a new incarnation.
Delayed old-incarnation hook/spool receipts can be retained for history but cannot
mutate current work.

## Deterministic fake worker requirements

The testkit must simulate:

- structured `system/init` / capabilities;
- normal progress;
- Stop candidate allowed;
- Stop candidate blocked by another parallel hook followed by continued work;
- later final structured result + exit;
- truncated stream / missing exit;
- StopFailure / SessionEnd;
- AskUserQuestion defer intent + confirmed deferred result;
- PreToolUse permission policy/defer/deny;
- resumed turn;
- duplicate terminal event;
- late old-incarnation event;
- stale runtime `working` disagreement;
- daemon-offline sanitized hook inbox;
- daemon-offline worker-host final-result/exit spool.

Core lifecycle correctness is proven against these fakes. Real Claude is a
provider-adapter conformance gate, not the only place lifecycle bugs can be found.
