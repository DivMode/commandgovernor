# V1 state machines

Command Governor models facts that other systems commonly collapse together:
worker lifecycle, foreman obligation processing, browser delivery, physical
ChatGPT turns, and worker-input delivery. The separation is the correctness
feature.

## State-machine rules

- Every transition is caused by an immutable accepted event.
- Provider status strings and hook callbacks are evidence classes, not domain
  states by themselves.
- A state transition is fenced by the identities relevant to that transition.
- Duplicate source events are idempotent.
- Silence is never success.
- An external side effect that may have happened is never automatically replayed.
- Closing an obligation always requires an explicit terminal disposition event.
- For providers whose hooks can veto one another, hook invocation is not terminal
  until the managed execution protocol confirms the outcome.

## 1. Durable obligation

The V1 obligation projection intentionally answers one operational question:
**what work is still owed now?**

### States

- `created` — durable work exists but the owning worker turn has not been accepted
  as running.
- `running` — a verified worker turn is in progress.
- `needs_input` — a verified durable pause/input boundary requires a decision.
- `failed` — a worker attempt reached a verified failure the foreman has not yet
  processed.
- `completed_unprocessed` — a confirmed worker result is durably captured but not
  processed/ACKed by the foreman.
- `claimed_by_foreman` — the current binding generation claimed the exact
  obligation version through MCP.
- `processing` — claimed result/input was handed to the current foreman tool
  call/turn. This is **not** acknowledgement.
- `acknowledged` — an explicit fenced foreman disposition closed the obligation.
- `cancelled_by_user` — explicit user-owned cancellation closed it.
- `superseded` — explicit recorded policy made this exact obligation obsolete.

`suspected_stall` is an attention condition layered on `running`, never a terminal
obligation state.

### Primary transitions

```text
created
  │ verified worker dispatch/start
  ▼
running
  ├── confirmed deferred/blocking boundary ─► needs_input
  │                                             │
  │            answer + fenced worker resume + │
  │            verified resumed-turn evidence  │
  │                                             │
  │◄────────────────────────────────────────────┘
  │
  ├── verified terminal failure ─────────────► failed
  │
  └── confirmed final result + durable artifact
                                      ───────► completed_unprocessed
                                                    │
needs_input / failed / completed_unprocessed        │
  │ foreman_resume(current generation/version)      │
  ▼                                                 │
claimed_by_foreman                                  │
  │ result/input handed to tool response            │
  ▼                                                 │
processing                                          │
  │ explicit foreman_ack + valid claim/fences       │
  ▼                                                 │
acknowledged ◄──────────────────────────────────────┘
```

A failure is **unprocessed work**, not automatic closure. The foreman may ACK a
failure with a disposition such as `reviewed_failure`, possibly after creating a
replacement attempt.

### Claim behavior

`foreman_resume` is state-changing: it creates a claim for one obligation version
under one `binding_generation`. A claim has an ID and bounded lease. Claim expiry
is internal coordination, not external worker/browser I/O; expiry may return the
obligation to its prior attention state but never closes it or releases a required
artifact.

### ACK fence

Normal ACK requires:

```text
obligation_id
obligation_version
source_event_id / source event fence
binding_generation
claim_id
terminal disposition
```

Any stale value returns a typed conflict with zero state mutation. A conversation
bound under generation N cannot ACK work under N+1.

### Terminal dispositions

Initial semantic values can include:

- `reviewed_accepted`
- `reviewed_changes_requested`
- `reviewed_no_action`
- `reviewed_failure`
- `input_resolved`
- `cancelled_by_user`
- `superseded`

Disposition never skips processing; ACK is valid only from a current fenced claim.

## 2. Worker lifecycle evidence

Worker lifecycle is normalized per `session_incarnation_id + turn_id`.

### Managed Claude V1 evidence precedence

The preferred execution path is `claude -p` with structured output. Current Claude
Code `Stop` hooks can block stopping, and all matching hooks run in parallel.
Therefore a Stop-hook callback is **not** a terminal fact by itself.

For managed non-interactive Claude V1, precedence is:

1. **final structured Claude programmatic result + matching command/process exit
   receipt** — authoritative terminal outcome for the fenced managed run;
2. **non-blockable/strong native events** such as `StopFailure`, `SessionEnd`, and
   a confirmed `PreToolUse` defer boundary — strong evidence reconciled with the
   structured result/exit;
3. **`Stop` hook invocation** — `stop_candidate` only; another parallel Stop hook
   may return `decision: block` and Claude may continue;
4. **Herdr/process-session observation** — transport evidence; a stale `working`
   sample cannot erase a confirmed final/deferred managed-run outcome;
5. **PTY idle/screen repaint** — fallback diagnostics only.

