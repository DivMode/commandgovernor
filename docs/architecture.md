# Command Governor V1 architecture

Status: **proposed implementation architecture**  
Research snapshot: [2026-08-31 technology review](research/2026-08-31-technology-review.md)

## Mission and invariant

Command Governor is a local-first durable control plane for AI/software-engineering
workers. It preserves **what work is still owed** when browsers, foremen, workers,
runtimes, or Command Governor restart.

> Delegated work remains a durable obligation until the bound foreman has fetched
> the real result or input request, performed the required review/action, and
> explicitly ACKed a fenced obligation. Worker completion, browser delivery, and
> ChatGPT assistant-turn settlement do not close it.

V1 is a Rust daemon + CLI. No conventional GUI and no human completion-notification
subsystem is part of correctness.

## Authority model

| Component | Authoritative for | Not authoritative for |
| --- | --- | --- |
| ChatGPT Web foreman | explicit review/disposition decisions | worker/process liveness, global lifecycle truth |
| Claude/Codex/future workers | structured/native worker protocol facts and produced work | whether work was independently reviewed |
| GitHub | issues, commits, PRs, reviews | transient worker/input/browser state |
| Herdr/runtime | process/session transport facts | semantic completion when stronger worker evidence disagrees |
| Command Governor | event order, projections, obligations, delivery ambiguity, bindings, ACK validity | repository correctness or user-owned authorization decisions |

Provider status strings and hook callbacks are evidence. Their names/timestamps do
not automatically define terminal domain state.

## Process boundary

```text
                                   dedicated authenticated Chrome profile
                                                │
                                                ▼
                                    headed Chrome / Chromium
                                                ▲
                                                │ CDP
┌───────────────────────────────────────────────┴─────────────────────────────────┐
│                         command-governor daemon                                 │
│                                                                                  │
│  lifecycle core ◄──► single-writer SQLite        private result artifacts       │
│       │                                                                          │
│       ├── worker adapters (Claude/Codex)                                          │
│       ├── runtime adapters (Herdr/future)                                        │
│       ├── governor-chatgpt-web (browser/CDP evidence)                            │
│       ├── rmcp MCP server ── supported OpenAI tunnel ── ChatGPT app              │
│       └── GitHub adapter                                                         │
└──────────────────────────────────────────────────────────────────────────────────┘

command-governor CLI ── owner-local IPC ──► daemon

Herdr/session runtime ──► command-governor worker-host claude <opaque-turn-id>
                              ├── launches/resumes `claude -p`
                              ├── private bounded structured-stream spool
                              └── sanitized child-exit receipt
```

There is one orchestration authority: the daemon. The Secure MCP Tunnel and Claude
worker-host are transport children with **zero task/obligation/binding authority**.
The worker-host exists so Claude can finish while the daemon is restarting without
losing the provider's final structured result.

## Rust workspace proposal

After architecture acceptance:

```text
crates/
  governor-core/
  governor-store-sqlite/
  governor-runtime/
  governor-runtime-herdr/
  governor-worker-claude/
  governor-worker-codex/
  governor-browser/
  governor-chatgpt-web/
  governor-mcp/
  governor-github/
  governor-daemon/
  governor-testkit/
  command-governor/
```

`governor-core`, `governor-store-sqlite`, and `governor-chatgpt-web` remain hard
boundaries. An official future OpenAI foreman/wake API should replace the ChatGPT
Web crate without rewriting the durable kernel.

Initial candidate toolchain: stable Rust 1.98.0, edition 2024; re-verify at the
scaffold commit. Domain crates use typed `thiserror`; `anyhow` is limited to
application/process boundaries.

## Durable truth model

Store immutable source/domain events plus replayable projections, not a few
booleans.

Important identities:

- project/task;
- session + session incarnation;
- turn;
- source event + source fence;
- result artifact;
- obligation + version;
- input request;
- foreman binding + monotonic binding generation;
- browser delivery + revision;
- foreman claim.

A session name is display metadata, never an identity fence. Event order comes from
the daemon-assigned SQLite sequence. Projection mismatch on startup fails closed.

See [data model](data-model.md).

## Obligation lifecycle

