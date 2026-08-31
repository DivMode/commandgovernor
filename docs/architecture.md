# Command Governor V1 architecture

Status: **proposed implementation architecture**  
Research snapshot: [2026-08-31 technology review](research/2026-08-31-technology-review.md)

## Mission

Command Governor is a local-first durable control plane for AI/software-engineering
workers. Its job is not to make a terminal look busy or to forward prompts. Its
job is to preserve **what work is still owed** when browsers, foremen, workers,
runtimes, or the governor itself disappear and restart.

The central invariant is:

> Delegated work remains a durable obligation until the bound foreman has fetched
> the real result or blocking request, performed the required review/action, and
> explicitly ACKed a fenced obligation. Worker completion, browser delivery, and
> ChatGPT turn settlement do not close it.

V1 is a Rust daemon plus CLI. There is no conventional application GUI and no
human completion-notification subsystem.

## Roles and authority

| Component | Role | Authoritative for | Not authoritative for |
| --- | --- | --- | --- |
| ChatGPT Web foreman | Planner and independent reviewer of record | review/disposition decisions it explicitly records | worker/process liveness, browser delivery, global lifecycle truth |
| Claude Code / Codex / future agents | Workers | their own native lifecycle/output facts | whether work was reviewed/consumed |
| GitHub | Engineering source of truth | issues, commits, PRs, reviews | transient worker/input/browser state |
| Herdr / runtime adapters | Process/session layer | process existence, PTY/session transport facts | task completion when native worker lifecycle disagrees |
| Command Governor | Control-plane authority | event order, projections, obligations, delivery ambiguity, binding generations, ACK validity | repository correctness or user-owned authorization decisions |

Provider status strings are observations. They never overwrite a stronger domain
fact merely because they are newer.

## V1 process boundary

```text
                          authenticated local browser profile
                                     │
                                     ▼
                         headed Chrome / Chromium
                                     ▲
                                     │ CDP
                                     │
┌────────────────────────────────────┴────────────────────────────────────┐
│                     command-governor daemon                             │
│                                                                          │
│  ┌────────────────┐   ┌─────────────────────┐   ┌────────────────────┐  │
│  │ lifecycle core │◄─►│ single-writer SQLite│   │ result artifact    │  │
│  │ + projections  │   │ DB actor            │   │ store (private)    │  │
│  └───────┬────────┘   └─────────────────────┘   └────────────────────┘  │
│          │                                                               │
│  ┌───────┴────────┐ ┌──────────────────┐ ┌───────────────────────────┐  │
│  │ worker adapters│ │ runtime adapters │ │ governor-chatgpt-web      │  │
│  │ Claude / Codex │ │ Herdr / future   │ │ browser + CDP evidence   │  │
│  └────────────────┘ └──────────────────┘ └─────────────┬─────────────┘  │
│                                                        │                │
│  ┌────────────────┐ ┌──────────────────┐               │                │
│  │ GitHub adapter │ │ rmcp MCP server  │◄──────────────┘                │
│  └────────────────┘ └─────────┬────────┘                                │
└───────────────────────────────┼──────────────────────────────────────────┘
                                │ OpenAI-supported tunnel/connectivity
                                ▼
                       ChatGPT Command Governor app

command-governor CLI ── local IPC ──► daemon
```

There is one orchestration authority. The browser supervisor, MCP endpoint,
runtime adapters, and lifecycle engine are parts of the daemon, not three
independent daemons with competing truth.

A supervised Secure MCP Tunnel process may exist because current ChatGPT
connectivity requires it; it is a transport child, never a second state owner.

## Rust workspace boundary

After the architecture gates pass sufficiently to begin scaffolding, use a small
Cargo workspace rather than an application starter:

```text
crates/
  governor-core/              # IDs, events, state machines, policies; no I/O
  governor-store-sqlite/      # rusqlite DB actor, migrations, replay/recovery
  governor-runtime/           # runtime traits + shared fencing contracts
  governor-runtime-herdr/     # Herdr adapter
  governor-worker-claude/     # Claude lifecycle/defer-resume adapter
  governor-worker-codex/      # Codex adapter
  governor-browser/           # narrow generic CDP/browser abstraction
  governor-chatgpt-web/       # ALL unofficial ChatGPT Web-specific behavior
  governor-mcp/               # stable ChatGPT-facing rmcp ABI
  governor-github/            # GitHub durable-source adapter
  governor-daemon/            # composition root, supervisors, local IPC
  governor-testkit/           # deterministic fakes and crash/failure fixtures
  command-governor/           # CLI/binary surface
```

