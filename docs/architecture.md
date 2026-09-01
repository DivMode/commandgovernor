# Command Governor V1 architecture

Status: **proposed implementation architecture — independently reviewed, live adapter gates remain**  
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

## Current ChatGPT capability gate

The architecture requires state-changing MCP operations for claim, ACK, and input
answers. Published OpenAI plan documentation is recorded as compatibility evidence,
but ADR 0006 establishes the support rule: **feature-test the exact bound
account/app/surface instead of inferring capability from a plan label**.

On 2026-08-31 the target ChatGPT Pro account/app/surface successfully performed
state-changing Tandem MCP operations and verified a host-filesystem mutation by
read-back. That disproved the earlier categorical plan-name assumption for this
actual target surface.

Therefore:

- the Rust kernel/store/testkit proceeds independently of any ChatGPT product
  assumption;
- `command-governor chatgpt bind` must execute a harmless synthetic
  mutation/read-back on the exact account/app/surface and record a
  `capability_epoch`;
- plan/workspace/model labels remain diagnostic metadata only;
- capability is revalidated after connector/account/product/ABI changes or drift;
- no browser signal, assistant settlement, or mislabeled read tool may substitute
  for explicit ACK.

This is a capability gate, not a reason to weaken the invariant.

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
                              ├── parses structured output online
                              ├── durable bounded final-result candidate only
                              └── sanitized run + child-exit receipts
```

There is one orchestration authority: the daemon. The Secure MCP Tunnel and Claude
worker-host are transport children with **zero task/obligation/binding authority**.
The worker-host exists so Claude can finish while the daemon is restarting without
losing the final reviewable result. It does **not** persist the complete provider
stream.

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

1. **managed-run staging** — the worker-host parses Claude structured output online
   and persists only sanitized lifecycle/run receipts plus, when a complete final
   result arrives, one bounded final-result candidate. Raw stream records, prompt
   text, tool arguments/results, commands, cwd, transcript paths, and secrets are
   never durably spooled;
2. **immutable result artifact** — the bounded final worker result required for
   review, referenced by SQLite and pinned while any open obligation needs it.

The result artifact is made durable **before** the transaction publishes
`completed_unprocessed`. Full terminal/provider transcripts do not belong in the
SQLite event ledger or any worker-host spool. GitHub commit/PR refs are
complementary evidence.

## Managed Claude lifecycle

### Stop hook is candidate, not completion

Current Claude Code allows multiple matching hooks to run in parallel, and a
`Stop` hook can block stopping. Therefore our Stop callback is only
`stop_candidate` evidence.

Preferred managed V1 uses Claude's non-interactive programmatic interface with
structured output. Successful terminal proof is:

1. complete final structured `result` for the exact fenced run; plus
2. matching child-command/process exit receipt from the worker-host.

The worker-host parses this stream online and durably retains only the bounded
final-result candidate plus sanitized receipts. It does not persist intermediate
provider records.

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

For `AskUserQuestion` and other out-of-band decisions, current documented
`PreToolUse` deferral is the preferred durable pause mechanism **only when exactly
one tool call is being processed and conformance proves the tool shape can be
resumed**. Current Claude documentation says `defer` in `-p` is ignored when
multiple tool calls are emitted together; that case must not be projected as a
clean `needs_input` pause.

Project `needs_input` only after the structured managed-run result confirms the
defer actually took effect (`tool_deferred`/equivalent current structured proof).

Current Claude documentation also says `PermissionRequest` hooks **can run in
non-interactive sessions that cannot show a prompt**; if no hook decides, the tool
is denied. In current hook input, `PermissionRequest` includes the tool name/input
but does not carry the same `tool_use_id` fence as `PreToolUse`, so V1 treats it as
a permission-decision signal, not as a generic durable pause/resume identity.
`PreToolUse` remains the preferred exact tool-call policy/defer boundary when a
stable tool-use fence is needed.

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

Separate **idempotency identity** from **wake possession correlation**:

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)

delivery_id = CSPRNG(>=192 bits)   # opaque random correlation ID
```

