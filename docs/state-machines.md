# V1 state machines

Command Governor deliberately separates worker lifecycle, foreman obligations,
browser delivery, physical ChatGPT turns, and worker-continuation delivery. The
separation is the correctness feature.

## Global rules

- Every transition comes from an immutable accepted event.
- Provider status strings/hook callbacks are evidence, not domain states by name.
- Every transition is fenced by relevant identities/generations/versions.
- Duplicate source events are idempotent.
- Silence is never success.
- External effects that may have happened are never blindly replayed.
- Closing an obligation always requires an explicit terminal disposition event.
- A provider hook that can be vetoed by another hook cannot be treated as terminal
  until the managed execution protocol confirms the outcome.

## 1. Durable obligation

### States

- `created`
- `running`
- `needs_input`
- `failed`
- `completed_unprocessed`
- `claimed_by_foreman`
- `processing`
- `acknowledged`
- `cancelled_by_user`
- `superseded`

`suspected_stall` is an attention condition layered on `running`, never a terminal
obligation state.

### Primary flow

```text
created
  │ verified worker start
  ▼
running
  ├── confirmed durable input/defer boundary ─► needs_input
  │                                                │
  │             answer + fenced worker resume +   │
  │             verified resumed-turn evidence    │
  │◄───────────────────────────────────────────────┘
  │
  ├── verified terminal failure ──────────────► failed
  │
  └── confirmed final result + durable artifact
                                         ─────► completed_unprocessed
                                                   │
needs_input / failed / completed_unprocessed       │
  │ foreman_resume(current version/generation)     │
  ▼                                                │
claimed_by_foreman                                 │
  │ result/input handed to current foreman         │
  ▼                                                │
processing                                         │
  │ explicit fenced foreman_ack                    │
  ▼                                                │
acknowledged ◄─────────────────────────────────────┘
```

Worker failure is unprocessed work until the foreman explicitly dispositions it.

### Claim/ACK fencing

`foreman_resume` creates a bounded claim under the current binding generation.
Claim expiry is internal coordination and may return the obligation to its prior
attention state; it never closes work or releases a required result artifact.

Normal ACK requires:

```text
obligation_id
obligation_version
source_event_id / source fence
binding_generation
claim_id
terminal disposition
```

A stale value causes a typed conflict and zero state mutation.

## 2. Managed Claude worker lifecycle

Preferred V1 is structured non-interactive Claude execution.

### Evidence precedence

1. final structured programmatic `result` + matching worker-host child exit —
   terminal outcome for the exact managed run;
2. strong native evidence such as `StopFailure`, `SessionEnd`, or a **confirmed
   single-tool `PreToolUse` defer** — interpreted with the structured/process
   outcome;
3. `Stop` hook callback — `stop_candidate` only because another parallel Stop hook
   may block stopping;
4. `PermissionRequest` hook — permission-decision evidence; current non-interactive
   hooks can fire, but its input lacks the same exact `tool_use_id` correlation as
   `PreToolUse`, so it is not by itself a durable pause/resume identity;
5. Herdr/process-session observation — transport evidence;
6. PTY idle/repaint — fallback diagnostics only.

"Newest timestamp wins" is rejected. Evidence class + fence wins.

### Successful completion

A Stop callback can record `stop_candidate` but cannot publish
`completed_unprocessed`.

Successful completion requires a complete final structured result for the exact
managed run plus the matching child exit receipt. The worker-host parses provider
output online, persists the bounded final-result candidate and sanitized run/exit
receipts only, then the daemon makes the immutable result artifact durable and
atomically publishes the terminal event/projection and one processing obligation.

Missing/truncated final result, unknown exit, or artifact failure creates
reconciliation attention—not fake processable completion. Intermediate provider
stream records are not durably spooled.

### Failure

`StopFailure`, nonzero child exit, explicit interrupt, transport loss, malformed
provider output, and denied permission are distinct evidence classes. Only
documented accepted combinations project `failed`.

### Stop-veto race

