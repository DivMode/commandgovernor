# V1 acceptance test plan

Command Governor's critical behavior must be deterministic without real ChatGPT,
Claude, Herdr, or GitHub. Live services are adapter-conformance gates layered on a
pure/fake-driven state-machine suite.

A feature is not accepted because a happy-path demo worked once.

## Test architecture

`governor-testkit` provides deterministic fakes for:

- wall/monotonic clock;
- generated IDs and provider/source identities;
- SQLite failures and restart points;
- result-artifact filesystem/failpoints;
- Claude structured stream / hook / worker-host child process;
- runtime/Herdr adapter;
- worker command delivery;
- browser/CDP transport;
- ChatGPT conversation/message tree and physical turn;
- MCP foreman client;
- GitHub/source-host references.

The core/domain crate has no network/process/browser dependency. Every state
machine can be driven by explicit events and replayed from an empty projection.

## Test levels

1. **Pure domain** — transition legality, fencing, deterministic IDs, replay.
2. **SQLite/store** — real SQLite, migrations, transactions, crash/reopen,
   uniqueness.
3. **Filesystem** — result artifacts, hook inbox, worker-host spool, permissions,
   symlink/tamper handling.
4. **Adapter contract** — fake CDP/Herdr/Claude around real adapter code.
5. **Live conformance** — real Claude/Herdr/Chrome/ChatGPT only after the pure
   suites pass.

---

## Obligation acceptance tests

### OBL-001 — completion cannot disappear before ACK

Given a confirmed terminal worker result and durable result artifact, assert the
obligation becomes `completed_unprocessed`. Restart daemon/store repeatedly,
close/delete the fake runtime session, settle a fake ChatGPT turn, and expire a
foreman claim. The obligation remains open until a valid `foreman_ack` transaction.

### OBL-002 — restart preserves open attention states

For `created`, `running`, `needs_input`, `failed`, `completed_unprocessed`,
`claimed_by_foreman`, and `processing`, crash/reopen/replay. State and source
fences remain equivalent. Expired internal claims may return to their prior
attention state but never close work.

### OBL-003 — stale binding generation cannot ACK

Claim under generation N, rebind to N+1, ACK with N. Expected:
`stale_binding_generation`, zero state mutation, artifact still pinned.

### OBL-004 — stale claim cannot ACK

Expire/reclaim an obligation then ACK with the old claim. Expected typed stale
claim error, no close.

### OBL-005 — duplicate terminal source event is idempotent

Replay the same confirmed terminal source event 100 times, including across
restarts. Exactly one terminal event/result reference/processing obligation exists.

### OBL-006 — conflicting terminal evidence is visible

After one confirmed terminal result, deliver contradictory terminal evidence for
the same turn. Expected no second automatic obligation and a durable
reconciliation/adapter-conflict condition.

### OBL-007 — physical ChatGPT settlement is not ACK

Accepted wake -> assistant starts -> assistant settles. No ACK. Obligation remains
open and result artifact pinned.

### OBL-008 — MCP result handoff is not ACK

`foreman_resume` claims and returns every artifact page; disconnect client. The
obligation stays open and may later be reclaimed after claim expiry.

### OBL-009 — ACK requires exact source/version

Vary source event, obligation version, claim, result identity, and binding
generation one field at a time. Every stale variant fails without mutation.

---

## Result artifact / worker transport tests

### ART-001 — artifact durable before completed obligation

Inject a crash at each file write/fsync/rename/directory-sync/DB point. Forbidden:
committed `completed_unprocessed` references a missing/non-durable result artifact.
Allowed: an unreferenced orphan file later quarantined/GCed.

### ART-002 — open obligation pins retention

Try GC before ACK, during claim, after physical ChatGPT settlement, and after claim
expiry. Artifact remains. Only after a valid closing disposition and retention
delay may GC delete it.

### ART-003 — artifact digest/length tamper fails closed

Modify artifact bytes after DB commit. MCP read reports integrity failure and the
obligation remains open.

### ART-004 — path traversal/symlink rejected

Attempt traversal, absolute paths, symlinks, unsafe parents, and relevant hard-link
edge cases. No read/write escapes the daemon-owned root.

### ART-005 — private filesystem permissions

