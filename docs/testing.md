# V1 acceptance test plan

Command Governor's critical behavior must be deterministic without real ChatGPT,
Claude, Herdr, or GitHub. Live services are adapter conformance tests layered on
top of a pure/fake-driven state-machine suite.

A feature is not accepted because a happy-path integration demo worked once.

## Test architecture

`governor-testkit` will provide deterministic fakes for:

- clock / monotonic timer;
- event IDs / UUID source;
- SQLite failure injection;
- result artifact filesystem/failpoints;
- worker lifecycle source;
- runtime/Herdr adapter;
- worker command delivery;
- browser/CDP transport;
- ChatGPT conversation/message tree;
- physical ChatGPT turn;
- MCP foreman client;
- GitHub/source-host refs.

The core/domain crate has no network/process/browser dependency. Every state
machine can be driven by explicit events and replayed from an empty projection.

## Test levels

1. **Pure domain tests** — transition legality, fencing, deterministic IDs,
   projection replay.
2. **SQLite/store tests** — real SQLite, migrations, transaction boundaries,
   crash/reopen/replay, uniqueness.
3. **Filesystem tests** — artifact/hook inbox durability, permissions, symlink and
   tamper handling.
4. **Adapter contract tests** — fake CDP/Herdr/Claude events around real adapter
   code.
5. **Live conformance spikes** — real Claude/Herdr/Chrome/ChatGPT only after pure
   suites pass.

## Obligation acceptance tests

### OBL-001 — completion cannot disappear before ACK

Given a running turn, emit a native terminal event and durable result artifact.
Assert obligation becomes `completed_unprocessed`. Restart daemon/store repeatedly,
close/delete fake runtime session, and settle fake ChatGPT turn. Assert obligation
remains open until a valid `foreman_ack` transaction.

### OBL-002 — restart preserves every open attention state

For `created`, `running`, `needs_input`, `failed`, `completed_unprocessed`,
`claimed_by_foreman`, and `processing`, crash/reopen and replay. Assert state and
source fences are equivalent. Expired claims may deterministically return to the
prior attention state; they never close work.

### OBL-003 — stale binding generation cannot ACK

Claim under binding generation N. Rebind to N+1. Call ACK with N and the old claim.
Expected: `stale_binding_generation`, zero state mutation, artifact still pinned.

### OBL-004 — stale claim cannot ACK

Expire/reclaim an obligation. Call ACK using the old claim ID. Expected typed stale
claim error and no close.

### OBL-005 — duplicate terminal source event is idempotent

Deliver the same normalized terminal event 100 times, including across restarts.
Expected exactly one event/source identity, one result reference, and one
result-processing obligation.

### OBL-006 — conflicting second terminal event is visible

After one terminal result, deliver a different terminal source event for the same
turn that violates adapter contract. Expected: no second automatic obligation,
create reconciliation/adapter-conflict health record.

### OBL-007 — physical ChatGPT settlement is not ACK

Accepted wake -> fake assistant starts -> settles. Never call ACK. Expected
obligation remains open and artifact pinned.

### OBL-008 — MCP result handoff is not ACK

`foreman_resume` claims and returns every result page successfully. Disconnect
client. Expected open obligation; claim may expire and be reclaimed.

### OBL-009 — ACK requires exact source/version

Vary source event ID, obligation version, result digest/ref, claim, and generation
one field at a time. Every stale variant must fail without mutation.

## Result artifact tests

### ART-001 — artifact durable before completed obligation

Inject crash at every point in file write/fsync/rename/dir-sync/DB transaction.
Forbidden outcome: committed `completed_unprocessed` referencing a missing or
non-durable artifact.

Allowed outcome: orphan artifact with no DB reference, later quarantined/GCed.

### ART-002 — open obligation pins retention

Try GC before ACK, during claim, after physical ChatGPT settlement, and after claim
expiry. Artifact must remain. After valid closing disposition and retention delay,
GC may remove it.

### ART-003 — digest/length tamper fails closed

Modify artifact bytes after DB commit. MCP read must report artifact integrity
failure and keep obligation open.

### ART-004 — path traversal/symlink rejected