Required case:

```text
CG Stop hook -> stop_candidate #1
other parallel Stop hook -> decision:block
Claude continues and emits progress
CG Stop hook -> stop_candidate #2
final structured result + matching child exit
```

No completion occurs until the final result/exit plus durable artifact.

## 3. Durable `needs_input`

Input is durable only when the provider/runtime is actually in a safely resumable
blocked state—not merely because a hook observed or attempted a decision.

### Input classes

- `engineering_question`
- `user_owned_decision`
- `runtime_input`
- `provider_elicitation` when exact resumability is proven

For preferred managed `claude -p`, exact durable out-of-band pause/resume uses a
confirmed **single-tool** `PreToolUse` defer when supported. Current Claude docs
state that `defer` is ignored when multiple tool calls are emitted together, so a
multi-tool shape cannot become clean `needs_input`.

The SQLite ledger stores safe opaque identity/classification and answer shape, not
raw Claude tool arguments/question payloads.

### AskUserQuestion / policy defer

Preferred sequence:

1. `PreToolUse` identifies exact fenced tool call;
2. verify the current event represents a single tool call eligible for defer;
3. Command Governor records safe defer intent and returns current documented
   non-interactive `defer` decision;
4. structured managed-run outcome proves `tool_deferred`/equivalent and the call
   remains pending;
5. only then project `needs_input`.

If the defer response is ignored/malformed, or a multi-tool shape is present,
create reconciliation/manual attention rather than a clean input state.

### Permission policy

Current Claude documentation says `PermissionRequest` hooks can run in
non-interactive sessions that cannot display a prompt. If no hook decides, the
permission is denied. V1 therefore does not discard this signal.

However, current `PermissionRequest` input contains tool name/input without the
same `tool_use_id` correlation available to `PreToolUse`. Therefore:

- `PreToolUse` is the preferred exact tool-call policy/defer boundary;
- `PermissionRequest` may produce a permission decision under pinned-release
  conformance, but cannot by itself claim a durable resumable pause identity;
- already delegated ordinary engineering action may proceed only as Claude's
  current settings/permission semantics allow;
- destructive, credential-sensitive, materially broader, or unknown action is
  user-owned and fails closed.

A hook allow is not assumed to override deny/ask settings.

### Answer/resume

`foreman_answer_input` verifies current claim/version/generation/input identity and
authorization policy, records only the structured answer, and creates a separate
worker-command delivery.

Answer recorded != worker received. Worker delivery acceptance also does not
restore `running` until matching structured/native resumed-turn evidence arrives.

## 4. Browser wake delivery

### Identity

V1 separates deterministic dedupe identity from the correlation token carried in
the wake:

```text
delivery_key = H(
  "command-governor/wake-key/v1",
  obligation_id,
  binding_generation,
  delivery_revision
)

delivery_id = CSPRNG(>=192 bits)
```

`delivery_key` is a non-secret idempotency key. `delivery_id` is generated once and
persisted with the durable delivery, appears in the accepted browser wake, is not
returned by bootstrap/status, and is required by `foreman_resume` as an
anti-confusion possession fence. Knowing the deterministic inputs must not reveal
`delivery_id`.

The delivery snapshots target obligation version/source event. If the target
changes before Send, the wake is stale and cannot submit.

### Projection / attempt states

Delivery projection:

- `pending`
- `claimed`
- `accepted`
- `failed`
- `ambiguous`

Attempt lifecycle:

```text
pending
  │ transaction before any browser I/O
  ▼
claimed
  ├── definite pre-submit error ─────────► failed
  │                                          │
  │            bounded safe retry may       │ create next attempt
  │◄─────────────────────────────────────────┘
  │
  │ revalidate target/binding/app/composer
  ▼
activation_armed
  ├── exact semantic submit evidence ───► accepted
  ├── definite proof no submit ─────────► failed
  └── uncertain/lost evidence ──────────► ambiguous
```

`activation_armed` is committed immediately before exact Send activation.

