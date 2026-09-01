# Command Governor roadmap

Milestones are outcomes and gates, not calendar promises. V1 advances when failure
modes are proven safe, not when a large amount of code exists.

## Phase 0 — verified architecture

Deliver:

- pinned current-source research;
- architecture/decision records;
- independent reviewer-of-record report;
- threat/security model;
- Rust workspace proposal;
- SQLite/data model;
- obligation/browser/worker/binding state machines;
- stable MCP contract;
- browser-control and private-API/DOM/hybrid comparison;
- Claude structured lifecycle/input/watchdog contract;
- result-artifact and worker-host transport boundaries;
- licensing/provenance plan;
- phased implementation plan;
- deterministic crash/failure/security acceptance tests.

Exit criteria:

- no normal state transition can close delegated work without explicit disposition;
- every consequential external write has an ambiguity boundary;
- a worker result can survive daemon/runtime restart until ACK;
- Claude Stop-hook veto behavior cannot create false completion;
- Gate A is capability-based: the exact ChatGPT account/app/surface must pass a
  harmless mutation/read-back probe for the current capability epoch, while plan
  name remains diagnostic metadata only;
- deterministic browser dedupe identity is separate from random wake-correlation
  possession fencing;
- no complete Claude/provider stream must be durably spooled for correctness;
- same-user file permissions are not described as hostile-worker containment;
- correctness does not depend on a GUI or human completion notification;
- the acceptance suite can be implemented without weakening the central invariant.

## Phase 1 — pure Rust kernel + store + testkit

After the architecture PR is accepted:

- scaffold the Cargo workspace with pinned stable Rust/edition 2024;
- implement opaque typed IDs, source/domain events, pure state machines, policies;
- implement explicit external-effect classes and write-ahead intent state before consequential I/O;
- implement stable mutation command identity with completed-result replay but uncertain-result no-replay semantics;
- implement incarnation/lease fencing for resources that require exclusive ownership;
- implement deterministic browser `delivery_key` plus CSPRNG random `delivery_id`;
- implement `rusqlite` single-writer DB actor;
- implement schema epoch/migrations and replayable projections;
- implement immutable private result-artifact store;
- implement owner-root daemon lock/local IPC skeleton;
- implement deterministic fake clock/runtime/browser/foreman/worker lifecycle;
- establish fmt/clippy/test/audit/deny CI from the first Rust commit.

Exit criteria:

- pure obligation/persistence/browser-delivery state tests pass;
- restart preserves every open obligation;
- duplicate source events are idempotent;
- stale generation/claim fences fail closed;
- deterministic delivery metadata cannot derive random wake correlation;
- result-artifact crash ordering passes;
- projection replay equivalence passes;
- kill-after-intent and kill-after-I/O-before-result both produce durable ambiguity/reconciliation with zero automatic replay;
- a repeated completed mutation identity returns its recorded result while a repeated pending/uncertain identity never redispatches;
- stale lease token/process incarnation/daemon epoch cannot mutate or release current ownership;
- forbidden-data scan is clean.

No real ChatGPT or Claude is required for this phase.

## Phase 2 — Claude structured worker transport + Herdr adapter

- implement `command-governor worker-host claude <opaque-turn-id>` as a
  transport-only Rust mode;
- launch/resume managed `claude -p` with structured output;
- parse structured provider output online;
- persist only sanitized managed-run receipts, one bounded complete final-result
  candidate, and sanitized child-exit receipt;
- explicitly **do not persist the complete provider stream**;
- implement hardened Command Governor-owned Claude settings/hook command;
- implement sanitized durable hook inbox with narrow per-turn locator rather than
  exporting the general state root;
- normalize structured init/result, Stop candidate, StopFailure, SessionEnd,
  progress, PermissionRequest decisions, and confirmed defer/resume evidence;
- treat Stop callbacks as candidate evidence only;
- implement `PreToolUse` policy/defer path for exact out-of-band input when the
  current single-tool shape supports it;
- treat multi-tool defer as unsupported/reconciliation rather than clean pause;
- implement current non-interactive `PermissionRequest` semantics without
  pretending it has a tool-use identity it does not expose;
- implement Herdr process/session adapter as lower-level runtime evidence;
- implement runtime conflict/clear-busy reconciliation;
- implement worker continuation delivery ambiguity semantics;
- promote only a validated bounded final-result candidate into the immutable
  result artifact before completion publication.