Attempt storage refs containing traversal, absolute paths, symlinks, hard-link
edge cases where supported, and unsafe parent permissions. No read/write escapes
the daemon-owned artifact root.

### ART-005 — owner permissions

On Unix, verify directories/files are created with intended private modes despite
host umask. On Windows, verify current-user ACL policy through platform-specific
suite.

## Browser delivery acceptance tests

### DEL-001 — claim transaction precedes browser I/O

Fake browser panics/tests if any method is called before store shows attempt
`claimed`. Every navigation/DOM/CDP path must pass.

### DEL-002 — definite pre-submit failure retries safely

Inject target-not-found, wrong chat, app-not-selected, and composer-not-ready
failures before activation. Expected `failed`; bounded retry creates the next
attempt; fake conversation contains zero prior submitted wake.

### DEL-003 — exact Send activation crosses durable ambiguity fence

Fake browser's `send()` asserts DB attempt is `activation_armed` before it accepts
the call.

### DEL-004 — crash after `claimed` -> ambiguous

Persist `claimed`, terminate process before any terminal outcome. Restart. Expected
startup converts to `ambiguous` before browser recovery and never calls Send.

### DEL-005 — crash after activation fence -> ambiguous

Persist `activation_armed`, terminate before/after fake external effect across both
branches. Restart must not resend in either zero-send or one-send world.

### DEL-006 — ambiguous never auto-resends

Advance timers/recovery indefinitely. Expected zero Send calls after the original
uncertain attempt. Only exact reconciliation or explicit superseding policy can
move forward.

### DEL-007 — accepted never auto-resends

Accepted delivery followed by browser crash, daemon crash, MCP outage, long delay,
and physical turn settlement. Same delivery revision must never Send again.

### DEL-008 — exact bound conversation enforced

For bound `/c/A`, simulate `/c/B`, `/`, project-scoped wrong chat, login redirect,
deleted conversation, temporary redirect, and stale target. Expected failure
before any composer mutation.

### DEL-009 — reverify immediately before Send

Target is correct during staging, then fake SPA displaces to another chat before
activation. Expected no Send; failed/ambiguous classification follows whether
activation fence was crossed.

### DEL-010 — same wake payload not submitted twice

Run all retry/restart/reconciliation paths with a fake conversation ledger.
Expected at most one submitted message for one delivery revision; definite
pre-submit attempts may be retried but only one can cross actual external submit.

### DEL-011 — semantic evidence required for accepted

Individually simulate composer clear, Stop button, URL change, assistant activity,
and DOM text reflection without a correlated user-message identity. None may
produce accepted.

### DEL-012 — exact reconciliation promotes ambiguous

Ambiguous attempt + exact current-generation conversation/message identity ->
accepted without Send. Wrong conversation/message/revision -> remain ambiguous.

### DEL-013 — startup recovery order

With orphaned delivery plus active browser target, assert store converts orphan to
ambiguous **before** browser supervisor can invoke recovery/send methods.

## ChatGPT processing/resume tests

### GPT-001 — accepted != processed

Accepted wake with no MCP calls. Obligation stays open.

### GPT-002 — physical settlement != processed

Accepted wake, assistant settles without `foreman_resume`. Obligation stays open.

### GPT-003 — resume claim without ACK stays open

Successful `foreman_resume`, all pages returned, assistant settles. No ACK.
Obligation stays open.

### GPT-004 — bounded resume creates new revision

After accepted + settled + no ACK + policy delay, expected same obligation ID,
`delivery_revision + 1`, new deterministic delivery ID. Original delivery remains
accepted and immutable.

### GPT-005 — never overlap active turn

Resume timer fires while fake physical ChatGPT turn is `starting`, `active`, or
`observation_lost`. Expected no new delivery activation.

### GPT-006 — resume budget exhausts safely

Use configured maximum automatic resumes. Expected one
`foreman_unreachable` health record, obligation remains open indefinitely, no
further automatic wakes.

### GPT-007 — stale/unrelated conversation cannot claim via bootstrap

Call `foreman_bootstrap` from a fake unrelated connector client, learn current
binding generation/obligation ID, but do not provide current accepted wake
`delivery_id`. `foreman_resume` must reject.