```text
created
  │
  ▼
running ───────────────► needs_input
  │                         │
  │                         └── answer + confirmed worker resume ─► running
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
`suspected_stall` is non-terminal attention only.

ACK requires exact obligation version/source event/current binding generation and
foreman claim. Old conversations/generations cannot close current work.

See [state machines](state-machines.md).

## Result durability

A terminal obligation is useless if the actual result still lives only in a PTY.

V1 uses two distinct private content boundaries:

1. **worker-host spool** — temporary sensitive structured provider output that can
   survive daemon restart; owns no lifecycle authority;
2. **immutable result artifact** — bounded final worker result required for review,
   referenced by SQLite and pinned while any open obligation needs it.

The result artifact is made durable **before** the transaction publishes
`completed_unprocessed`. Full terminal/provider transcripts do not belong in the
SQLite event ledger. GitHub commit/PR refs are complementary evidence.

## Managed Claude lifecycle

### Stop hook is candidate, not completion

Current Claude Code allows multiple matching hooks to run in parallel, and a
`Stop` hook can block stopping. Therefore our Stop callback is only
`stop_candidate` evidence.

Preferred managed V1 uses Claude's non-interactive programmatic interface with
structured output. Successful terminal proof is:

1. complete final structured `result` for the exact fenced run; plus
2. matching child-command/process exit receipt from the worker-host.

`StopFailure`, `SessionEnd`, structured failure, interrupt, and process loss are
separate evidence classes with explicit adapter rules. A stale Herdr `working`
sample cannot override a confirmed structured final/deferred state; a lone Stop
candidate cannot claim such authority either.

### Hooks and settings

Managed hooks deposit sanitized lifecycle envelopes to an owner-private durable
inbox before returning so daemon restart cannot lose progress/input/native
observations. The inbox never stores prompt text, raw tool arguments, commands,
cwd, transcript paths, terminal transcripts, or full provider output.

Command Governor never edits personal Claude settings. Current Claude settings can
merge hooks across scopes, so `--settings` is not assumed to isolate all
user/project/plugin hooks. Gate C must measure the actual settings/hook behavior of
the chosen invocation.

### Input and permission in preferred `claude -p`

For `AskUserQuestion` and out-of-band tool decisions, prefer current documented
`PreToolUse` deferral where conformance proves the exact tool shape is safely
resumable. Project `needs_input` only after the structured managed-run result
confirms the defer actually took effect.

Current Claude documentation says **`PermissionRequest` hooks do not fire in
non-interactive `-p` mode**. Therefore managed V1 does not depend on
`PermissionRequest`; automated permission policy is implemented at `PreToolUse`.
An interactive/future Claude adapter may use PermissionRequest only after separate
conformance.

High-risk, destructive, credential-sensitive, materially broader, or unknown
requests stay user-owned. A worker-generated request cannot widen authority, and a
hook allow cannot be assumed to override Claude settings deny/ask rules.

See [worker lifecycle contract](worker-lifecycle.md).

## Watchdog

Use verified structured/native/tool progress, not screen repaint. Persist only
bounded safe metadata such as `last_progress_at` and event class.

No progress beyond threshold creates `suspected_stall`; it never fabricates
completion/failure or opens a monitor-only Claude session.

## ChatGPT binding

V1 has exactly one active foreman binding. Persist canonical `/c/<id>`, binding
generation, dedicated profile identity, and connector/capability epoch—never
"current tab," history, or most-recent conversation.

Existing binding verifies the exact resolved conversation before composer mutation.
New-chat binding commits only after a real `/c/<id>` exists. Rebinding increments
generation; old deliveries/claims cannot mutate new work.

## ChatGPT browser-backed hybrid

### Write path

The real authenticated ChatGPT SPA performs sensitive submission:

1. acquire single-flight browser worker;
2. verify exact bound conversation/profile;
3. verify target obligation version/source is still current;
4. verify/select the Command Governor app for this exact message;
5. stage only the tiny opaque wake;
6. durably arm Send ambiguity fence;
7. invoke exact composer-local Send once;
8. observe CDP/network/message evidence;
9. persist accepted/failed/ambiguous.

Wake text contains no worker output, code, prompt, cwd, transcript, tool arguments,
secrets, or GitHub credentials.

### Observation

Prefer CDP Target/Page/Network evidence: navigation, request/response/stream
lifecycle, provider user-message/conversation identity, and physical assistant
turn lifecycle. DOM is for structural control and fallback observation.

Composer emptied, URL changed, Stop appeared, or assistant started are not enough
alone to prove accepted delivery.

Narrow authenticated reads/passive interpretation may aid reconciliation. No
protected private ChatGPT write client or challenge bypass is implemented.

See [browser transport](browser-transport.md).

## Browser delivery semantics

Conceptual deterministic ID:

```text
delivery_id = H("command-governor/wake/v1",
                obligation_id,
                binding_generation,
                delivery_revision)