Crates may be merged if implementation proves a boundary too fine-grained, but
`governor-core`, `governor-store-sqlite`, and `governor-chatgpt-web` remain hard
architectural boundaries. The rest of the system must be able to delete and
replace `governor-chatgpt-web` if OpenAI ships an official foreman/wake API.

Use Rust stable 1.98.0 / edition 2024 as the initial pin, re-verifying versions at
the scaffold commit. Domain crates use typed `thiserror` errors; `anyhow` is
limited to binary/process composition boundaries.

## Durable truth model

The control plane stores **source events and durable projections**, not a set of
mutable booleans.

Core identities:

- `project_id`
- `task_id`
- `session_id`
- `session_incarnation_id`
- `turn_id`
- `source_event_id`
- `result_artifact_id`
- `obligation_id`
- `input_request_id`
- `foreman_binding_id` + monotonic `binding_generation`
- `delivery_id` + `delivery_revision`
- `foreman_claim_id`

A session name is display metadata, never a sufficient fence.

Events are immutable and ordered by a daemon-assigned sequence. Materialized
projections are transactional caches and must be replayable. If replay and a
materialized projection disagree, startup fails closed into repair/doctor mode;
it does not silently choose the more convenient state.

See [data model](data-model.md).

## Obligation lifecycle

A worker attempt and its foreman obligation are deliberately separate.
Conceptually:

```text
created
  │
  ▼
running ───────────────► needs_input
  │                         │
  │                         └── answer/resume ──► running
  ├──────────────► failed
  │
  └──────────────► completed_unprocessed
                         │
                         ▼
                 claimed_by_foreman
                         │
                         ▼
                     processing
                         │
                         ▼
                    acknowledged
```

`needs_input`, `failed`, and `completed_unprocessed` are durable attention states.
A terminal worker result creates or updates the obligation; it does not close it.

`suspected_stall` is a non-terminal attention condition. It never converts
silence into completion or failure.

ACK is fenced by task/session-incarnation/turn/source-event/obligation/binding
generation and the current foreman claim. A stale conversation or old binding
generation cannot close current work.

See [state machines](state-machines.md).

## Result durability boundary

The obligation invariant is meaningless if the worker result vanishes with its
PTY. Therefore a terminal result must be made durable **before**
`completed_unprocessed` is published.

Command Governor will not store full terminal transcripts in SQLite. Instead it
uses a private immutable result-artifact store:

- daemon-private directory (0700 on Unix; equivalent owner-only ACL on Windows);
- artifact file owner-only (0600 on Unix);
- SQLite stores opaque reference, digest, size, source event, and retention state;
- content can contain the bounded final worker result needed by the foreman;
- content never enters browser wake text or routine tracing;
- artifact deletion cannot precede the obligation's closing disposition and
  retention policy.

GitHub commit/PR refs are complementary engineering evidence, not a substitute
for preserving the actual result that created the obligation.

## Native worker lifecycle beats runtime inference

Worker adapters normalize the strongest available native events. For Claude Code,
current hooks provide authoritative `Stop`, `StopFailure`, `PermissionRequest`,
and tool-progress evidence. Current non-interactive `AskUserQuestion` deferral can
also preserve a blocked tool call for later same-session resume.

Ordering policy is evidence-specific, not simply "last timestamp wins":

- native `Stop` proves the worker response boundary even if Herdr still says
  `working`;
- native input/permission blocking proves `needs_input` even if a PTY is not idle;
- a later native resumed-turn event can supersede the previous blocked projection;
- runtime death is important evidence but cannot erase an already persisted
  result/obligation;
- screen repaint/idle heuristics are fallback diagnostics only.

The exact stale-Herdr failure reproduced in Tandem is a mandatory deterministic
test fixture.

See [worker lifecycle contract](worker-lifecycle.md).

## Watchdog

Progress is based on verified lifecycle/tool activity, not terminal repaint.
`PostToolUse`/equivalent events may update only bounded metadata such as
`last_progress_at` and a safe event class.