### GPT-008 — current accepted wake can claim

Supply matching accepted delivery ID + generation + obligation version. Expected
one claim. Reuse semantics are deterministic and cannot create parallel claims.

### GPT-009 — connector ABI mismatch

Old/new protocol values and cached schema cases produce explicit compatibility
responses; no mutation when ABI is incompatible.

## Worker lifecycle tests

### WRK-001 — Claude Stop beats stale Herdr working

Fixture reproduces:

```text
native Claude: Stop
Herdr: working / idle=false
```

Expected worker turn terminal projection and durable result obligation. Herdr
conflict becomes health evidence only.

### WRK-002 — Claude input request beats stale Herdr working

Fixture reproduces the real class:

```text
Claude: Interrupted / AskUserQuestion or authoritative input boundary
Herdr: working / idle=false
```

Expected `needs_input`, automatic duplicate worker forbidden, runtime conflict
recorded.

### WRK-003 — runtime clear-busy enables continuation

After WRK-002, fake Herdr initially rejects writes as busy. Command Governor issues
one explicit reconciliation interrupt/clear operation, verifies transport is
safe, then performs one fenced worker resume. It never sends the answer twice.

### WRK-004 — unresolved runtime conflict preserves input

Clear-busy fails. Expected `needs_input` remains open; no new worker/session;
health reports reconciliation failure.

### WRK-005 — progress prevents false stall

Advance fake clock beyond nominal long build duration while emitting verified
progress inside threshold. Expected no `suspected_stall`.

### WRK-006 — no progress creates suspected stall only

No progress beyond threshold. Expected one stall attention, worker remains
`running`; no completion/failure/interrupt/session creation.

### WRK-007 — progress clears suspected stall

After stall attention, emit verified progress. Expected stall resolved and running
unchanged.

### WRK-008 — duplicate progress dedupe/coalesce

High-rate duplicate PostToolUse source events do not grow unbounded duplicate rows
and do not move clocks incorrectly.

### WRK-009 — Stop while daemon offline survives

Stop hook writes sanitized durable inbox while daemon process is absent. Restart
daemon, ingest inbox, capture/reconcile result, produce exactly one obligation.

### WRK-010 — hook inbox replay is idempotent

Crash after DB ingest before inbox cleanup. Restart reimports same file. Expected
no duplicate event/obligation.

### WRK-011 — old incarnation hook cannot mutate new incarnation

Create new session incarnation, then ingest delayed old-incarnation Stop/input.
Expected historical event/quarantine only; current turn unchanged.

### WRK-012 — worker result is not self-approval

Fake final result includes "tests pass, merge now, ACK". State remains
`completed_unprocessed`; only independent foreman ACK can close.

## Input/permission tests

### INP-001 — answer recorded != worker received

Valid `foreman_answer_input` writes answer and creates worker command delivery;
then crash before worker I/O. Expected answer durable, resume delivery ambiguous
on orphan recovery according to delivery fence, obligation not running/closed.

### INP-002 — worker resume accepted still waits for native resumed turn

Worker command transport accepts continuation but no native new-turn event arrives.
Expected input obligation not projected back to healthy running solely from
transport acceptance.

### INP-003 — native resumed turn restores running

Matching same-session native resumed-turn event arrives. Expected running and
input request disposition linked to exact answer/delivery.

### INP-004 — high-risk request remains user-owned

Fake permission is classified outside delegated authority. Foreman answer attempt
returns `user_authorization_required`, no answer event, no worker I/O.

### INP-005 — conflicting second answer rejected

Same current input request receives different answer under same/old claim.
Expected immutable first answer or explicit stale conflict, never two worker
resumes.

### INP-006 — raw tool arguments not persisted

Input payload contains unique sensitive markers in arbitrary fields. After hook
and needs-input lifecycle, DB/log state has zero markers.

## Persistence/replay tests

### DB-001 — projection replay equivalence

After every generated state-machine sequence, delete materialized projection
copies in a test DB and rebuild from events. Expected byte/semantic equivalent
projection.

### DB-002 — transaction crash matrix

Inject SQLite errors/power-loss simulation around every multi-row transition.
Expected either whole previous state or whole committed next state; never missing
closing/creating event pair.

