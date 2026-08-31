# Command Governor roadmap

Milestones are outcomes and gates, not calendar promises. V1 advances when failure
modes are proven safe, not when a large amount of code exists.

## Phase 0 — verified architecture

Deliver:

- pinned current-source research;
- architecture/decision records;
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
- correctness does not depend on a GUI or human completion notification;
- reviewers agree the acceptance suite can be implemented without weakening the
  central invariant.

## Phase 1 — pure Rust kernel + store + testkit

After architecture review:

- scaffold the Cargo workspace with pinned stable Rust/edition 2024;
- implement opaque typed IDs, source/domain events, pure state machines, policies;
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
- result-artifact crash ordering passes;
- projection replay equivalence passes;
- forbidden-data scan is clean.

No real ChatGPT or Claude is required for this phase.

## Phase 2 — Claude structured worker transport + Herdr adapter

- implement `command-governor worker-host claude <opaque-turn-id>` as a
  transport-only Rust mode;
- launch/resume managed `claude -p` with structured output;
- capture a private bounded provider stream spool and sanitized child-exit receipt;
- implement hardened Command Governor-owned Claude settings/hook command;
- implement sanitized durable hook inbox;
- normalize structured init/result, Stop candidate, StopFailure, SessionEnd,
  progress, permission, and confirmed defer/resume evidence;
- treat Stop callbacks as candidate evidence only;
- implement `PreToolUse` policy/defer path for out-of-band input where supported;
- implement Herdr process/session adapter as lower-level runtime evidence;
- implement runtime conflict/clear-busy reconciliation;
- implement worker continuation delivery ambiguity semantics;
- extract only the bounded final result from a confirmed worker-host spool into the
  immutable result artifact before completion publication.

Exit criteria for deterministic implementation tests:

- Stop candidate alone never creates completion;
- another fake Stop hook can veto and Claude continues without false terminal state;
- structured final result + child exit creates exactly one result obligation;
- truncated/missing stream/exit fails visibly;
- daemon-offline spool and hook-inbox recovery are exactly-once/idempotent;
- stale Herdr working cannot veto a confirmed structured final/deferred state;
- personal Claude settings remain untouched;
- raw prompt/tool/cwd/transcript/provider-stream fields do not leak into safe
  persistence/logs.

## Gate C — live Claude managed-execution conformance

Run a disposable real Claude/Herdr matrix on the exact pinned versions before the
Claude adapter is called supported:

- structured `system/init` and capability feature detection;
- normal final structured `result` + matching child exit;
- controlled parallel Stop-hook `decision:block` case followed by continued work;
- exactly one terminal result only after the later true final result;
- StopFailure/SessionEnd failure semantics;
- actual active settings/hook source behavior for the selected CLI invocation;
- confirmed AskUserQuestion/policy defer + same-session resume;
- unsupported multi-tool defer shape;
- daemon killed while worker continues -> worker-host result/exit survives;
- truncated worker-host spool -> no fabricated completion;
- stale Herdr `working / idle:false` conflict;
- forbidden-data byte scan.

A false completion caused by a Stop-hook callback fails Gate C.

## Phase 3 — stable MCP server + supported tunnel

- implement the four-tool `rmcp` ABI;
- implement accepted-wake correlation and claim fencing;
- implement result paging;
- implement explicit ACK/disposition and input-answer policy;
- integrate current OpenAI-supported Secure MCP Tunnel/connectivity without a
  second state authority;
- implement `doctor` ABI/capability checks.

Exit criteria:

- unrelated/stale fake connector cannot claim current work from bootstrap alone;
- exact repeated ACK is idempotent, conflicting stale ACK rejected;
- tunnel/shim restart cannot close/lose obligations;
- all deterministic MCP fencing/security tests pass.

## Gate A — ChatGPT MCP mutation capability

For each declared supported ChatGPT plan/surface:

- install/configure the V1 Command Governor app through the supported path;
- verify connector ABI/tool visibility;
- perform a synthetic harmless mutation from the actual ChatGPT surface;
- prove state-changing resume/ACK/input-action class tools are available;
- prove stale binding generation fails closed.

If the target account/surface is read/fetch-only under current product policy,
record `write_capability_unavailable`. Do not fake ACK or mislabel a write as a
read.

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
- profile ownership and secret-redaction tests pass.

## Gate B — authenticated headed ChatGPT browser spike

Use a disposable bound conversation and fake local obligations:

- normal login/profile persistence;
- exact `/c/<id>` binding and wrong-route fencing;
- Command Governor app selection for each message;
- 10/10 unique wake submissions;
- strong network/message accepted evidence;
- pre-submit failure;
- crash after claim and around exact Send activation;
- ambiguity reconciliation without replay;
- physical settlement != ACK;
- bounded resume revision;
- rebind-generation fencing;
- browser restart;
- MCP outage;
- separate `--headless=new` comparison.

A duplicate Send fails Gate B.

## Phase 5 — `governor-chatgpt-web`

After Gate B has frozen the required evidence contract sufficiently:

- exact binding verification;
- current message-scoped app selection;
- deterministic tiny wake;
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

Prove:

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
- local IPC/profile/artifact/hook/spool security audit;
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
- stronger worker OS sandboxing;
- official foreman/wake provider APIs when available.

A future UI never becomes lifecycle authority. Human completion notifications
remain optional operator convenience, not the primary wake design.