Exit criteria for deterministic implementation tests:

- Stop candidate alone never creates completion;
- another fake Stop hook can veto and Claude continues without false terminal state;
- structured final result + child exit creates exactly one result obligation;
- truncated/missing final result/exit fails visibly;
- daemon-offline final-result candidate/run/exit receipts and hook-inbox recovery
  are exactly-once/idempotent;
- raw `tool_use`/`tool_result`/prompt/command/intermediate provider records are not
  durably persisted;
- confirmed single-tool defer creates `needs_input`; multi-tool defer does not;
- non-interactive PermissionRequest is not discarded by obsolete assumptions;
- stale Herdr working cannot veto a confirmed structured final/deferred state;
- personal Claude settings remain untouched;
- general Command Governor state-root is not intentionally exported to Claude;
- raw prompt/tool/cwd/transcript/provider-stream fields do not leak into safe
  persistence/logs.

## Gate C — live Claude managed-execution conformance

Run a disposable real Claude/Herdr matrix on the exact pinned versions before the
Claude adapter is called supported:

- record exact Claude Code release/main context; current research snapshot found
  `v2.1.252` on 2026-08-31;
- structured `system/init` and capability feature detection;
- normal final structured `result` + matching child exit;
- provider stream containing tool-use/tool-result sentinels is parsed but not
  durably spooled;
- controlled parallel Stop-hook `decision:block` case followed by continued work;
- exactly one terminal result only after the later true final result;
- StopFailure/SessionEnd failure semantics;
- actual active settings/hook source behavior for the selected CLI invocation;
- confirmed single-tool AskUserQuestion/policy defer + same-session resume;
- multi-tool defer is observed as ignored/unsupported and never becomes clean
  `needs_input`;
- non-interactive `PermissionRequest` behavior and its current correlation fields
  are measured;
- daemon killed while worker continues -> bounded final-result candidate/run/exit
  survives without a raw stream spool;
- stale Herdr `working / idle:false` conflict;
- forbidden-data byte scan;
- managed worker environment does not intentionally expose general Governor state
  root or secrets.

A false completion caused by a Stop-hook callback or a raw provider-stream leak
fails Gate C.

## Phase 3 — stable MCP server + supported tunnel

- implement the four-tool `rmcp` ABI;
- implement low-information bootstrap;
- implement accepted random-wake correlation and claim fencing;
- implement result paging;
- implement explicit ACK/disposition and input-answer policy;
- integrate current OpenAI-supported Secure MCP Tunnel/connectivity without a
  second state authority;
- implement `doctor` ABI/capability checks.

Exit criteria:

- unrelated/stale fake connector cannot claim current work from bootstrap or
  deterministic scheduling metadata alone;
- exact accepted random delivery ID + current fences can claim;
- exact repeated ACK is idempotent, conflicting stale ACK rejected;
- tunnel/shim restart cannot close/lose obligations;
- all deterministic MCP fencing/security tests pass.

## Gate A — ChatGPT MCP mutation capability

Research and live evidence on 2026-08-31 establish two separate facts:

- published OpenAI plan documentation remains compatibility evidence and may not
  match every account/app/surface behavior;
- the exact target ChatGPT Pro surface successfully performed state-changing
  Tandem MCP operations and verified the resulting host-filesystem mutation.

ADR 0006 therefore makes Gate A **capability-based, not plan-name-based**.

For every candidate bound account/app/surface:

- record exact plan/workspace/model/date as diagnostic metadata;
- install/refresh the exact V1 Command Governor connector ABI;
- verify the app/tools are mounted for the message;
- perform a harmless synthetic state mutation and read it back from the MCP
  server;
- prove the mutation is correlated to the exact test record;
- prove stale binding generation is rejected;
- characterize confirmation/permission behavior and whether the legitimate
  model-driven flow can complete it without bypass;
- record a `capability_epoch` and revalidate it after relevant connector,
  account/workspace, product, or ABI changes and on repeated action rejection.

Keep `app_tools_not_mounted`, `write_action_unavailable`,
`write_action_rejected`, `confirmation_required`, `connector_unreachable`, and
`connector_abi_mismatch` as distinct failure classes. If the current probe fails,
leave obligations open and record the unsupported capability. Do not fake ACK,
mislabel a write as a read, or substitute browser/assistant settlement.