On Unix verify intended private modes regardless of host umask. On Windows verify
current-user ACL policy in the platform suite.

### ART-006 — worker-host final result survives daemon outage

Run fake worker-host while the authoritative daemon is absent. It receives a
structured Claude final result, writes a complete bounded private spool plus
sanitized child-exit receipt, and exits. Restart daemon. Expected exactly one
confirmed terminal result -> immutable result artifact -> one open obligation.

### ART-007 — truncated worker-host stream never becomes completion

Crash worker-host/child at every point before a complete structured final result
and matching exit receipt. Expected reconciliation/failure attention according to
known evidence, never `completed_unprocessed` from partial bytes.

### ART-008 — spool is transport-only

Populate a valid transport spool and exit receipt without starting daemon. Assert
no task/obligation state exists until the authoritative daemon imports/reconciles
it. Worker-host never writes SQLite lifecycle projections.

### ART-009 — spool content is not routine diagnostics

Inject prompt/tool/result sentinels into the private provider stream. Safe logs,
SQLite, status/doctor output, hook inbox, and crash metadata contain none of them.
Only the explicitly sensitive spool/result-artifact boundary may contain allowed
content.

---

## Browser delivery acceptance tests

### DEL-001 — claim transaction precedes all browser I/O

Fake browser panics if any method is called before the store shows the attempt
`claimed`. Navigation, DOM and CDP paths must satisfy it.

### DEL-002 — definite pre-submit failure retries safely

Inject target-not-found, stale obligation version, wrong chat, app-not-selected,
and composer-not-ready before activation. Expected `failed`; bounded retry may
create the next attempt; zero submitted wake messages so far.

### DEL-003 — Send activation crosses durable ambiguity fence

Fake browser `send()` asserts DB attempt is `activation_armed` before accepting the
call.

### DEL-004 — crash after `claimed` -> ambiguous

Persist `claimed`, terminate before terminal outcome, restart. Startup converts it
to `ambiguous` before browser recovery and never calls Send.

### DEL-005 — crash around activation fence -> ambiguous

Test both zero-send and one-send physical worlds around `activation_armed`. Restart
must not resend in either world.

### DEL-006 — ambiguous never auto-resends

Advance timers/recovery indefinitely. Zero additional Send calls for that revision.
Only exact reconciliation or explicit audited superseding policy may move forward.

### DEL-007 — accepted never auto-resends

After accepted, trigger browser crash, daemon crash, MCP outage, long delay, and
physical settlement. Same delivery revision never Sends again.

### DEL-008 — exact bound conversation enforced

Bound `/c/A`; simulate `/c/B`, `/`, project-scoped wrong chat, login redirect,
deleted chat, and stale target. Expected failure before composer mutation.

### DEL-009 — target reverified immediately before Send

Target is correct during staging then displaced before activation. No Send. Outcome
classification depends on whether the ambiguity fence was crossed.

### DEL-010 — target obligation version reverified immediately before Send

Wake targets obligation version V/source S. Change obligation before activation.
Expected stale delivery, zero Send.

### DEL-011 — same wake revision not submitted twice

Exercise retry/restart/reconciliation matrices. At most one physical submitted
message exists for one delivery revision.

### DEL-012 — semantic evidence required for accepted

Composer clear, Stop button, URL change, assistant activity, and DOM text reflection
without correlated user-message identity never produce accepted.

### DEL-013 — exact reconciliation promotes ambiguous

Ambiguous + exact current-generation conversation/message identity -> accepted
without Send. Wrong conversation/message/revision -> remain ambiguous.

### DEL-014 — startup recovery order

With an orphaned attempt and live browser target, assert orphan conversion to
ambiguous occurs before browser supervisor recovery/send methods.

---

## ChatGPT processing / resume tests

### GPT-001 — accepted != processed

Accepted wake, no MCP. Obligation stays open.

### GPT-002 — physical settlement != processed

Accepted wake, assistant settles without `foreman_resume`. Obligation stays open.

### GPT-003 — resume claim without ACK stays open

Successful `foreman_resume`, all pages returned, assistant settles. No ACK. Open.

### GPT-004 — bounded resume creates new revision

Accepted + settled + no ACK + policy delay -> same obligation, revision +1, new
deterministic delivery ID. Original accepted revision stays immutable.