A configured no-progress threshold creates `suspected_stall`. Long builds with
verified progress remain healthy. No monitor-only Claude session is ever opened
just to watch another worker.

## ChatGPT conversation binding

There is exactly one active foreman binding in V1.

Persist:

- canonical ChatGPT conversation ID and `/c/<id>` URL;
- `binding_generation`;
- dedicated browser-profile identity;
- provider/account/profile metadata sufficient to detect displacement without
  storing credentials;
- connector/app compatibility epoch and last successful capability preflight.

Never bind "current tab", browser history, or most-recent conversation.

Binding an existing chat requires the browser to resolve to the exact expected
conversation before composer mutation. A new-chat workflow must not commit `/` or
another provisional route; binding is committed only after a concrete final
`/c/<id>` exists.

Rebinding increments `binding_generation` transactionally. Old deliveries,
claims, or ChatGPT turns from a prior generation may be observed for history but
cannot ACK current obligations.

## ChatGPT transport: browser-backed hybrid

### Write path

Sensitive submission belongs to the authenticated ChatGPT SPA:

1. acquire the daemon's single-flight browser-delivery worker;
2. load the exact bound conversation;
3. verify canonical conversation ID and connector/app availability;
4. select/mention the Command Governor app for **this message** as required by
   the current ChatGPT surface;
5. stage only the tiny opaque wake payload;
6. durably arm the ambiguity fence;
7. invoke the exact composer-local Send action once;
8. observe CDP/network and message-tree evidence;
9. persist `accepted`, `failed`, or `ambiguous`.

Wake text contains only opaque identifiers and an instruction to use the Command
Governor app. It does not contain worker output, source, prompt text, cwd,
terminal transcript, tool arguments, secrets, or GitHub credentials.

### Observation path

Prefer CDP evidence:

- exact Target/Page identity;
- navigation/redirect events;
- request initiation;
- response/stream events;
- message identifiers/conversation identifiers visible in real SPA traffic;
- physical assistant-turn start/settlement.

DOM is required for structural control (composer, app selection, Send) and is a
fallback observation source. Weak evidence such as "composer became empty",
"URL changed", or "Stop appeared" never promotes a delivery to accepted by
itself.

Narrow authenticated reads/passive network interpretation may be used for
reconciliation if they prove stable. They are optimization/observation only; no
protected private-write client is implemented.

See [browser transport](browser-transport.md).

## Browser delivery safety model

Each wake revision has a deterministic identity such as:

```text
delivery_id = H("command-governor/wake/v1",
                obligation_id,
                binding_generation,
                delivery_revision)
```

A database transaction records `claimed` before external browser I/O.
Immediately before the exact Send activation, another durable transition records
`activation_armed`. That is the external-I/O ambiguity fence.

Terminal outcomes:

- **failed** — Command Governor has evidence submission did not occur; bounded
  retry is safe.
- **accepted** — strong semantic evidence shows the intended user message was
  submitted to the exact bound conversation.
- **ambiguous** — submission may have occurred but cannot be proven.

On startup, any nonterminal `claimed`/`activation_armed` attempt from the previous
daemon is converted to `ambiguous` before browser recovery. This deliberately
allows a crash before the physical click to create a zero-send ambiguity; safety
wins over liveness.

`accepted` and `ambiguous` are never automatically resent. Ambiguous may be
promoted to accepted only by exact reconciliation evidence.

## ChatGPT settlement is not ACK

Three facts remain distinct:

1. browser delivery accepted;
2. physical ChatGPT assistant turn settled;
3. foreman explicitly processed and ACKed the obligation through MCP.

Only (3) closes the obligation.

If (2) occurs without (3), the obligation remains outstanding. After a bounded
policy delay, with no overlapping ChatGPT turn, Command Governor may create a
**new delivery revision** for the same obligation. Automatic resumes are capped;
a repeated failure becomes durable `foreman_unreachable` health state while the
obligation remains open indefinitely.

## MCP contract

Use the official Rust SDK (`rmcp`). Keep the ChatGPT-facing ABI intentionally
small because connector schemas can be cached and current ChatGPT app updates
require explicit refresh/action enablement.

V1 surface:

- `foreman_bootstrap` — discover protocol/health/binding generation and urgent
  outstanding work;
- `foreman_resume` — claim/fetch one fenced obligation and its real result or
  input request;
