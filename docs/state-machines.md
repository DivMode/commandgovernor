# V1 state machines

Command Governor models facts that other systems commonly collapse together:
worker lifecycle, foreman obligation processing, browser delivery, physical
ChatGPT turns, and worker-input delivery. The separation is the correctness
feature.

## State-machine rules

- Every transition is caused by an immutable accepted event.
- Provider status strings are evidence classes, not domain states by themselves.
- A state transition is fenced by the identities relevant to that transition.
- Duplicate source events are idempotent.
- Silence is never success.
- An external side effect that may have happened is never automatically replayed.
- Closing an obligation always requires an explicit terminal disposition event.

## 1. Durable obligation

The V1 obligation projection intentionally includes worker-attention and foreman
processing phases because the user needs to answer one question reliably:
**what work is still owed now?**

### States

- `created` — durable task/obligation exists but the owning worker turn has not
  been accepted as running.
- `running` — verified worker turn is in progress.
- `needs_input` — native lifecycle/input evidence says the worker cannot proceed
  without a decision/answer/permission.
- `failed` — worker attempt reached a verified failure that the foreman has not
  yet processed.
- `completed_unprocessed` — worker result is durably captured but not yet
  processed/ACKed by the foreman.
- `claimed_by_foreman` — the current binding generation has claimed the exact
  obligation version through MCP.
- `processing` — the claimed result/input has been handed to the current foreman
  tool call/turn for review/action. This is **not** acknowledgement.
- `acknowledged` — explicit fenced foreman ACK closed the obligation with a
  terminal disposition.
- `cancelled_by_user` — explicit user-owned cancellation closed it.
- `superseded` — a recorded policy transition made this exact obligation obsolete
  while preserving its history.

`suspected_stall` is an attention record layered onto `running`; it is not a
terminal obligation state.

### Primary transitions

```text
created
  │ worker dispatch accepted + native start/baseline
  ▼
running
  ├── native input/permission block ─────────► needs_input
  │                                               │
  │                   answer recorded + worker-resume delivery +
  │                   verified native resumed turn
  │                                               │
  │◄──────────────────────────────────────────────┘
  │
  ├── native/verified failure ───────────────► failed
  │                                               │
  └── result durable + terminal event ───────► completed_unprocessed
                                                  │
needs_input / failed / completed_unprocessed      │
  │ foreman_resume(current generation/version)    │
  ▼                                               │
claimed_by_foreman                                │
  │ result/input handed to tool response          │
  ▼                                               │
processing                                        │
  │ explicit foreman_ack + valid claim/fences     │
  ▼                                               │
acknowledged ◄────────────────────────────────────┘
```

A failure is **unprocessed work**, not automatic closure. The foreman may ACK a
failure with a disposition such as `reviewed_failure`, possibly after creating a
replacement task/attempt.

### Claim behavior

`foreman_resume` is state-changing: it creates a claim for one obligation version
under one `binding_generation`. This is why the actual ChatGPT connector must
support write-capable MCP; it is not acceptable to mislabel this as read-only.

A claim has an ID and bounded lease. Claim expiration is safe because it is
internal coordination, not external worker/browser I/O. If a claim expires
without ACK or another durable transition, the obligation returns to its prior
attention state (`needs_input`, `failed`, or `completed_unprocessed`) and can be
claimed again.

The result artifact remains pinned throughout claim expiry/reclaim.

### ACK fence

Normal ACK requires all of:

```text
obligation_id
obligation_version
source_event_id / source_event fence
binding_generation
claim_id
terminal disposition
```

If any value is stale, the operation returns a typed stale/conflict response and
performs no state change. A conversation bound under generation N can never ACK
work claimed under N+1.

### Terminal dispositions

The schema stores a disposition separately from state. Initial values should be
small and semantic, for example:

- `reviewed_accepted`
- `reviewed_changes_requested`
- `reviewed_no_action`
- `reviewed_failure`
- `input_resolved`
- `cancelled_by_user`
- `superseded`

A disposition is not a way to skip processing: ACK is accepted only from a current
claim over the exact source version.

## 2. Worker lifecycle evidence

Worker lifecycle is normalized per `session_incarnation_id + turn_id`.

### Evidence precedence

For Claude Code V1:

1. native lifecycle (`Stop`, `StopFailure`, blocking input/permission events,
   verified resume/start) is strongest;
2. process exit/start is strong runtime evidence but cannot contradict an already
   persisted native terminal fact;