### Restart

Any previous-process attempt left `claimed`/`activation_armed` without terminal
outcome becomes `ambiguous` **before browser recovery**. This can conservatively
quarantine a zero-send crash; duplicate avoidance wins over guessing.

### Accepted evidence

Require semantic evidence binding the intended wake to the exact conversation and
provider user-message identity where available. These are insufficient alone:

- composer emptied;
- URL changed;
- Stop button appeared;
- assistant started;
- wake text appears in DOM.

### Ambiguous reconciliation

Automatic reconciliation may only promote `ambiguous -> accepted` with exact
already-submitted message evidence. Absence is not proof of no submission.
Accepted/ambiguous is frozen and never automatically resent.

A later bounded foreman resume is a **new delivery revision** with a new random
`delivery_id`, not another attempt on the old accepted/ambiguous wake.

## 5. Exact foreman binding

V1 has one active binding.

### Existing chat

```text
unbound
  -> navigate requested /c/<id>
  -> verify resolved canonical conversation exactly
  -> verify profile/app/capabilities
  -> commit new binding generation
```

Any displacement/login/deleted/wrong-chat route fails before composer mutation.

### Rebind

Verify new target, increment generation, activate new binding, supersede old one,
and reject all old-generation mutations.

### New chat

Never persist `/`/temporary route. Commit only after a concrete canonical
`/c/<id>` exists.

## 6. Physical ChatGPT turn

```text
idle/unknown -> starting -> active -> settled
                         \-> observation_lost
```

No new wake while the exact bound surface is active/unknown. `settled` means only
that the physical assistant turn appears finished; it means neither MCP success nor
obligation processing.

## 7. Settled but unACKed resume

Eligible only when:

- prior wake accepted;
- physical turn settled;
- obligation still open;
- no current processing claim;
- no active/unknown ChatGPT turn;
- backoff elapsed;
- automatic-resume budget remains;
- current binding/capability still valid.

Create the next delivery revision for the same obligation. On budget exhaustion,
record `foreman_unreachable`, keep the obligation open indefinitely, and stop
automatic wakes.

## 8. Worker answer/resume delivery

Worker continuations use the same external-I/O discipline:

```text
pending -> claimed -> accepted | failed | ambiguous
```

Once resume/stdin/provider submission may have occurred, blind replay is forbidden.
Matching resumed-turn evidence reconciles acceptance and restores `running`.

## 9. Watchdog

For a running turn:

```text
last_verified_progress_at + threshold < now
  AND no confirmed terminal/input boundary
  -> suspected_stall attention
```

Later verified progress resolves the attention. Silence never emits synthetic Stop,
failure, completion, interrupt, or duplicate worker.

## Required invariants

1. Open obligation count cannot decrease without a closing disposition event.
2. Required result artifact cannot be released while an open obligation references
   it.
3. Duplicate confirmed terminal source events create at most one result obligation.
4. Claude Stop callback alone cannot create `completed_unprocessed`.
5. A Stop candidate blocked by another hook cannot create completion.
6. A clean managed `claude -p` defer requires a confirmed single-tool deferred
   boundary; multi-tool defer cannot fabricate `needs_input`.
7. `PermissionRequest` is handled according to pinned non-interactive semantics but
   is not treated as an exact durable pause identity without a matching fence.
8. Old session-incarnation events cannot mutate current incarnation.
9. Old binding generation cannot ACK/answer current work.
10. Browser attempt is `claimed` before any browser I/O.
11. Send ambiguity fence is durable before exact Send.
12. Startup quarantines orphaned claimed/armed attempts before browser recovery.
13. Accepted/ambiguous browser delivery is never automatically resent.
14. Browser accepted != ChatGPT settled != foreman ACK.
15. Confirmed structured/native worker result/input beats stale Herdr `working`.
16. Watchdog creates attention, never fake terminal state.
17. Deterministic delivery metadata cannot be used to derive the random wake
   `delivery_id` required for `foreman_resume`.