- `foreman_ack` — explicit state-changing disposition; the only normal path that
  closes processed work;
- `foreman_answer_input` — structured state-changing response to a fenced worker
  input request.

No general arbitrary-action dispatcher is exposed in V1. Tool responses are
versioned/forward-compatible so additive fields do not require a new tool every
week.

All mutation tools require current binding generation, obligation/source-event
identity, and claim fencing. The daemon rejects stale claims without side effects.

Current published ChatGPT product availability makes write-capable MCP a **hard
binding preflight**. If the actual account/surface cannot call state-changing MCP
actions, V1 must report that unsupported combination; it must not fake ACK from
browser settlement.

See [MCP contract](mcp-contract.md).

## Persistence

Use `rusqlite` with one daemon-owned DB actor and bundled SQLite.

Initial policy:

- WAL;
- foreign keys enabled;
- bounded busy timeout;
- `synchronous=FULL` until crash-injection testing justifies another choice;
- explicit migrations and schema epoch;
- append source event + update projection + create obligation in one transaction
  where the facts are inseparable;
- uniqueness constraints for source-event and terminal-event deduplication;
- no ORM;
- no browser cookies/tokens or terminal transcripts in the database.

Async daemon tasks communicate with the DB actor through typed requests rather
than sharing arbitrary connections.

## Local IPC and secrets

CLI operations go through daemon-owned local IPC. Prefer a Unix domain socket on
macOS/Linux and a named pipe on Windows. If a loopback HTTP fallback is required,
it must use an owner-local capability token and reject non-loopback origins.

The dedicated Chrome profile is credential-equivalent and owner-private. The DB,
artifact store, logs, tunnel credentials, and local control endpoints have
separate least-privilege paths. Secrets never appear in command-line arguments or
structured tracing fields.

## GitHub integration

GitHub remains engineering truth. Command Governor may record opaque repository,
issue, commit, and PR identifiers and use them during review, but it does not
replace GitHub with a local source-code database.

GitHub content and worker output are untrusted input to the foreman. The MCP
boundary must label them as data, preserve provenance, and never treat text inside
an issue/diff/result as Command Governor policy.

## Startup recovery order

Before accepting new orchestration work, daemon startup must:

1. acquire the single-daemon ownership lock;
2. validate filesystem ownership/permissions;
3. open SQLite, verify schema epoch, integrity policy, and migration status;
4. convert orphaned browser `claimed`/`activation_armed` attempts to `ambiguous`;
5. replay/validate materialized projections;
6. verify referenced result artifacts needed by open obligations;
7. reconcile native worker lifecycle against runtime observations;
8. restore watchdog schedules without fabricating terminal events;
9. reconnect/supervise the browser and MCP tunnel;
10. verify the exact foreman binding and capability epoch before any wake;
11. resume only operations proven idempotent/safe.

Recovery never converts missing observations into success.

## V1 explicit non-goals

- conventional GUI, Electron, Tauri, Dioxus, Iced, or menu-bar app;
- phone/email/Slack/Telegram/ntfy completion notifications;
- hosted multi-tenant control plane;
- storing full terminal transcripts or browser credentials in the ledger;
- private ChatGPT protocol emulation or challenge bypass;
- exactly-once claims over interfaces without transactional idempotency;
- multiple simultaneously active foreman conversations;
- using runtime idle/screen state as stronger truth than native lifecycle;
- permitting a worker to independently approve its own implementation.

## Architecture gates before implementation claims

The domain/store/testkit foundation can proceed once this ADR set is accepted,
but the ChatGPT end-to-end V1 is not "supported" until two live gates pass:

### Gate A — MCP mutation capability

The exact target ChatGPT account/surface must invoke `foreman_ack` and
`foreman_answer_input` as genuine state-changing tools through the supported
connector path. Published Pro limitations mean this cannot be assumed.

### Gate B — browser transport spike

A dedicated headed Chrome profile must pass the spike in
[browser-transport.md](browser-transport.md): exact binding, per-message app
selection, ten sends, strong accepted evidence, crash-at-send ambiguity, no
replay, profile restart, and generation fencing. Headless is evaluated separately
and remains experimental unless it matches the headed result.

If either gate fails, the durable control-plane architecture remains valid. The
unsupported adapter/surface is fenced off rather than weakening the invariant.