"Newest timestamp wins" is rejected. Evidence class and fence matter.

### Successful completion

A `Stop` hook callback may create `stop_candidate` but does not transition the
obligation to `completed_unprocessed`.

Successful completion requires the worker-host/runtime adapter to prove the final
structured programmatic result for the exact managed turn and the matching child
command exit. The bounded final result is then made durable in the private result
artifact store. Only the transaction that references that durable artifact may
publish `completed_unprocessed`.

If the stream is truncated, the final result is absent, the child exit is unknown,
or artifact publication fails, create reconciliation/health attention and do
**not** pretend the result is processable.

### Failure

`StopFailure` is strong native evidence that a turn ended due to Claude API error,
but the adapter still records/reconciles the managed command outcome. Non-zero
command exit, explicit interrupt, transport loss, and malformed/truncated provider
output are separate failure classes rather than one undifferentiated `failed`.

The domain may project `failed` only from a documented accepted combination of
those evidence classes.

### Stop-hook veto race

Required scenario:

```text
CG Stop hook fires -> records stop_candidate
other parallel Stop hook returns decision:block
Claude continues working
later CG Stop hook fires again
final structured result + process exit arrives
```

Expected: no `completed_unprocessed` after the first candidate; only the confirmed
final managed result can create completion.

## 3. `needs_input`

Input is first-class durable state, but a hook merely *requesting* a pause is not
sufficient if the provider did not actually honor it.

### Input classes

- `engineering_question` — ordinary bounded coordination within delegated scope;
- `permission_request` — action needs a policy/permission decision;
- `user_owned_decision` — credentials, destructive actions, broader authority, or
  another policy says only the user may decide;
- `runtime_input` — verified runtime-level blocking request;
- `provider_elicitation` — provider/MCP form or elicitation boundary when the
  adapter can prove it is durably pending.

The ledger stores only safe opaque identity/classification and answer shape. Raw
Claude tool arguments/question payloads are not copied into SQLite. Current detail
is retrieved ephemerally from the native provider/session when available.

### `AskUserQuestion` defer

Preferred non-interactive path:

1. `PreToolUse` identifies the exact fenced `AskUserQuestion` tool call;
2. the Command Governor hook durably records a safe defer intent and returns the
   current documented `defer` response;
3. the adapter waits for the structured managed-run outcome that proves the run
   actually stopped with the call pending;
4. only then project `needs_input`.

If the defer response is ignored/malformed or several tool calls create an
unsupported partial-execution shape, preserve reconciliation attention instead of
creating a false clean pause.

### Permission requests

`PermissionRequest` is authoritative evidence that Claude requested permission,
but its own hook is not assumed to be a durable pause/resume primitive. V1 should
prefer a policy `PreToolUse` defer **before** a tool whose authorization must be
resolved out of band. `PermissionRequest`, `PermissionDenied`, and current
permission-prompt-tool behavior remain additional evidence/integration points and
must pass conformance before becoming lifecycle authority.

### Answer path

`foreman_answer_input` records only a structured answer after verifying claim,
obligation version, binding generation, input identity, and authorization policy.
It then creates a separate **worker command delivery**.

Answer recorded != worker received it.

For a deferred Claude call, the same Claude session is resumed through the current
supported provider mechanism. Only verified structured/native resumed-turn
evidence moves the obligation back to `running`.

### Authorization boundary

A ChatGPT foreman cannot convert an ungranted high-risk permission into authority
because a worker requested it. Unknown/broader/destructive requests are user-owned
by default; `foreman_answer_input` fails closed outside recorded policy.

## 4. Browser wake delivery

### Identity

```text
delivery_id = H(
  "command-governor/wake/v1",
  obligation_id,
  binding_generation,
  delivery_revision
)
```

A delivery also snapshots the exact target obligation version/source event. A wake
that became stale before Send cannot submit.

### Projection states

- `pending`
- `claimed`
- `accepted`
- `failed`
- `ambiguous`

Each revision may have multiple attempts only while every earlier attempt is
proved failed before the Send ambiguity fence.

### Attempt states

- `claimed` — committed before **any browser I/O**;
- `activation_armed` — committed immediately before exact Send activation;
- `failed`;
- `accepted`;
- `ambiguous`.

### Transition graph

```text
pending
  │ DB transaction before browser I/O
  ▼
claimed
  ├── definite pre-submit error ───────────► failed
  │                                            │
  │                bounded safe retry may     │ create next attempt
  │◄───────────────────────────────────────────┘
  │
  │ all target/obligation/composer/app checks pass
  ▼
activation_armed
  ├── exact semantic submission evidence ─► accepted
  ├── definite proof submission did not happen ─► failed
  └── evidence lost/uncertain ─────────────► ambiguous
```

