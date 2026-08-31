# Command Governor V1 architecture

Status: **proposed implementation architecture**  
Research snapshot: [2026-08-31 technology review](research/2026-08-31-technology-review.md)

## Mission

Command Governor is a local-first durable control plane for AI/software-engineering
workers. Its job is not to make a terminal look busy or merely relay prompts. Its
job is to preserve **what work is still owed** when browsers, foremen, workers,
runtimes, or the governor itself disappear and restart.

The central invariant is:

> Delegated work remains a durable obligation until the bound foreman has fetched
> the real result or blocking request, performed the required review/action, and
> explicitly ACKed a fenced obligation. Worker completion, browser delivery, and
> ChatGPT assistant-turn settlement do not close it.

V1 is a Rust daemon plus CLI. There is no conventional application GUI and no
human completion-notification subsystem.

## Roles and authority

| Component | Role | Authoritative for | Not authoritative for |
| --- | --- | --- | --- |
| ChatGPT Web foreman | Planner and independent reviewer of record | explicit review/disposition decisions | worker/process liveness, browser delivery, global lifecycle truth |
| Claude Code / Codex / future agents | Workers | structured/native worker protocol facts and produced work | whether work was independently reviewed/consumed |
| GitHub | Engineering source of truth | issues, commits, PRs, reviews | transient worker/input/browser state |
| Herdr / runtime adapters | Process/session layer | process/session transport facts | semantic task completion when stronger worker evidence disagrees |
| Command Governor | Control-plane authority | event order, projections, obligations, ambiguity, bindings, ACK validity | repository correctness or user-owned authorization decisions |

Provider status strings and hook callbacks are observations. They do not become
terminal domain facts merely because of their name or timestamp.

## V1 process boundary

```text
                                   authenticated local browser profile
                                              │
                                              ▼
                                  headed Chrome / Chromium
                                              ▲
                                              │ CDP
                                              │
┌─────────────────────────────────────────────┴───────────────────────────────────┐
│                         command-governor daemon                                 │
│                                                                                  │
│  ┌─────────────────┐   ┌──────────────────────┐   ┌──────────────────────────┐  │
│  │ lifecycle core  │◄─►│ single-writer SQLite │   │ result artifact store    │  │
│  │ + projections   │   │ DB actor             │   │ (private, immutable)     │  │
│  └────────┬────────┘   └──────────────────────┘   └──────────────────────────┘  │
│           │                                                                      │
│  ┌────────┴────────┐  ┌────────────────────┐  ┌──────────────────────────────┐  │
│  │ worker adapters │  │ runtime adapters   │  │ governor-chatgpt-web         │  │
│  │ Claude / Codex  │  │ Herdr / future     │  │ browser + CDP evidence      │  │
│  └─────────────────┘  └──────────┬─────────┘  └──────────────┬───────────────┘  │
│                                  │                            │                  │
│  ┌─────────────────┐  ┌──────────┴─────────┐                 │                  │
│  │ GitHub adapter  │  │ rmcp MCP server    │◄────────────────┘                  │
│  └─────────────────┘  └──────────┬─────────┘                                    │
└──────────────────────────────────┼───────────────────────────────────────────────┘
                                   │ OpenAI-supported tunnel/connectivity
                                   ▼
                          ChatGPT Command Governor app

command-governor CLI ── owner-local IPC ──► daemon

Herdr/session runtime ──► command-governor worker-host claude <opaque-turn-id>
                              │
                              ├── launches/resumes `claude -p`
                              ├── private bounded structured-stream spool
                              └── sanitized child-exit receipt
```

There is one orchestration authority: the daemon. The browser supervisor, MCP
endpoint, lifecycle engine, runtime adapters, and GitHub integration are daemon
components. A Secure MCP Tunnel process and the Claude worker-host are **transport
children only**. Neither owns tasks, obligations, bindings, or terminal review
state.

The worker-host exists so a managed worker can finish while the daemon is
restarting without losing the provider's final structured result.

## Rust workspace boundary

After architecture acceptance, start with a small Cargo workspace rather than an
application starter:

```text
crates/
  governor-core/              # IDs, events, state machines, policies; no I/O
  governor-store-sqlite/      # rusqlite DB actor, migrations, replay/recovery
  governor-runtime/           # runtime traits + shared fencing contracts
  governor-runtime-herdr/     # Herdr adapter
  governor-worker-claude/     # structured Claude protocol/hooks/worker-host
  governor-worker-codex/      # Codex adapter
  governor-browser/           # narrow generic CDP/browser abstraction
  governor-chatgpt-web/       # ALL unofficial ChatGPT Web-specific behavior
  governor-mcp/               # stable ChatGPT-facing rmcp ABI
  governor-github/            # GitHub durable-source adapter
  governor-daemon/            # composition root, supervisors, local IPC
  governor-testkit/           # deterministic fakes and crash/failure fixtures
  command-governor/           # CLI + hook/worker-host binary modes
```

Crates may merge if implementation proves a boundary too fine-grained, but
`governor-core`, `governor-store-sqlite`, and `governor-chatgpt-web` remain hard
architectural boundaries. If OpenAI later publishes an official foreman/wake API,
we should be able to delete/replace `governor-chatgpt-web` without rewriting the
control plane.

Use Rust stable 1.98.0 / edition 2024 as the initial candidate pin and re-verify at
the scaffold commit. Domain crates expose typed `thiserror` errors; `anyhow` is
acceptable only at application/process composition boundaries.

## Durable truth model

The control plane stores **immutable source/domain events plus durable
projections**, not a few mutable booleans.

Core identities include:

- `project_id`
- `task_id`
- `session_id`
- `session_incarnation_id`
- `turn_id`
- `source_event_id` + source fence
- `result_artifact_id`
- `obligation_id` + version
- `input_request_id`
- `foreman_binding_id` + monotonic `binding_generation`
- `delivery_id` + `delivery_revision`
- `foreman_claim_id`

A session name is display metadata, never a sufficient identity fence.

Events are ordered by a daemon-assigned SQLite sequence. Materialized projections
are transactional caches and must be replayable/validatable. Projection mismatch
on startup fails closed into repair/doctor mode rather than choosing whichever
state is convenient.

See [data model](data-model.md).

## Obligation lifecycle

A worker attempt and its foreman-processing obligation are deliberately separate.
Conceptually:

```text
created
  │
  ▼
running ───────────────► needs_input
  │                         │
  │                         └── answer + confirmed resume ──► running
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
A worker terminal result creates/preserves work; it does not close it.

`suspected_stall` is a non-terminal attention condition. Silence never becomes
fake completion/failure.

ACK is fenced by the exact task/session-incarnation/turn/source-event/obligation
version/current binding generation and foreman claim. A stale conversation or old
binding generation cannot close current work.

See [state machines](state-machines.md).

## Result durability boundary

The invariant is meaningless if the result still lives only in a PTY. A confirmed
terminal result must be durable before `completed_unprocessed` becomes visible.

Command Governor does not store full terminal/provider transcripts in SQLite. It
uses an immutable private result-artifact store:

- daemon-private directory (`0700` on Unix; owner-only ACL equivalent elsewhere);
- owner-only files (`0600` on Unix);
- SQLite stores opaque reference, digest, size, source event, and retention state;
- content is only the bounded final worker result required for review;
- result content never enters browser wake text or routine tracing;
- open obligations pin artifact retention.

For Claude, the transport-level structured stream can briefly live in a separate
private worker-host spool because the daemon itself may be down when Claude exits.
The daemon validates that spool/exit receipt, extracts the bounded final result,
commits the immutable result artifact, then applies transport-spool retention.

GitHub commit/PR refs are complementary engineering evidence, not the only
persistence guarantee.

## Managed Claude lifecycle: structured result, not Stop-hook intuition

Current Claude Code has a critical semantic detail: multiple matching hooks can
run in parallel, and a `Stop` hook can block stopping and make Claude continue.
Therefore **our Stop hook firing is only a `stop_candidate`, not final
completion**.

For the preferred V1 managed execution mode, use Claude's programmatic
non-interactive interface with structured output (`claude -p` and current
stream-json equivalent). Successful terminal evidence is:

1. the final structured programmatic result for the exact fenced run; plus
2. the matching worker-host child-process/command completion receipt.

`StopFailure`, `SessionEnd`, a confirmed `PreToolUse` defer, structured failure,
interrupt, and process loss are separate evidence classes with explicit adapter
rules. Event names do not implicitly define precedence.

This means a stale Herdr `working` sample cannot override a confirmed structured
final/deferred result. But a lone Stop-hook candidate also cannot override Herdr;
it has not proved terminal settlement.

### Durable hook inbox

Managed hooks remain important for progress, input, permission, and native event
observations. Hooks first deposit a **sanitized** owner-private envelope to a local
inbox so daemon restart cannot lose the event. The inbox never stores raw prompt,
tool arguments, commands, cwd, transcript path, terminal transcript, or full
provider output.

### Input

For `AskUserQuestion` and tool calls that require out-of-band authorization,
prefer a current documented `PreToolUse` defer path when live conformance proves
it. Project `needs_input` only after the structured managed-run result confirms
the defer actually took effect.

`PermissionRequest` is strong evidence that Claude requested a decision, but it is
not assumed to be a durable pause/resume primitive. A policy `PreToolUse` defer
before execution is safer for decisions that must leave the worker and reach the
foreman/user.

High-risk, destructive, credential-sensitive, materially broader, or unknown
authorization stays user-owned. A worker request cannot widen authority.

### Configuration isolation

Command Governor never edits personal Claude settings. It supplies its own
owner-private managed settings file. Current Claude configuration can merge hooks
from user/project/plugin scopes, so `--settings` is **not** assumed to isolate
Command Governor from every other Stop hook. The adapter conformance suite must
prove the actual settings sources/hooks active for the chosen launch mode.

The completion rule remains correct even if another Stop hook exists because
terminal success is confirmed by the programmatic result/process receipt, not by
our hook callback.

See [worker lifecycle contract](worker-lifecycle.md).

## Watchdog

Progress uses verified structured/native/tool activity, not screen repaint.
Adapters persist only bounded safe metadata such as `last_progress_at` and a safe
event class.

A configured no-progress threshold creates `suspected_stall`. A 30-minute build is
healthy if progress is verified. No monitor-only Claude session is ever opened.

## ChatGPT conversation binding

V1 has exactly one active foreman binding.

Persist only:

- canonical ChatGPT conversation ID and `/c/<id>` URL;
- monotonic `binding_generation`;
- dedicated browser-profile identity (non-secret metadata);
- provider/account/profile metadata sufficient for displacement fencing;
- connector ABI/capability epoch and last successful preflight.

Never bind current tab, browser history, or most-recent conversation.

Binding an existing chat requires the browser to resolve to the exact expected
conversation before composer mutation. A new-chat workflow commits nothing until
ChatGPT has assigned a concrete final `/c/<id>`.

Rebinding increments generation transactionally. Old deliveries, claims, and
ChatGPT turns remain historical but cannot ACK current work.

## ChatGPT transport: browser-backed hybrid

### Write path

Sensitive submission belongs to the authenticated ChatGPT SPA:

1. acquire the daemon's single-flight browser delivery worker;
2. load and verify the exact bound conversation/profile;
3. verify the target obligation version/source event is still current;
4. verify Command Governor app/connector availability;
5. select/mention the Command Governor app for **this exact message** as the
   current ChatGPT surface requires;
6. stage only the tiny opaque wake payload;
7. durably arm the Send ambiguity fence;
8. invoke the exact composer-local Send action once;
9. observe CDP/network/message-tree evidence;
10. persist `accepted`, `failed`, or `ambiguous`.

Wake text contains only opaque identifiers and the instruction to use Command
Governor. It contains no worker output, source code, prompt, cwd, transcript,
tool arguments, secrets, or GitHub credentials.

### Observation path

Prefer CDP evidence:

- exact Target/Page/frame identity;
- navigation/redirect events;
- request initiation and response/stream lifecycle;
- user-message/conversation identities visible in the real SPA flow;
- physical assistant-turn start/settlement.

DOM remains necessary for structural control (composer, app selection, Send) and
fallback observation. Weak signals such as composer emptied, URL changed, Stop
appeared, or assistant began do not promote a wake to accepted by themselves.

Narrow authenticated reads/passive network interpretation may assist
reconciliation when proven robust. They are observation/optimization only. The
project does not implement a protected private ChatGPT write client.

See [browser transport](browser-transport.md).

## Browser delivery safety model

Each wake revision has deterministic identity, conceptually:

```text
delivery_id = H("command-governor/wake/v1",
                obligation_id,
                binding_generation,
                delivery_revision)
