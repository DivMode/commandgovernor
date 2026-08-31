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
        │     ├── structured `claude -p` online parser
        │     ├── managed lifecycle hooks
        │     ├── native session/turn correlation
        │     └── bounded final-result capture
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
| `PreToolUse` `defer` + confirmed `tool_deferred` result | durable non-interactive single-tool input boundary | authoritative only after exact single-tool fence and provider confirmation |
| `PermissionRequest` | non-interactive/interactive permission decision signal | current docs say it can fire when no prompt UI exists; weaker exact tool correlation than PreToolUse because current input lacks `tool_use_id` |
| `PermissionDenied` | permission denial observation | denial evidence; not necessarily terminal |
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
- current `PreToolUse` can defer one tool call in non-interactive mode;
- documented print-mode success/failure exit behavior;
- bounded final result can be captured explicitly;
- process lifetime and Claude logical session can be fenced separately.

The worker-host consumes the structured stream online. Current stream-json can
contain intermediate tool-use/tool-result records, so those records are **never
written to a durable raw spool**. Only allowlisted sanitized run receipts and the
bounded final assistant result candidate may be persisted.

Feature-detect capabilities where available rather than relying only on Claude
version strings.

Herdr may host the process/session layer, but Command Governor consumes Claude's
structured protocol as semantic evidence rather than treating PTY status as the
semantic API.

Interactive Claude is a separate/future adapter mode. It must define its own
evidence contract; it cannot inherit `-p` semantics by analogy.

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

## Durable worker-host / managed-run staging boundary

The final programmatic result must survive the **authoritative daemon** restarting
while Claude continues. Relying only on the daemon's stdout reader would recreate
the original lost-completion failure.

V1 therefore uses a small Rust worker-host mode in the same product/runtime:

```text
Herdr/session runtime
  -> command-governor worker-host claude <opaque-turn-id>
       -> launches/resumes `claude -p`
       -> parses structured stdout online
       -> writes allowlisted sanitized managed-run receipts
       -> writes one bounded final-result candidate if/when complete
       -> writes sanitized child-exit receipt
       -> exits
```

The worker-host owns **no task/obligation truth**. It is a crash-surviving transport
shim. The daemon later validates the final-result/run/exit fences, promotes the
bounded final response through the immutable result-artifact store, and applies
staging retention.

There is intentionally **no complete provider-stream spool**. Intermediate
stream-json records—including tool_use/tool_result blocks—are processed in memory
and discarded after required safe evidence is extracted.

Managed-run staging must:

- be owner-private;
- be allocated by Command Governor, never a worker-supplied path;
- distinguish a complete final structured result from truncation;
- persist only allowlisted run/exit metadata plus the bounded final-result candidate;
- exclude raw prompt text, tool args/results, commands, cwd, transcript paths,
  terminal transcript, arbitrary environment data, and generic provider JSON;
- be excluded from routine diagnostics;
- be integrity-checked before result promotion;
- have explicit size ceilings and retention policy.

## Durable sanitized hook inbox

Hooks remain useful for progress, defer intent, starts/stops-as-candidates,
permission decisions, and other native observations that happen before final
process settlement. A hook that only sends live IPC can lose exactly those
observations during daemon restart.

Managed hooks therefore first deposit a **sanitized durable envelope** to a private
per-turn hook inbox location, then return to Claude.

Deposit sequence:

1. read hook JSON from stdin without logging it;
2. validate event class and Command Governor correlation fences;
3. extract only allowed IDs/safe fields;
4. derive/dedupe source identity from stable non-secret facts;
5. write owner-only temp envelope to the allocated narrow inbox location;
6. fsync as required;
7. atomic rename;
8. sync directory as required;
9. emit only exact Claude hook-control JSON when this hook has a decision;
10. exit under Claude's documented hook contract.

Daemon ingestion deduplicates the event transactionally and only then removes or
archives the inbox file. The inbox contains lifecycle envelopes, **not transcript,
raw tool input, or provider-result content**.

## Hook correlation and minimum worker environment

Managed turns pass only opaque Governor IDs/locators needed for correlation, for
example:

```text
COMMAND_GOVERNOR_SESSION_ID
COMMAND_GOVERNOR_SESSION_INCARNATION_ID
COMMAND_GOVERNOR_TURN_ID
COMMAND_GOVERNOR_HOOK_EPOCH
COMMAND_GOVERNOR_HOOK_INBOX
```

`COMMAND_GOVERNOR_HOOK_INBOX` is a narrow per-turn location allocated for hook
deposits. The general Command Governor state-root path is not intentionally
exported to Claude.

When Claude exposes a native session ID through its structured protocol/hook
schema, record it as a safe external identity. An event that cannot prove it
belongs to the current incarnation/turn is quarantined for history/reconciliation
and cannot mutate the current turn.

This minimizes accidental exposure; it is not an OS sandbox against a malicious
process running as the same user.

## Forbidden durable persistence

Except for the explicitly bounded **final assistant result** needed for review,
Command Governor durable stores—including hook inbox, SQLite/WAL, safe logs,
diagnostics, managed-run receipts, and worker-host staging—must never persist:

- prompt text;
- raw tool arguments;
- raw tool results;
- shell commands;
- cwd;
- transcript path;
- terminal transcript;
- environment secrets;
- browser/GitHub credentials;
- complete Claude structured stream/provider response bodies.