## Phase 4 — generic Rust browser/CDP foundation

- implement `governor-browser` trait and `chromiumoxide` driver;
- launch/attach dedicated headed Chrome profile;
- target/page/network event plumbing;
- browser process/profile ownership/restart supervision;
- staged route/composer readiness diagnostics;
- no protected private ChatGPT write logic.

Exit criteria:

- fake target/CDP crash tests pass;
- no browser I/O precedes durable delivery claim;
- random wake-correlation identity and deterministic dedupe key stay separate;
- profile ownership and secret-redaction tests pass.

## Gate B — authenticated headed ChatGPT browser spike

Only after Gate A identifies a write-capable foreman surface, use a disposable
bound conversation and fake local obligations:

- normal login/profile persistence;
- exact `/c/<id>` binding and wrong-route fencing;
- Command Governor app selection for each message;
- 10/10 unique random wake submissions;
- strong network/message accepted evidence;
- pre-submit failure;
- crash after claim and around exact Send activation;
- ambiguity reconciliation without replay;
- physical settlement != ACK;
- bounded resume revision with a new random delivery ID;
- random correlation cannot be reconstructed from bootstrap/deterministic fields;
- rebind-generation fencing;
- browser restart;
- MCP outage;
- separate `--headless=new` comparison with no stealth/challenge bypass.

A duplicate Send fails Gate B.

## Phase 5 — `governor-chatgpt-web`

After Gate B has frozen the required evidence contract sufficiently:

- exact binding verification;
- current message-scoped app selection;
- tiny wake containing opaque obligation + random delivery IDs only;
- durable `claimed` + `activation_armed` fence;
- CDP semantic accepted evidence;
- passive/direct read reconciliation where robust;
- physical assistant-turn observation;
- bounded settled-without-ACK resume revisions;
- all private endpoint/schema observations confined to this replaceable crate.

Exit criteria:

- deterministic browser-delivery suite passes;
- Gate B passes on supported headed platform;
- accepted/ambiguous never automatically replay;
- same delivery revision cannot physically submit twice.

## Phase 6 — end-to-end durable foreman loop

Prove on a surface that passed Gates A/B/C:

```text
managed worker
  -> confirmed terminal/input evidence
  -> durable result/input obligation
  -> browser wake accepted
  -> exact ChatGPT foreman turn
  -> MCP resume/fetch
  -> independent review/action
  -> explicit ACK or structured input answer
  -> obligation closes/resumes only on fenced evidence
```

Inject daemon/browser/runtime/worker-host/worker/tunnel restarts at every boundary.

Exit criteria:

- no lost result/input obligation;
- no duplicate browser or worker continuation;
- no stale-generation ACK;
- no work considered complete before independent disposition;
- `status`, `obligations`, and `doctor` explain every unresolved condition.

## Phase 7 — GitHub engineering integration

- project/repository binding;
- issue/commit/PR refs and review evidence;
- source refs attached to obligations/results;
- least-privilege authentication path;
- prompt-injection separation between GitHub data and Governor control fields.

GitHub remains engineering truth; the local DB stores references/provenance, not a
shadow repository.

## Phase 8 — Codex and additional adapters

- Codex worker lifecycle adapter;
- additional runtimes/workers only when they satisfy the same fencing/durability
  contract;
- capability/support matrix;
- adapter conformance required before support claim.

Provider additions do not redefine obligation or browser semantics.

## Phase 9 — public V1 hardening

- migration/backup/restore tooling;
- local IPC/profile/artifact/hook/managed-run staging security audit;
- explicit documentation that same-user workers are not sandbox-contained;
- crash/failpoint extended CI;
- dependency/license/source review;
- signed/reproducible release strategy where practical;
- macOS first-class packaging;
- Linux/Windows support matrix;
- documented unsupported ChatGPT/Claude combinations;
- exact provenance/third-party notices.

## Later, not V1 correctness requirements

- optional menu-bar/status UI as a daemon client;
- multi-machine control plane;
- multiple simultaneous foreman bindings with explicit routing policy;
- team/multi-user authorization;
- stronger worker OS sandboxing / separate execution identity;
- official foreman/wake provider APIs when available.

A future UI never becomes lifecycle authority. Human completion notifications
remain optional operator convenience, not the primary wake design.