### GPT-005 — never overlap active/unknown ChatGPT turn

Resume timer fires while turn is starting/active/observation_lost. No new delivery
activation.

### GPT-006 — resume budget exhausts safely

After configured automatic resumes, create one `foreman_unreachable`; obligation
remains open indefinitely; no infinite wake loop.

### GPT-007 — unrelated/stale conversation cannot claim via bootstrap

Fake unrelated connector learns generation/obligation from bootstrap but not the
accepted wake `delivery_id`. `foreman_resume` rejects it.

### GPT-008 — current accepted wake can claim

Matching accepted delivery ID + generation + obligation version creates one
current claim. Parallel/repeated claim semantics are deterministic.

### GPT-009 — connector ABI mismatch fails closed

Old/new cached schema cases produce compatibility results; no mutation under an
incompatible ABI.

---

## Claude / worker lifecycle tests

### WRK-001 — structured final result beats stale Herdr working

Fixture:

```text
Claude worker-host: complete final structured result + matching successful exit
Herdr: working / idle=false
```

Expected confirmed terminal worker outcome, durable result artifact, one
`completed_unprocessed`, and `runtime_state_conflict` only for Herdr disagreement.

### WRK-002 — confirmed deferred input beats stale Herdr working

Fixture:

```text
Command Governor PreToolUse defer for AskUserQuestion
Claude structured result: run ended with pending deferred call
Herdr: working / idle=false
```

Expected `needs_input`; duplicate worker forbidden; runtime conflict recorded.

### WRK-003 — Stop-hook callback alone is not completion

Emit only a matching Claude `Stop` hook callback. No final structured result or
child exit. Expected bounded `stop_candidate` evidence/progress and **no**
`completed_unprocessed`.

### WRK-004 — parallel Stop-hook veto cannot create false completion

Fixture:

```text
CG Stop hook fires -> stop_candidate #1
another matching Stop hook returns decision:block
Claude continues and emits more progress/tool events
CG Stop hook fires -> stop_candidate #2
final structured result + matching child exit arrives
```

Expected no completion after candidate #1 or #2. Completion occurs exactly once
only after final structured result/exit is proven and artifact is durable.

### WRK-005 — stop candidate followed by continued work updates watchdog

After stop candidate, emit verified tool progress. Turn remains `running` and the
new progress resets `last_progress_at`; no contradiction or duplicate obligation.

### WRK-006 — child success exit without final result is not completion

Process reports successful exit but provider stream lacks a complete final
structured result. Expected reconciliation condition, no completed result.

### WRK-007 — final result without trustworthy child exit is not completion

Complete-looking result exists but worker-host exit receipt is missing/ambiguous.
Expected reconciliation condition, no completed result until safe reconciliation.

### WRK-008 — StopFailure is reconciled, not blindly equated

Emit `StopFailure` plus each relevant process outcome. Adapter maps only documented
accepted combinations to `failed`; raw provider error body is not persisted.

### WRK-009 — SessionEnd is session end, not result success

Emit SessionEnd without successful structured result. It can close session
transport projection but cannot create `completed_unprocessed`.

### WRK-010 — runtime clear-busy enables one continuation

After confirmed deferred input while fake Herdr remains busy, Command Governor
issues one fenced reconciliation interrupt/clear, verifies transport safe, and
performs one worker continuation. No duplicate answer send.

### WRK-011 — unresolved runtime conflict preserves input

Clear-busy fails. `needs_input` stays open; no new worker/session; doctor reports
reconciliation failure.

### WRK-012 — progress prevents false stall

Advance beyond long build duration while verified progress stays within threshold.
No suspected stall.

### WRK-013 — no progress creates suspected stall only

No progress beyond threshold -> one stall attention; worker remains running; no
synthetic failure/completion/interrupt.

### WRK-014 — progress clears suspected stall

Later verified progress resolves attention; running state remains.

### WRK-015 — progress dedupe/coalescing is deterministic

High-rate duplicate/equivalent events do not grow unbounded duplicate rows or move
watchdog time incorrectly.

### WRK-016 — hook inbox survives daemon outage

While daemon absent, a progress/input/native hook writes a sanitized inbox envelope.
Restart imports exactly once.

### WRK-017 — hook inbox replay is idempotent