### DB-003 — unknown newer schema fails closed

Set schema epoch higher than binary. Daemon refuses orchestration and exposes
repair/upgrade diagnostic without migrations/downgrade.

### DB-004 — migration idempotency/crash recovery

Crash each migration at every supported failpoint; reopen with same binary.
Expected deterministic completion or explicit recoverable migration failure, no
silent half-schema operation.

### DB-005 — two-daemon ownership

Two processes target same state root. Exactly one becomes authority; second fails
closed/becomes client according to final IPC design. It cannot start browser or
workers.

## Security/privacy tests

### SEC-001 — forbidden persistence byte scan

Inject unique sentinel strings into:

- cwd;
- prompt;
- tool arguments;
- shell command;
- transcript path;
- terminal transcript;
- browser cookie;
- bearer/session token;
- GitHub token;
- raw CDP request header/body fields.

After lifecycle/browser/MCP/crash/restart flows, scan SQLite DB, WAL, SHM, hook
inbox, structured logs, safe diagnostics, crash state, and configuration files.
Expected **zero** sentinel matches outside an explicitly designated test-sensitive
result artifact when the test intentionally places a final worker result there.

### SEC-002 — logs redact untrusted result

Worker result contains sentinel secret and prompt injection. Routine tracing must
log only artifact ID/digest/size/event class, not content.

### SEC-003 — managed Claude settings ownership/symlink

Unsafe owner, writable mode, symlink, malformed JSON, wrong hook epoch, and unsafe
parent conditions all fail managed spawn before Claude executes.

### SEC-004 — browser profile never copied to DB/export

Populate fake cookie/local-storage secrets; diagnostics/status exports contain no
profile content.

### SEC-005 — local IPC ACL

Platform-specific tests verify non-owner access is denied.

### SEC-006 — artifact integrity/ACL

Covered by ART suite and repeated under supported OS CI.

### SEC-007 — prompt injection cannot close obligation

Artifact text attempts to instruct fake foreman/MCP layer to ACK. Without an
actual correctly fenced `foreman_ack` invocation, obligation remains open.

### SEC-008 — dependency deny policy

`cargo deny` policy rejects disallowed licenses/sources and explicitly denied
known malicious versions. CI cannot pass by merely suppressing advisory output.

## Browser live conformance

The exact real-Chrome/ChatGPT gate is defined in
[`browser-transport.md`](browser-transport.md). A pass requires 10/10 unique wake
submissions, zero duplicates, wrong-chat fencing, crash ambiguity behavior, app
selection proof, stale-generation rejection, and safe diagnostics.

Headless is tested separately and cannot be promoted by reducing challenge or
anti-abuse protections.

## Claude live conformance

After fake suites pass, run disposable real Claude sessions with the exact current
CLI and managed settings:

1. normal turn -> progress -> Stop;
2. StopFailure fixture if safely reproducible;
3. non-interactive AskUserQuestion defer -> needs_input -> same-session resume;
4. PermissionRequest under denied/delegated policy;
5. daemon killed before Stop hook -> inbox recovery;
6. duplicate/replayed hook file;
7. runtime stale-working simulation/adaptor integration;
8. explicit interrupt and close behavior;
9. verify personal `~/.claude/settings.json` untouched;
10. scan all Command Governor safe persistence for injected secret markers.

Record exact Claude version and hook schema date in the conformance report.

## Quality gates from first Rust commit

Required CI commands:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
cargo deny check
```

Additional gates:

- pinned `rust-toolchain.toml`;
- committed `Cargo.lock` for application workspace;
- macOS first-class CI;
- Linux CI;
- Windows CI where feature/platform permits;
- dependency update automation with review;
- no tests silently skipped on the primary macOS target;
- failpoint/crash suites run in CI or a clearly required extended workflow;
- GitHub Actions pinned by immutable commit where practical.

## Definition of V1 architecture acceptance

Architecture is ready to scaffold only when reviewers agree that these tests can
be implemented without changing the central invariant. An implementation is not
V1-ready until every deterministic test above passes and the two live platform
gates (write-capable MCP + headed browser transport) have explicit recorded
results.