### Restart rule

On startup, any previous-process attempt whose latest state is `claimed` or
`activation_armed` without a terminal result becomes `ambiguous` **before browser
recovery**.

This intentionally may quarantine a zero-send crash. At-most-once safety is more
important than guessing and sending twice.

### Accepted evidence

`accepted` requires semantic evidence binding the intended wake to the exact
conversation, preferably a provider user-message identity observed in real SPA
network/message-tree evidence plus matching conversation/opaque delivery identity.

Insufficient alone:

- composer cleared;
- URL changed;
- Stop button appeared;
- assistant started;
- wake text appears somewhere in the DOM.

### Ambiguous reconciliation

Automatic reconciliation may only promote:

```text
ambiguous -> accepted
```

when exact evidence identifies the already-submitted wake. Absence is not proof of
no submission, so ambiguous is not auto-demoted to failed and never auto-retried.

An operator may later explicitly supersede an ambiguity under a separately audited
policy; history is never rewritten.

### Accepted/ambiguous freeze

Once any attempt is accepted or ambiguous, no attempt for that revision may
activate Send again. A later bounded resume is a new `delivery_revision` and ID.

## 5. Exact ChatGPT conversation binding

There is one active binding in V1.

### Bind existing conversation

```text
unbound
  -> navigate requested exact /c/<id>
  -> browser reports resolved route
  -> verify canonical conversation ID == requested ID
  -> verify dedicated profile identity
  -> verify Command Governor app availability/capability
  -> commit new binding generation
```

Redirect/displacement/wrong chat fails before composer mutation.

### Rebind

1. verify new target independently;
2. increment generation;
3. insert/activate new binding;
4. supersede old generation;
5. invalidate old claims for mutation purposes.

Old accepted deliveries remain historical facts but cannot satisfy new-generation
ACK fences.

### New-chat binding

Never persist `/` or a provisional route. Commit only after ChatGPT assigns a
concrete canonical `/c/<id>`.

## 6. Physical ChatGPT turn

```text
unknown/idle -> starting -> active -> settled
                         \-> observation_lost
```

No new wake while the exact bound surface is active. `observation_lost` must be
reconciled before another Send.

`settled` means the physical assistant response appears finished by strong
evidence. It means neither "MCP tools worked" nor "obligation processed."

## 7. Settled but unACKed resume policy

A bounded automatic resume is eligible only when:

- prior wake is `accepted`;
- its physical ChatGPT turn is `settled`;
- obligation is still open;
- no current claim is processing;
- no ChatGPT turn is active/unknown;
- backoff elapsed;
- automatic-resume budget remains;
- exact binding/capability preflight passes.

Action: create a **new** delivery revision for the same obligation.

After budget exhaustion, create/update `foreman_unreachable`; keep the obligation
open indefinitely. Never create an infinite wake loop.

## 8. Worker answer/resume delivery

The same external-I/O rule applies when sending an answer/continuation to a
worker:

```text
pending -> claimed -> accepted | failed | ambiguous
```

An answer can be retried only before worker I/O is proven possible. Once a resume,
stdin write, or provider submission may have happened, ambiguity is durable and
blind replay is forbidden. Matching structured/native resumed-turn evidence is
used to reconcile acceptance and return the obligation to `running`.

## 9. Watchdog / suspected stall

For each running turn:

```text
last_verified_progress_at + threshold < now
  AND no confirmed terminal/input boundary
  -> suspected_stall attention event
```

Later verified progress resolves the attention and keeps the turn running. No
amount of silence emits a synthetic Stop, failure, or completion.

## 10. Required invariants

1. Open obligation count cannot decrease without a closing disposition event.
2. Result artifact retention cannot release while an open obligation references
   it.
3. Duplicate confirmed terminal source events produce at most one result
   obligation.
4. A Claude `Stop` hook callback alone cannot create `completed_unprocessed`.
5. A Stop candidate blocked by another parallel hook cannot produce completion.
6. Old session-incarnation events cannot mutate the current incarnation.
7. Old binding generation cannot ACK or answer current work.
8. Browser attempt is `claimed` before any browser I/O.
9. Send ambiguity fence is durable before exact Send activation.
10. Startup turns orphaned `claimed`/`activation_armed` into `ambiguous` before
    browser recovery.
11. `accepted` and `ambiguous` are never automatically resent.
12. Browser accepted != ChatGPT settled != foreman ACK.
13. Confirmed structured/native worker terminal/input evidence beats stale Herdr
    `working` for the same fenced turn.
14. Watchdog creates attention, never fake terminal state.
15. Foreman answer cannot grant authority outside recorded user policy.