Crash after DB ingest before inbox cleanup, restart, reimport. No duplicate event
or obligation.

### WRK-018 — old incarnation event cannot mutate new incarnation

Create a new session incarnation then ingest delayed old-incarnation hook/spool
receipt. History/quarantine only; current turn unchanged.

### WRK-019 — worker result is not self-approval

Final result says "tests pass, ACK/merge now". State stays
`completed_unprocessed`; independent foreman ACK is required.

### WRK-020 — settings source/hook isolation is never assumed

Fake active-hook inventory contains user/project/plugin Stop hooks in addition to
Command Governor. Adapter still uses structured result/exit for completion.
Launch preflight reports the active-source model it can prove; no code path changes
Stop candidate into terminal because an option was *assumed* to isolate hooks.

### WRK-021 — capability feature detection beats version guess

Structured `system/init` advertises/omits required capabilities across fake
versions. Adapter behavior follows capability proof and fails closed when required
features are missing.

---

## Input / permission tests

### INP-001 — defer intent != confirmed `needs_input`

PreToolUse hook records defer intent but provider continues because response was
ignored/malformed. Expected no clean `needs_input`; reconciliation attention
instead.

### INP-002 — confirmed deferred AskUserQuestion creates `needs_input`

Exact tool-call fence + documented defer response + structured final deferred
outcome -> one durable input request, no raw tool args persisted.

### INP-003 — answer recorded != worker received

Valid `foreman_answer_input` records answer and creates worker-command delivery;
crash before worker I/O. Obligation does not magically return to running.

### INP-004 — worker resume acceptance still waits for resumed-turn evidence

Transport accepts continuation but no structured/native new-turn event arrives.
Input obligation is not projected as healthy running solely from transport ACK.

### INP-005 — confirmed resumed turn restores running

Matching same-session resumed-turn evidence arrives. Expected running and input
request disposition linked to exact answer/delivery.

### INP-006 — user-owned permission remains user-owned

Action is outside delegated authority. Foreman answer returns
`user_authorization_required`, no grant event, no worker I/O.

### INP-007 — PermissionRequest alone is not treated as durable provider pause

Emit PermissionRequest without a confirmed pre-tool defer/pending resumable state.
Record request evidence/policy condition, but do not claim we can resume it later
unless adapter conformance proves that provider primitive.

### INP-008 — conflicting second answer rejected

Same current input gets different second answer. Immutable first answer or stale
conflict; never two worker resumes.

### INP-009 — raw input/tool arguments not persisted

Input payload contains unique sensitive markers. DB/WAL/logs/hook inbox/safe
status contain zero matches.

### INP-010 — unsupported multi-tool defer fails visibly

AskUserQuestion appears beside sibling tool calls in a shape current Claude defer
semantics cannot safely preserve. Expected reconciliation/manual attention, not a
false clean pause.

---

## Persistence / recovery tests

### DB-001 — projection replay equivalence

After every generated state-machine sequence, rebuild materialized projections
from events. Semantic state must match.

### DB-002 — transition crash matrix

Inject SQLite errors/crashes around each multi-row transition. Reopen yields the
previous complete state or committed next state, never a half-transition.

### DB-003 — unknown newer schema fails closed

Set schema epoch above binary. Daemon refuses orchestration and exposes upgrade
status; no downgrade/mutation.

### DB-004 — migration crash recovery

Crash each migration at supported failpoints. Reopen produces deterministic
completion or explicit repair state.

### DB-005 — two-daemon ownership

Two processes target same state root. Exactly one becomes authority; the second
cannot start browser/workers or write lifecycle state.

### DB-006 — recovery order imports spools before scheduling wake

On startup with pending hook inbox, completed worker-host spool, stale Herdr
working, and an open browser target, assert worker result/lifecycle reconciliation
finishes before any new browser wake is scheduled.

---

## Security / privacy tests

### SEC-001 — forbidden persistence byte scan

Inject unique sentinels into:

- cwd;
- prompt;
- raw tool arguments;
- shell command;
- transcript path;
- terminal transcript;
- browser cookie/token;
- GitHub auth;
- raw CDP headers/bodies;
- full Claude structured stream fields not explicitly allowed in final result.