`delivery_key` is a non-secret deterministic dedupe key and never authorizes a
foreman mutation. `delivery_id` is generated once when the delivery row is created,
persisted durably, carried in the browser wake, omitted from bootstrap/status, and
required by `foreman_resume` as an anti-confusion possession fence. Connector
authentication and all obligation/generation/version fences remain required; the
random ID is not sole authentication.

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
requires the random accepted wake `delivery_id`; bootstrap/status do not disclose
it. Resume mints the claim needed for later mutation.

Write-capable MCP cannot be assumed from a plan name or from an earlier capability
epoch. Gate A feature-tests the exact bound account/app/surface with a harmless
state mutation/read-back, stale-generation rejection, and confirmation
characterization. The target Pro surface demonstrated state-changing Tandem MCP on
2026-08-31, while any surface whose current probe fails remains unsupported without
fake ACK, browser-inferred ACK, or a mutation mislabeled as read-only.

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
  provider streams in the ledger or durable worker staging.

See [data model](data-model.md).

## Local IPC / secrets and V1 trust boundary

CLI uses owner-local IPC: Unix socket on macOS/Linux, named pipe on Windows where
implemented. Loopback HTTP is only a fallback with an owner-local capability.

Chrome profile, result artifacts, managed-run staging, hook inbox, database, logs,
and tunnel credentials have separate private paths. Secrets do not go in argv or
structured logs when a safer mechanism exists.

V1's local administrative trust root is the OS user account. A Claude/tool process
running as that same user is **not sandbox-contained by owner-only file modes** and
could maliciously tamper with Governor state if fully compromised or deliberately
hostile. Command Governor minimizes paths/capabilities exposed to workers and its
own worker-host code writes only allocated staging paths, but this is an
application boundary, not same-user OS isolation. Hostile-worker containment would
require a future separate OS identity/sandbox/broker and is not claimed by V1.

Managed worker environments therefore receive only the opaque correlation values
needed by hooks; the full Command Governor state-root path is not intentionally
exported to Claude as a generic environment variable.

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
7. reconcile managed-run final-result candidates/run+exit receipts and publish
   only proven results;
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
- full terminal/provider transcript database or raw provider-stream spool;
- direct protected private ChatGPT write protocol;
- CAPTCHA/Turnstile/Sentinel/PoW/rate/entitlement bypass;
- multiple active foreman conversations;
- exactly-once claims from external interfaces without idempotency;
- Herdr idle/screen state as semantic completion truth;
- Claude Stop callback alone as completion;
- worker self-approval;
- hostile same-user worker/process containment.

## Live gates

### Gate A — ChatGPT MCP mutation

The exact bound ChatGPT account/app/surface must prove the state-changing foreman
tool class with a harmless synthetic mutation/read-back, stale-generation
rejection, tool-mount characterization, and current confirmation behavior. The
support decision is fenced by `capability_epoch`; plan name is diagnostic only.

The target Pro surface already demonstrated state-changing Tandem MCP on
2026-08-31, but that is compatibility evidence rather than a permanent entitlement
or a substitute for the Command Governor preflight. If writes become unavailable,
the surface is unsupported for that epoch and obligations stay open without
weakening ACK.

### Gate B — headed Chrome/CDP

Dedicated headed Chrome must prove exact binding, per-message app selection,
10/10 unique wakes, semantic accepted evidence, crash-at-Send ambiguity/no replay,
restart, MCP outage, and generation fencing. Headless is separate/experimental.

### Gate C — Claude managed execution

Pinned Claude invocation must prove:

- structured init/capabilities;
- final structured result + child exit while raw intermediate stream records are
  not persisted;
- parallel Stop-hook veto without false completion;
- actual settings/hook-source behavior;
- confirmed AskUserQuestion/PreToolUse single-tool defer + `tool_deferred`
  structured result + same-session resume;
- multi-tool defer is detected as unsupported and never projected as clean pause;
- non-interactive `PermissionRequest` behavior and its weaker correlation are
  handled exactly as the pinned release documents;
- daemon-offline worker-host final-result/receipt recovery;
- stale Herdr working reconciliation;
- forbidden-data non-persistence.

## Recommendation

The architecture is suitable for a **small pure Rust core/store/testkit Phase 1
scaffold after this architecture PR is reviewed/accepted**. ChatGPT and Claude
service adapters remain gated capabilities, not assumptions. End-to-end V1 foreman automation targets any exact ChatGPT surface that
passes the current ADR-0006 capability epoch; plan name alone neither admits nor
excludes the surface.
