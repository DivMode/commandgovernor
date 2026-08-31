# Command Governor roadmap

Milestones are outcomes and gates, not calendar promises. V1 does not advance by
writing a large amount of code; it advances when the relevant failure modes are
proven safe.

## Phase 0 — verified architecture

Deliverables:

- current technology/source review with exact commits/releases/dates;
- accepted/proposed ADR set;
- threat/security model;
- Rust workspace proposal;
- SQLite/data model;
- obligation/delivery/binding state machines;
- stable MCP contract;
- worker lifecycle/input/watchdog contract;
- browser-control comparison and hybrid decision;
- authenticated browser spike protocol;
- licensing/provenance plan;
- deterministic acceptance tests.

Exit criteria:

- central invariant has no known state transition that closes work without ACK;
- every external non-idempotent write has an explicit ambiguity boundary;
- no required correctness property depends on a GUI or human completion alert;
- reviewers agree the acceptance tests can be implemented without redesigning the
  domain model.

## Gate A — ChatGPT MCP mutation capability

Before claiming the automatic foreman loop works on a target ChatGPT account:

- install/configure the V1 Command Governor app through the supported path;
- verify connector ABI/tool visibility;
- run a synthetic state-changing mutation through the real bound ChatGPT surface;
- prove `foreman_ack`/`foreman_answer_input` class actions are available;
- prove stale binding generation is rejected.

If the target account/surface is read/fetch-only under current product policy,
record `write_capability_unavailable`. Do not weaken the ACK invariant.

This gate can be developed in parallel with pure core work but must pass before
end-to-end V1 is called supported.

## Phase 1 — Rust kernel + store + testkit

Only after Phase 0 architecture acceptance:

- scaffold Cargo workspace with pinned stable Rust/edition 2024;
- implement typed IDs/domain events and pure state machines;
- implement `rusqlite` single-writer DB actor;
- implement migrations/schema epoch and replayable projections;
- implement private result-artifact store and crash-safe commit order;
- implement owner-local daemon lock and local IPC skeleton;
- implement deterministic fake clock/runtime/browser/foreman/lifecycle sources;
- enable fmt/clippy/test/audit/deny CI from first Rust commit.

Exit criteria:

- all pure obligation and persistence tests in `testing.md` pass;
- completion/result obligation survives forced daemon/runtime restart until ACK;
- duplicate source events are idempotent;
- projection replay equivalence holds;
- forbidden-data scan is clean.

No real ChatGPT or Claude is required for this phase.

## Phase 2 — Claude native lifecycle + Herdr runtime

- implement hardened managed Claude settings/hook command;
- implement owner-private durable hook inbox;
- normalize Stop/StopFailure/input/progress events;
- prefer tested non-interactive `AskUserQuestion` defer/resume path;
- implement Herdr runtime adapter as process/session evidence only;
- implement runtime conflict/clear-busy reconciliation;
- implement worker answer/resume delivery ambiguity state;
- capture bounded final worker result into result artifact before completion
  publication.

Exit criteria:

- fake stale-Herdr tests pass;
- real Claude conformance report passes on pinned Claude version;
- daemon-offline Stop hook recovers exactly once;
- personal Claude settings remain untouched;
- no monitor-only sessions are required;
- raw prompt/tool/cwd/transcript data does not appear in Command Governor safe
  persistence/logs.

## Phase 3 — stable MCP server + supported tunnel

- implement `rmcp` V1 four-tool ABI;
- implement wake-delivery correlation and claim fencing;
- implement paging for bounded result artifacts;
- implement explicit ACK/disposition and input-answer policy;
- integrate the currently supported OpenAI Secure MCP Tunnel/connectivity path
  without creating a second state authority;
- implement `doctor` compatibility/capability checks.

Exit criteria:

- deterministic MCP fencing/security tests pass;
- unrelated/stale fake connector cannot claim current work from bootstrap alone;
- exact repeated ACK is idempotent, conflicting stale ACK is rejected;
- transport restart cannot close/lose obligations;
- Gate A is run and recorded on every declared supported ChatGPT plan/surface.