```

The delivery also snapshots target obligation version/source event.

- commit `claimed` before **any** browser I/O;
- immediately before exact Send, revalidate target/binding and commit
  `activation_armed`;
- `failed` only when submission is proven not to have happened;
- `accepted` only with strong exact message/conversation evidence;
- otherwise `ambiguous`.

Startup converts orphaned nonterminal claimed/armed attempts to ambiguous before
browser recovery. Accepted/ambiguous are never automatically resent. Exact
reconciliation may only promote ambiguous -> accepted.

## ChatGPT settlement is not ACK

Keep distinct:

1. browser delivery accepted;
2. physical ChatGPT turn settled;
3. explicit foreman processing + ACK through MCP.

Only (3) closes normal processed work. A settled/unACKed obligation may get a
bounded **new delivery revision** after policy backoff, never replay of the old
wake. Exhaustion creates `foreman_unreachable` while the obligation remains open.

## MCP contract

Use official Rust `rmcp` with a deliberately small stable V1 ABI:

- `foreman_bootstrap`
- `foreman_resume`
- `foreman_ack`
- `foreman_answer_input`

Resume/ACK/input answer are truthful mutations. Because MCP does not currently
supply a documented trustworthy ChatGPT conversation principal, resume also
requires the opaque accepted wake `delivery_id`; bootstrap does not disclose it.
Resume mints the claim needed for later mutation.

Current ChatGPT plan/surface capabilities mean write-capable MCP cannot be assumed.
`chatgpt bind` must feature-test the real account with a synthetic harmless
mutation. If writes are unavailable, mark that combination unsupported—do not fake
ACK from browser/assistant state or mislabel a mutation as read-only.

See [MCP contract](mcp-contract.md).

## SQLite

Use bundled `rusqlite` with one daemon-owned DB actor:

- WAL;
- foreign keys;
- bounded busy timeout;
- `synchronous=FULL` initially;
- explicit migrations/schema epoch;
- source-event uniqueness;
- compare-and-swap fences;
- no ORM;
- no browser cookies/tokens, raw tool args, terminal transcripts, or complete
  provider streams in the ledger.

See [data model](data-model.md).

## Local IPC / secrets

CLI uses owner-local IPC: Unix socket on macOS/Linux, named pipe on Windows where
implemented. Loopback HTTP is only a fallback with an owner-local capability.

Chrome profile, result artifacts, worker-host spools, hook inbox, database, logs,
and tunnel credentials have separate private paths. Secrets do not go in argv or
structured logs when a safer mechanism exists.

## GitHub

GitHub remains engineering source of truth. SQLite stores stable refs/provenance,
not a shadow source repository. GitHub content and worker results are untrusted
data; text in them cannot substitute for Governor policy or an MCP argument.

## Startup recovery order

Before new orchestration work:

1. acquire single-daemon state-root lock;
2. validate filesystem ownership/permissions;
3. open SQLite and verify schema/integrity/migrations;
4. quarantine orphaned browser/worker external deliveries according to ambiguity
   rules before new I/O;
5. replay/validate projections;
6. ingest/dedupe sanitized hook inbox;
7. reconcile worker-host spools/exit receipts and publish only proven results;
8. verify artifacts required by open obligations;
9. reconcile structured/native worker evidence against runtime observations;
10. restore watchdog schedules without fabricating terminal state;
11. reconnect/supervise browser and MCP tunnel;
12. verify exact foreman binding/app/capability before any wake;
13. resume only operations proven safe/idempotent.

Missing evidence never becomes success.

## V1 non-goals

- GUI/menu-bar app as correctness layer;
- human completion notifications;
- hosted multi-tenant authority;
- full terminal/provider transcript database;
- direct protected private ChatGPT write protocol;
- CAPTCHA/Turnstile/Sentinel/PoW/rate/entitlement bypass;
- multiple active foreman conversations;
- exactly-once claims from external interfaces without idempotency;
- Herdr idle/screen state as semantic completion truth;
- Claude Stop callback alone as completion;
- `PermissionRequest` as a managed `-p` input primitive;
- worker self-approval.

## Live gates

### Gate A — ChatGPT MCP mutation

Target ChatGPT plan/surface must prove state-changing foreman tools on the actual
account. If unavailable, the surface is unsupported without weakening ACK.

### Gate B — headed Chrome/CDP

Dedicated headed Chrome must prove exact binding, per-message app selection,
10/10 unique wakes, semantic accepted evidence, crash-at-Send ambiguity/no replay,
restart, MCP outage, and generation fencing. Headless is separate/experimental.

### Gate C — Claude managed execution

Pinned Claude invocation must prove:

- structured init/capabilities;
- final structured result + child exit;
- parallel Stop-hook veto without false completion;
- actual settings/hook-source behavior;
- confirmed AskUserQuestion/PreToolUse defer + same-session resume;
- managed permission decisions through PreToolUse rather than unavailable `-p`
  PermissionRequest hooks;
- daemon-offline worker-host recovery;
- stale Herdr working reconciliation;
- forbidden-data non-persistence.

## Recommendation

The architecture is suitable for a **small pure Rust core/store/testkit Phase 1
scaffold after this architecture PR is reviewed/accepted**. ChatGPT and Claude
service adapters remain gated capabilities, not assumptions.