Progress persists only identity/time/safe class. The final-result candidate and
immutable result artifact contain only the bounded final worker result required by
the central durable-review invariant.

For blocking input, SQLite stores safe opaque input identity/classification, not
raw tool arguments. Current question detail is obtained ephemerally from the native
session/protocol where available. If it cannot be recovered, keep the durable
`needs_input` obligation open with `input_detail_unavailable`; do not invent an
answer.

## `AskUserQuestion` / non-interactive defer

Current Claude docs support `defer` from `PreToolUse` in non-interactive `-p`; the
process can exit with the tool call preserved for later same-session resume.
Current docs also state that `defer` is **ignored if Claude emits several tool calls
at once**.

V1 clean-defer flow therefore requires an exact single-tool case:

```text
Claude calls AskUserQuestion as one tool call
  -> CG PreToolUse hook identifies exact fenced tool_use_id
  -> sanitized defer intent is durable
  -> hook returns current documented defer decision
  -> structured managed-run result confirms stop_reason/tool_deferred
  -> obligation = needs_input
  -> exact ChatGPT foreman wakes and claims it
  -> current question is obtained ephemerally from native session/provider
  -> foreman_answer_input records authorized structured answer
  -> worker continuation delivery is claimed/fenced
  -> same Claude session resumes via current supported `--resume` mechanism
  -> structured/native resumed-turn evidence arrives
  -> obligation returns to running
```

Do not project clean `needs_input` merely because the hook attempted defer. If the
provider ignores/misparses it or a multi-tool shape exists, create
reconciliation/manual attention instead.

The durable receipt for a deferred call stores only safe opaque identity/class,
not `deferred_tool_use.input` or the raw question/options.

## Permission handling in preferred `-p` mode

The independent review corrected the earlier draft: current Claude hook docs say
**`PermissionRequest` hooks can run in sessions that cannot show a prompt**, which
includes non-interactive/background contexts. If no hook decides in such a
context, the tool is denied.

Current `PermissionRequest` input carries `tool_name` and `tool_input` but lacks the
same exact `tool_use_id` correlation available to `PreToolUse`. Therefore V1 uses
these surfaces differently:

1. `PreToolUse` is the preferred exact tool-call policy/defer boundary;
2. `PermissionRequest` may make/record a bounded permission decision only under
   the pinned release's proven correlation/ordering contract;
3. a `PermissionRequest` event alone is not treated as proof of a durable,
   later-resumable pause;
4. already delegated low-risk engineering actions may proceed only as current
   Claude settings/permission rules permit;
5. destructive, credential-sensitive, materially broader, or unknown actions stay
   user-owned and fail closed.

Current docs also state permission hook outcomes interact with settings/rule
precedence; an allow is not treated as authority to override a stronger recorded
user/managed restriction. The adapter models that explicitly.

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

For managed `claude -p`, the worker-host parses provider structured output online.
When and only when a complete final `result` arrives, it writes the bounded final
assistant response candidate plus sanitized run/exit receipts. The daemon validates
the exact session/turn/result and child exit, then commits the candidate through
the immutable result-artifact sequence in [data-model.md](data-model.md).

Only after artifact durability and the terminal event/obligation transaction may
`completed_unprocessed` become visible to wake scheduling.

A Stop candidate cannot publish completion by itself. The final result artifact is
not the entire stream/transcript; large code/diff evidence remains in GitHub.

## Failure capture

`StopFailure`, non-zero child outcome, truncated/no-final-result run, explicit
interrupt, and transport loss are separate evidence classes. The adapter maps them
deterministically and does not persist raw provider error bodies into durable safe
state.

## Session re-adoption / incarnation

After restart, re-adopt a Claude session only when continuity is proven from stable
native/runtime identities and stored fences. Otherwise create a new incarnation.
Delayed old-incarnation hook/run receipts can be retained for history only if their
safe form passes the same redaction contract; they cannot mutate current work.

## Same-user trust boundary

The worker-host owns no orchestration API authority, but V1 does not claim hostile
same-user process containment. Claude and tools normally run as the same OS user as
Command Governor, so owner-only filesystem modes do not stop a deliberately
malicious same-user process from modifying files it can discover.

V1 therefore:

- minimizes exposed state paths/capabilities;
- does not export the general state root to Claude;
- validates all imported staging/inbox data as untrusted;
- treats repository/worker text as untrusted policy input;
- explicitly leaves hostile-worker OS containment to a future separate-user or
  sandbox/broker design.

## Deterministic fake worker requirements

The testkit must simulate:

- structured `system/init` / capabilities;
- normal progress;
- Stop candidate allowed;
- Stop candidate blocked by another parallel hook followed by continued work;
- later final structured result + exit;
- truncated final result / missing exit;
- StopFailure / SessionEnd;
- single-tool AskUserQuestion defer intent + confirmed `tool_deferred` result;
- multi-tool defer ignored/unsupported;
- non-interactive PermissionRequest decision behavior;
- PreToolUse permission policy/defer/deny;
- resumed turn;
- duplicate terminal event;
- late old-incarnation event;
- stale runtime `working` disagreement;
- daemon-offline sanitized hook inbox;
- daemon-offline final-result candidate + run/exit receipts;
- provider stream containing prompt/tool-use/tool-result sentinels that must never
  appear in durable staging/DB/logs.

Core lifecycle correctness is proven against these fakes. Real Claude is a
provider-adapter conformance gate, not the only place lifecycle bugs can be found.