After lifecycle/browser/MCP/crash/restart flows, scan SQLite DB/WAL/SHM, hook
inbox, structured logs, safe diagnostics, crash state, and configuration. Expected
**zero matches** outside deliberately sensitive worker-host/result-artifact files
where the test intentionally places allowed content.

### SEC-002 — routine logs redact worker result

Result contains secret/prompt-injection sentinel. Logs contain only artifact
identity/digest/size/event classes, not content.

### SEC-003 — managed Claude settings ownership/symlink

Unsafe owner/mode, symlink, malformed JSON, wrong hook epoch, and unsafe parent all
fail managed spawn before Claude executes.

### SEC-004 — browser profile never copied to DB/export

Fake cookie/local-storage secrets never appear in diagnostics/status/state exports.

### SEC-005 — local IPC ACL

Platform suites reject non-owner access.

### SEC-006 — result/spool integrity and ACL

Artifact and worker-host spool roots enforce private ownership, no-follow rooted
access, digest/complete-record checks, bounded sizes, and explicit retention.

### SEC-007 — prompt injection cannot close obligation

Artifact/repository text instructs fake foreman to ACK. Without a correctly fenced
`foreman_ack` tool call, obligation stays open.

### SEC-008 — dependency deny policy

`cargo deny` rejects disallowed licenses/sources and explicit known-bad malicious
versions. Advisory output cannot be silently suppressed to green CI.

### SEC-009 — worker-host owns no authority

Attempt to feed worker-host arbitrary obligation IDs/state commands. It may write
only its allocated transport spool/receipt; it has no SQLite/control-plane mutation
capability.

---

## Live Gate B — Chrome / ChatGPT conformance

The real headed-Chrome/ChatGPT protocol is defined in
[`browser-transport.md`](browser-transport.md). Support requires 10/10 unique wake
submissions, zero duplicates, wrong-chat fencing, per-message app selection,
crash-at-Send ambiguity/no replay, restart/rebind behavior, and safe diagnostics.

Headless is a separate experiment and cannot be promoted by adding stealth,
challenge, CAPTCHA, or anti-abuse bypass.

## Live Gate C — Claude managed-execution conformance

After fake suites pass, use a disposable real Claude session/runtime and record the
exact Claude version, CLI invocation, active settings sources, runtime version,
and Command Governor commit.

Required cases:

1. `system/init` exposes expected session/capability metadata;
2. normal managed turn yields one complete final structured `result` and matching
   successful child exit;
3. Command Governor Stop hook fires but another controlled Stop hook returns
   `decision:block`; Claude continues; **no terminal obligation exists yet**;
4. later real final result/exit creates exactly one result obligation;
5. StopFailure/error cases map to documented failure state;
6. `SessionEnd` without successful result cannot fabricate completion;
7. AskUserQuestion `PreToolUse` defer is confirmed by the structured result and the
   same Claude session resumes correctly;
8. unsupported multi-tool defer shape fails visibly;
9. permission policy defer/deny behavior matches current Claude semantics;
10. kill Command Governor daemon before Claude finishes; worker-host still records
    complete private result/exit and daemon later recovers exactly once;
11. kill/truncate worker-host path before final record; daemon does not fabricate
    completion;
12. stale Herdr `working / idle:false` cannot block a confirmed final/deferred
    Claude state;
13. personal Claude settings are never modified;
14. active user/project/plugin hooks under the selected CLI flags are measured,
    not assumed away;
15. forbidden prompt/tool/cwd/transcript sentinels are absent from safe persistence.

A Stop-veto false completion is a **Gate C failure**.

## Quality gates from the first Rust commit

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
cargo deny check
```

Also require:

- pinned `rust-toolchain.toml`;
- committed application `Cargo.lock`;
- macOS first-class CI;
- Linux CI;
- Windows CI where supported;
- dependency-update automation with human review;
- no silently skipped tests on the primary macOS target;
- failpoint/crash suites in CI or a required extended workflow;
- GitHub Actions pinned to immutable commit SHAs where practical.

## Definition of architecture acceptance

Architecture is ready for the small Phase 1 Rust core/store/testkit scaffold when
reviewers agree these tests can be implemented without weakening the central
invariant. End-to-end V1 is not supported until deterministic suites and live Gates
A, B, and C have explicit recorded results.