## Phase 4 — generic browser/CDP foundation

- implement `governor-browser` trait and `chromiumoxide` driver;
- launch/attach dedicated Chrome profile;
- target/page/network event plumbing;
- browser process/profile ownership/restart supervision;
- staged route/composer readiness diagnostics;
- no ChatGPT private-write logic.

Exit criteria:

- fake CDP/target crash tests pass;
- no browser I/O can occur before durable delivery claim in adapter tests;
- profile ownership and secret-redaction tests pass.

## Gate B — authenticated headed ChatGPT browser spike

Run the exact spike in `browser-transport.md` with a disposable bound conversation
and fake obligations:

- login/profile persistence;
- exact conversation binding;
- per-message Command Governor app selection;
- 10/10 unique wake submissions;
- strong accepted evidence;
- wrong-chat fencing;
- crash after claim and around Send activation;
- ambiguous reconciliation without replay;
- physical settlement != ACK;
- rebind generation fencing;
- browser restart;
- MCP outage;
- separate `--headless=new` comparison.

A duplicate Send fails the gate.

## Phase 5 — `governor-chatgpt-web`

Only after enough of Gate B is understood to freeze the adapter contract:

- implement exact binding verification;
- implement current app-selection structural control;
- implement tiny deterministic wake payload;
- implement durable `claimed` + `activation_armed` Send boundary;
- implement CDP semantic accepted evidence;
- implement passive/direct read reconciliation where robust;
- implement physical assistant-turn observation;
- implement bounded settled-without-ACK resume revisions;
- keep private endpoint/schema observations internal and replaceable.

Exit criteria:

- all deterministic browser-delivery tests pass;
- Gate B passes on supported headed Chrome platform;
- accepted/ambiguous never automatically replay;
- exact same delivery revision cannot physically submit twice;
- ChatGPT-specific code is confined to the adapter crate.

## Phase 6 — end-to-end durable foreman loop

Scenario:

```text
Claude work
  -> native terminal/input event
  -> durable obligation + result/input identity
  -> browser wake accepted
  -> exact ChatGPT foreman turn
  -> MCP resume/fetch
  -> independent review/action
  -> explicit ACK or input answer
  -> obligation closes/resumes only on fenced evidence
```

Inject daemon/browser/runtime/worker/tunnel restarts at every boundary.

Exit criteria:

- no lost result/input obligation;
- no duplicate browser or worker continuation;
- no stale-generation ACK;
- no task considered complete before independent review disposition;
- `status`, `obligations`, and `doctor` explain every unresolved condition.

## Phase 7 — GitHub engineering integration

- stable project/repository binding;
- issue/commit/PR references and review evidence;
- source refs attached to obligations/results;
- least-privilege auth path;
- prompt-injection separation between GitHub content and Governor control fields.

GitHub remains engineering truth; the local DB stores refs/provenance, not a
shadow source-code repository.

## Phase 8 — Codex and additional adapters

- Codex worker lifecycle adapter;
- additional runtime adapters only when they can satisfy the same contract;
- compatibility/capability matrix;
- adapter conformance suite required before support claim.

Provider additions do not change obligation or browser semantics.

## Phase 9 — hardening and public V1 release

- migration/backup/restore tooling;
- security audit of local IPC/profile/artifact/hook boundaries;
- crash/failpoint extended CI;
- dependency/license/source review;
- signed/reproducible release strategy where practical;
- macOS first-class packaging, Linux and Windows support matrix;
- documentation of unsupported ChatGPT plan/surface combinations;
- exact provenance/third-party notices.

## Later, not V1 correctness requirements

- optional menu-bar/status UI as a daemon client;
- multi-machine control plane;
- multiple simultaneous foreman bindings with explicit routing policy;
- team/multi-user authorization;
- stronger OS sandboxing for workers;
- official foreman/wake provider APIs when available.

A future UI must never become the lifecycle authority. Human completion
notifications remain optional operator convenience, not the primary wake design.