3. Herdr process/session state is runtime evidence;
4. PTY idle/screen heuristics are fallback diagnostics only.

"Newest timestamp wins" is explicitly rejected. A late Herdr `working` sample
cannot erase an earlier native `Stop` from the same fenced turn.

### Completion

`Stop` does not immediately publish `completed_unprocessed` until the bounded
result artifact is durable. The publication transaction references the terminal
source event and artifact together.

If result capture cannot be made durable, the system creates a health/recovery
condition and does **not** pretend completion is processable.

### Failure

`StopFailure` or an equivalent authoritative terminal error creates `failed` for
the obligation. If the runtime disappears without a native terminal event, the
adapter uses its documented reconciliation policy; missing evidence is not
converted to success.

## 3. `needs_input`

Input is first-class durable state, not terminal text scraping.

### Input classes

- `engineering_question` — an ordinary bounded coordination decision the foreman
  may answer if within delegated scope.
- `permission_request` — a tool/action permission boundary.
- `user_owned_decision` — credentials, destructive action, financial/legal/user
  authorization, or another decision policy says only the human may make.
- `runtime_input` — a verified runtime-level blocking request.

The native payload is normalized into a safe schema plus, when needed, a private
input artifact. Arbitrary tool arguments are not copied into the event ledger.

### Answer path

`foreman_answer_input` only records a structured answer after verifying claim,
obligation version, binding generation, input request identity, and authorization
policy. It then creates a **worker command delivery** to resume/answer the worker.

Recording an answer is not evidence that the worker received it.

For current Claude non-interactive `AskUserQuestion`, the preferred V1 path is
native defer/resume of the same Claude session where feasible. The pending tool
call is preserved by Claude, and Command Governor supplies the structured answer
on resume. After delivery, native resumed-turn evidence moves the obligation back
to `running`.

### Authorization boundary

A ChatGPT foreman may not convert an ungranted high-risk permission into granted
authority simply because a worker asks. A `user_owned_decision` remains open until
there is an explicit user-authorized event/policy scope. `foreman_answer_input`
fails closed for actions outside delegated policy.

## 4. Browser wake delivery

### Identity

One wake revision is deterministic:

```text
delivery_id = H(
  "command-governor/wake/v1",
  obligation_id,
  binding_generation,
  delivery_revision
)
```

`delivery_revision` increments only when policy intentionally creates a new wake
for the same still-open obligation, such as bounded resume after a settled but
unACKed foreman turn.

### Delivery projection states

- `pending`
- `claimed`
- `accepted`
- `failed`
- `ambiguous`

Each delivery has one or more attempt rows. Additional attempts are permitted only
when every prior attempt definitely failed **before** the send ambiguity fence.

### Attempt states

- `claimed` — committed before any browser I/O for this attempt.
- `activation_armed` — committed immediately before invoking the exact Send
  action; the external-I/O ambiguity boundary has been crossed for recovery.
- `failed`
- `accepted`
- `ambiguous`

### Transition graph

```text
pending
  │ DB transaction before browser I/O
  ▼
claimed
  ├── definite pre-submit error ─────────────► failed
  │                                               │
  │                     bounded retry may create │ next attempt
  │◄──────────────────────────────────────────────┘
  │
  │ all target/composer/app checks pass
  ▼
activation_armed
  ├── exact semantic submission evidence ───► accepted
  ├── definite evidence handoff did not occur ─► failed   (only if provable)
  └── evidence lost/uncertain ───────────────► ambiguous
```

### Restart rule

On daemon startup, **any previous-process delivery attempt whose latest durable
state is `claimed` or `activation_armed` and lacks a terminal result becomes
`ambiguous` before browser recovery starts.**

This is intentionally stricter than reconstructing from guessed stages. A crash
while merely navigating can therefore quarantine a delivery that probably sent
nothing. The tradeoff is deliberate: a zero-send ambiguity is recoverable;
a duplicate foreman wake can cause duplicate work and is not automatically safe.

### Accepted evidence

`accepted` requires semantic evidence that binds the intended submission to the
exact conversation, preferably a new user-message/provider message ID observed in
real SPA network/message-tree evidence plus the expected conversation identity.

These are **insufficient alone**:

- composer became empty;
- URL changed;
- Stop button appeared;
- assistant started generating;
- document text contains the wake payload.

### Ambiguous reconciliation

Automatic reconciliation may inspect the bound conversation/message tree and
promote:

```text
ambiguous -> accepted
```

only when exact evidence identifies the already-submitted wake. Absence of a
message is not proof that it was never submitted, so automatic reconciliation
does not demote ambiguous to failed and does not resend.

An explicit future operator resolution may decide that an ambiguous delivery is
safe to supersede, but that action is separately audited and creates a new
revision; it never rewrites history.

### Accepted/ambiguous freeze

Once any attempt is accepted or ambiguous:

- no attempt for that delivery revision may activate Send again;
- no generic retry loop may replay the wake;
- a later bounded resume is a new `delivery_revision` with a new deterministic ID.

## 5. Exact conversation binding

There is exactly one active binding in V1.

### Bind existing conversation

```text
unbound
  -> navigate requested exact /c/<id>
  -> browser reports resolved route
  -> verify canonical conversation ID == requested ID
  -> verify dedicated profile identity and Command Governor app availability
  -> capability preflight
  -> commit new binding generation
```

Any redirect/displacement/wrong chat before commit fails binding. No composer
mutation occurs.

### Rebind

Rebind is transactional:

1. verify the new target independently;
2. increment generation;
3. insert/activate new binding;
4. supersede old generation;
5. invalidate old foreman claims for mutation purposes.

Old accepted deliveries remain historical facts but cannot satisfy the new
generation's ACK fence.

### New-chat binding

A `/` route or provisional temporary URL is never persisted. If new-chat creation
is supported later, the binding is committed only after ChatGPT assigns a concrete
canonical `/c/<id>`.

## 6. Physical ChatGPT turn

Browser observation tracks a foreman turn independently:

```text
unknown/idle -> starting -> active -> settled
                         \-> observation_lost
```

No new wake is issued while the exact bound surface is known active. If turn state
is `observation_lost`, the system reconciles before any further send.

`settled` means the physical assistant response appears finished by strong
network/message evidence. It means neither "MCP tools worked" nor "obligation
processed".

## 7. Settled but unACKed resume policy

Condition for automatic resume:

- prior wake is `accepted`;
- its physical ChatGPT turn is `settled`;
- obligation remains open;
- no current foreman claim is actively processing;
- no ChatGPT turn is active/unknown;
- configured backoff has elapsed;
- automatic-resume budget is not exhausted;
- exact binding/capability preflight still passes.

Action: create the next `delivery_revision` for the **same obligation**.

The policy is bounded. After the configured maximum automatic resumes, create or
update durable `foreman_unreachable`; keep the obligation open indefinitely and
surface the condition through `status`, `obligations`, `doctor`, and
`foreman_bootstrap`. Never create an infinite wake loop.

## 8. Worker answer/resume delivery

The same external-I/O principle applies when Command Governor sends an answer or
continuation to a worker:

```text
pending -> claimed -> accepted | failed | ambiguous
```

An answer stored in SQLite can be retried before worker I/O. Once a worker resume
or PTY send may have occurred, ambiguity is durable and blind resend is forbidden.
Native worker events are used to reconcile whether the expected next turn began.

This prevents fixing browser duplication while retaining the same bug on the
worker side.

## 9. Watchdog / suspected stall

For each running turn:

```text
last_verified_progress_at + threshold < now
  AND no native terminal/input event
  -> suspected_stall attention event
```

A later verified progress event resolves the stall attention and keeps the turn
`running`. A later terminal/input event drives the normal lifecycle state.

No amount of watchdog silence emits `Stop`, `failed`, or `completed_unprocessed`.

## 10. Required invariants

These are implementation-level properties, not documentation aspirations:

1. Open obligation count cannot decrease without a closing disposition event.
2. Result artifact retention cannot release while an open obligation references
   it.
3. Duplicate native terminal source events produce at most one result obligation.
4. Old session incarnation events cannot mutate the current incarnation.
5. Old binding generation cannot ACK or answer current work.
6. Browser attempt is `claimed` before any target navigation/composer mutation.
7. Send ambiguity fence is durable before exact Send activation.
8. Startup turns orphaned `claimed`/`activation_armed` attempts into `ambiguous`
   before browser recovery.
9. `accepted` and `ambiguous` are never automatically resent.
10. Browser accepted != ChatGPT settled != foreman ACK.
11. Worker native completion/input beats stale Herdr `working` for the same fenced
    turn.
12. Watchdog can create attention, never fake terminal state.
13. Foreman answer cannot grant authority outside recorded user policy.