```

The delivery row also snapshots the exact target obligation version/source event.
If that target becomes stale before Send, the wake cannot submit.

A transaction records attempt `claimed` **before any browser I/O**. Immediately
before the exact Send action, another transaction re-verifies binding/obligation
and records `activation_armed`, the external-I/O ambiguity fence.

Terminal outcomes:

- **failed** — proof submission did not happen; bounded retry is safe;
- **accepted** — strong semantic evidence proves the intended wake was submitted
  to the exact bound conversation;
- **ambiguous** — submission may have happened but cannot be proved.

Startup converts previous-process nonterminal `claimed`/`activation_armed` to
`ambiguous` before browser recovery. This may conservatively quarantine a zero-send
crash; that is preferable to an automatic duplicate.

`accepted` and `ambiguous` are never automatically resent. An ambiguous delivery
may only be promoted to accepted by exact reconciliation evidence.

## ChatGPT settlement is not ACK

Keep three facts separate:

1. browser delivery accepted;
2. physical ChatGPT assistant turn settled;
3. foreman explicitly processed and ACKed the obligation through MCP.

Only (3) closes normal processed work.

If (2) occurs without (3), the obligation stays open. After a bounded policy delay
with no overlapping/unknown ChatGPT turn, Command Governor may create a **new
delivery revision** for the same obligation. Resumes are capped; budget exhaustion
creates durable `foreman_unreachable` while preserving the obligation indefinitely.

## MCP contract

Use the official Rust SDK (`rmcp`). Keep the ChatGPT-facing ABI deliberately small
because configured apps/conversations can retain tool schemas and current ChatGPT
app changes require refresh/action availability.

V1 surface:

- `foreman_bootstrap` — read-only health/outstanding-work discovery;
- `foreman_resume` — state-changing claim/fetch of one exact wake-correlated
  obligation/result/input;
- `foreman_ack` — explicit state-changing disposition; normal closure path;
- `foreman_answer_input` — structured state-changing answer that schedules a
  separately fenced worker continuation.

No general arbitrary-action dispatcher is exposed in V1.

All mutation tools require current binding generation and obligation/source/claim
fences. Because MCP does not currently provide a documented trustworthy ChatGPT
conversation identity as the caller principal, `foreman_resume` also requires the
accepted current wake's opaque `delivery_id`; bootstrap does not disclose that
nonce. Resume then mints the claim needed for ACK/input mutation.

### Current ChatGPT capability gate

Current published ChatGPT product availability means write-capable custom MCP
cannot be assumed on every plan/surface. `chatgpt bind` must execute a harmless
synthetic mutation preflight on the actual account/surface.

If state-changing actions are unavailable, V1 reports that combination
unsupported. It does **not** fake ACK from assistant settlement, DOM observation,
or a misleading read-only tool.

See [MCP contract](mcp-contract.md).

## SQLite persistence

Use `rusqlite` with bundled SQLite and one daemon-owned DB actor.

Initial policy:

- WAL;
- foreign keys enabled;
- bounded busy timeout;
- `synchronous=FULL` until crash injection justifies relaxing it;
- explicit migrations/schema epoch;
- source-event uniqueness and terminal-event dedupe;
- explicit compare-and-swap fences;
- no ORM;
- no browser cookies/tokens, terminal transcripts, raw tool args, or provider
  stream bodies in the ledger.

Async tasks use typed requests to the store actor rather than sharing arbitrary
write connections.

See [data model](data-model.md).

## Local IPC and secrets

CLI operations go through owner-local daemon IPC. Prefer a Unix domain socket on
macOS/Linux and a named pipe on Windows. A loopback HTTP fallback, if required,
uses an owner-local capability and strict loopback/origin handling.

The dedicated Chrome profile is credential-equivalent and owner-private. SQLite,
result artifacts, worker-host transport spools, hook inbox, logs, tunnel
credentials, and local control endpoints have distinct least-privilege paths.
Secrets never appear in command-line arguments or structured tracing fields.

## GitHub integration

GitHub remains engineering source of truth. Command Governor records opaque
repository/issue/commit/PR references and review evidence, not a shadow source-code
database.

GitHub content and worker results are untrusted input to the foreman. MCP responses
separate protocol/control fields from untrusted result/repository data. Text inside
a diff/issue/result cannot become Command Governor policy merely by instructing the
foreman to ACK or execute something.

## Startup recovery order

Before accepting new orchestration work, daemon startup must:

1. acquire the single-daemon state-root ownership lock;
2. validate filesystem ownership/permissions;
3. open SQLite and verify schema/migration/integrity policy;
4. convert orphaned browser/worker external-delivery claims that crossed their
   recovery ambiguity rules into explicit ambiguity before new I/O;
5. replay/validate materialized projections;
6. ingest/dedupe sanitized hook inbox events;
7. reconcile worker-host structured spools/exit receipts and publish only results
   whose terminal evidence/artifact durability can be proved;
8. verify every artifact required by an open obligation;
9. reconcile structured/native worker facts against runtime observations without
   letting stale Herdr `working` erase stronger evidence;
10. restore watchdog schedules without fabricating terminal state;
11. reconnect/supervise browser and supported MCP tunnel;
12. verify exact foreman binding/app/capability epoch before any wake;
13. resume only operations proven idempotent/safe.

Recovery never converts missing evidence into success.

## V1 explicit non-goals

- conventional GUI, Electron, Tauri, Dioxus, Iced, or menu-bar app;
- phone/email/Slack/Telegram/ntfy completion notifications;
- hosted multi-tenant orchestration authority;
- storing full terminal/provider transcripts or browser credentials in SQLite;
- private ChatGPT protected-write protocol emulation or challenge bypass;
- CAPTCHA/Turnstile/Sentinel/PoW/rate-limit/entitlement circumvention;
- exactly-once claims over external interfaces that do not provide transactional
  idempotency;
- multiple simultaneously active foreman conversations;
- treating Herdr idle/screen state as stronger truth than a confirmed worker
  protocol result;
- treating a Claude Stop-hook callback alone as completion;
- permitting a worker to independently approve its own implementation.

## Architecture gates before implementation claims

The pure domain/store/testkit foundation can proceed after architecture review, but
the end-to-end V1 must not be called supported until these live gates pass.

### Gate A — MCP mutation capability

The exact target ChatGPT account/surface must invoke genuine state-changing
`foreman_resume`/`foreman_ack`/input actions through the supported connector path.
If unavailable, preserve the architecture and mark that platform combination
unsupported.

### Gate B — browser transport spike

A dedicated headed Chrome profile must pass the spike in
[browser-transport.md](browser-transport.md): exact binding, per-message app
selection, ten unique wakes, strong accepted evidence, crash-at-Send ambiguity, no
replay, restart, MCP outage, and generation fencing. Headless is evaluated
separately and remains experimental unless equivalent.

### Gate C — Claude managed-execution conformance

The pinned Claude version/invocation must prove:

- structured `system/init`/capability handling;
- final structured `result` + child exit semantics;
- a parallel Stop hook can veto a stop without producing false completion;
- actual settings/hook sources active under the chosen CLI flags;
- confirmed `AskUserQuestion`/policy defer and same-session resume behavior;
- daemon-offline worker-host spool/exit recovery;
- stale Herdr `working` cannot block a confirmed final/deferred worker state;
- forbidden prompt/tool/cwd/transcript data does not leak to the safe ledger/logs.

If this gate fails, change the Claude adapter—not the central obligation or ACK
invariant.

## Recommendation

The control-plane architecture is suitable for a small Phase 1 Rust
core/store/testkit scaffold once this corrected architecture is reviewed. The
ChatGPT and Claude service adapters remain gated capabilities, not assumptions.
