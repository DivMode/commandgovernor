# V1 acceptance test plan

Command Governor's critical behavior must be deterministic without real ChatGPT,
Claude, Herdr, or GitHub. Live services are adapter-conformance gates layered on a
pure/fake-driven state-machine suite.

A feature is not accepted because a happy-path demo worked once.

## Test architecture

`governor-testkit` provides deterministic fakes for:

- wall/monotonic clock;
- generated IDs, CSPRNG output, and provider/source identities;
- SQLite failures and restart points;
- result-artifact and managed-run staging filesystem/failpoints;
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

1. **Pure domain** — transition legality, fencing, dedupe keys, random correlation
   identity, replay.
2. **SQLite/store** — real SQLite, migrations, transactions, crash/reopen,
   uniqueness.
3. **Filesystem** — result artifacts, hook inbox, managed-run staging,
   permissions, symlink/tamper handling.
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

### OBL-010 — failure is unprocessed work

A verified terminal worker failure creates/keeps a processable failure obligation.
Runtime close, restart, or assistant settlement cannot silently discard it.

---

## Result artifact / worker transport tests

### ART-001 — artifact durable before completed obligation

Inject a crash at each candidate-validation/file write/fsync/rename/directory-sync/
DB point. Forbidden: committed `completed_unprocessed` references a missing or
non-durable result artifact. Allowed: an unreferenced orphan file later
quarantined/GCed.

### ART-002 — open obligation pins retention

Try GC before ACK, during claim, after physical ChatGPT settlement, and after claim
expiry. Artifact remains. Only after a valid closing disposition and retention
delay may GC delete it.

### ART-003 — artifact digest/length tamper fails closed

Modify artifact bytes after DB commit. MCP read reports integrity failure and the
obligation remains open.

### ART-004 — path traversal/symlink rejected

Attempt traversal, absolute paths, symlinks, unsafe parents, and relevant hard-link
edge cases. No read/write escapes the daemon-owned root under the platform's
supported rooted/no-follow policy.

### ART-005 — private filesystem permissions

On Unix verify intended private modes regardless of host umask. On Windows verify
current-user ACL policy in the platform suite. This test proves privacy against
other OS principals, **not** hostile same-user worker containment.

### ART-006 — final result survives daemon outage

Run fake worker-host while authoritative daemon is absent. It parses structured
Claude output, writes one bounded final-result candidate plus sanitized run/exit
receipts, and exits. Restart daemon. Expected exactly one confirmed terminal
result -> immutable artifact -> one open obligation.

### ART-007 — truncated result never becomes completion

Crash worker-host/child at every point before a complete final structured result
and matching exit receipt. Expected reconciliation/failure attention according to
known evidence, never `completed_unprocessed` from partial bytes.

### ART-008 — worker-host is transport-only

Populate valid final-result candidate and receipts without starting daemon. Assert
no task/obligation projection exists until authoritative daemon imports/reconciles
them. Worker-host has no protocol path that writes lifecycle SQLite state.

### ART-009 — no raw provider-stream spool exists

Feed a fake stream containing unique sentinels in prompt fragments, `tool_use`
inputs, shell command, `tool_result`, cwd, transcript path, and intermediate
assistant content. The worker-host may inspect records in memory but after exit no
durable staging file contains those sentinels. Only the explicitly designated
bounded final assistant result sentinel may exist in final-result candidate/result
artifact.

### ART-010 — managed-run receipts are allowlist-only

Fuzz provider records with unknown nested fields and sensitive strings. Durable run
and exit receipts contain only defined opaque IDs, safe event/outcome classes,
flags, timestamps, and bounded numeric metadata.

### ART-011 — deferred input receipt omits raw question/tool input

A confirmed deferred AskUserQuestion contains unique option/question sentinels.
Durable receipt stores only safe input identity/classification/stop reason. Raw
question/options are absent from DB, staging, inbox, logs, and diagnostics.

---

## Browser delivery acceptance tests

### DEL-001 — delivery key deterministic, delivery ID random

For identical `(obligation, binding_generation, revision)`, deterministic
`delivery_key` is identical. Independent logical revisions produce distinct keys.
The associated `delivery_id` is generated from configured CSPRNG, has at least 192
bits of entropy in production construction, and is not a hash of deterministic
metadata.

### DEL-002 — duplicate scheduling converges

Schedule the same logical revision concurrently/repeatedly. Unique `delivery_key`
returns one durable delivery row and one previously generated random `delivery_id`;
it never creates two physical revisions.

### DEL-003 — claim transaction precedes all browser I/O

Fake browser panics if any method is called before store shows attempt `claimed`.
Navigation, DOM, and CDP paths must satisfy it.

### DEL-004 — definite pre-submit failure retries safely

Inject target-not-found, stale obligation version, wrong chat, app-not-selected,
and composer-not-ready before activation. Expected `failed`; bounded retry may
create the next attempt under the same delivery revision; zero submitted messages.

### DEL-005 — Send activation crosses durable ambiguity fence

Fake browser `send()` asserts DB attempt is `activation_armed` before accepting the
call.

### DEL-006 — crash after `claimed` -> ambiguous

Persist `claimed`, terminate before terminal outcome, restart. Startup converts it
to `ambiguous` before browser recovery and never calls Send.

### DEL-007 — crash around activation fence -> ambiguous

Test both zero-send and one-send physical worlds around `activation_armed`. Restart
must not resend in either world.

### DEL-008 — ambiguous never auto-resends

Advance timers/recovery indefinitely. Zero additional Send calls for that revision.
Only exact reconciliation or a later separately created resume revision may move
forward.

### DEL-009 — accepted never auto-resends

After accepted, trigger browser crash, daemon crash, MCP outage, long delay, and
physical settlement. Same delivery revision never Sends again.

### DEL-010 — exact bound conversation enforced

Bound `/c/A`; simulate `/c/B`, `/`, project-scoped wrong chat, login redirect,
deleted chat, and stale target. Expected failure before composer mutation.

### DEL-011 — target reverified immediately before Send

Target is correct during staging then displaced before activation. No Send. Outcome
classification depends on whether ambiguity fence was crossed.

### DEL-012 — target obligation version reverified immediately before Send

Wake targets obligation version V/source S. Change obligation before activation.
Expected stale delivery, zero Send.

### DEL-013 — same delivery revision not submitted twice

Exercise retry/restart/reconciliation matrices. At most one physical submitted
message exists for one delivery revision.

### DEL-014 — semantic evidence required for accepted

Composer clear, Stop button, URL change, assistant activity, and DOM text
reflection without correlated exact user-message identity never produce accepted.

### DEL-015 — exact reconciliation promotes ambiguous

Ambiguous + exact current-generation conversation/message identity -> accepted
without Send. Wrong conversation/message/random delivery ID -> remain ambiguous.

### DEL-016 — startup recovery order

With an orphaned attempt and live browser target, assert orphan conversion to
ambiguous occurs before browser supervisor recovery/send methods.

### DEL-017 — new resume revision gets new random correlation ID

Accepted/settled/unACKed obligation becomes eligible for bounded resume. New
revision has new deterministic `delivery_key` and independent random `delivery_id`;
old accepted revision remains immutable.

### DEL-018 — deterministic metadata cannot reconstruct delivery ID

Give an attacker fake client obligation ID, binding generation, revision,
`delivery_key`, wake protocol, bootstrap response, and all public safe metadata.
Without the random `delivery_id`, `foreman_resume` must fail; no deterministic
function of supplied metadata yields the accepted delivery ID.

---

## ChatGPT processing / MCP tests

### GPT-001 — accepted != processed

Accepted wake, no MCP. Obligation stays open.

### GPT-002 — physical settlement != processed

Accepted wake, assistant settles without `foreman_resume`. Obligation stays open.

### GPT-003 — resume claim without ACK stays open

Successful `foreman_resume`, all pages returned, assistant settles. No ACK. Open.

### GPT-004 — bounded resume creates new revision

Accepted + settled + no ACK + policy delay -> same obligation, revision +1, new
delivery key/random delivery ID. Original accepted revision stays immutable.

### GPT-005 — never overlap active/unknown ChatGPT turn

Resume timer fires while turn is starting/active/observation_lost. No new delivery
activation.

### GPT-006 — resume budget exhausts safely

After configured automatic resumes, create one `foreman_unreachable`; obligation
remains open indefinitely; no infinite wake loop.

### GPT-007 — bootstrap is low-information

Unrelated fake connector calls bootstrap. It may learn compatibility, active
binding generation, health, aggregate attention kinds/counts/priority/age/wake
state. It must not receive repository/project refs, task/session/worker refs,
result content, raw obligation metadata, or accepted random `delivery_id`.

### GPT-008 — unrelated/stale conversation cannot claim from bootstrap

Fake unrelated connector learns every bootstrap field and deterministic delivery
metadata but not the random accepted wake `delivery_id`. `foreman_resume` rejects
it with zero claim/state mutation.

### GPT-009 — current accepted wake can claim

Matching random accepted delivery ID + generation + obligation version creates one
current claim. Parallel/repeated claim semantics are deterministic.

### GPT-010 — connector ABI mismatch fails closed

Old/new cached schema cases produce compatibility results; no mutation under an
incompatible ABI.

### GPT-011 — write capability loss preserves work

Simulate binding originally write-capable, then MCP writes become unavailable.
No browser/assistant event closes work; doctor/health reports capability failure and
obligations remain durable.

### GPT-012 — product confirmation is not bypassed

Fake write action requires a user-owned/unsupported confirmation state. Command
Governor does not automate around it, reclassify the tool read-only, or mark ACK
successful. Binding/action state fails visibly.

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
CG PreToolUse: exact single-tool AskUserQuestion defer
Claude structured result: stop_reason/tool_deferred
Herdr: working / idle=false
```

Expected `needs_input`; duplicate worker forbidden; runtime conflict recorded.

### WRK-003 — Stop-hook callback alone is not completion

Emit only matching Claude `Stop` hook callback. No final structured result or child
exit. Expected bounded `stop_candidate` evidence and **no**
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

No completion after either candidate. Completion occurs exactly once only after
final result/exit is proven and artifact is durable.

### WRK-005 — stop candidate followed by progress stays running

After stop candidate, emit verified tool progress. Turn remains `running` and
progress resets watchdog timestamp.

### WRK-006 — child success exit without final result is not completion

Process reports successful exit but provider output lacks a complete final result.
Expected reconciliation condition, no completed result.

### WRK-007 — final result without trustworthy child exit is not completion

Complete final result exists but worker-host exit receipt is missing/ambiguous.
Expected reconciliation condition until safe evidence resolves.

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

### WRK-015 — progress dedupe/coalescing deterministic

High-rate duplicate/equivalent events do not grow unbounded duplicate rows or move
watchdog time incorrectly.

### WRK-016 — hook inbox survives daemon outage

While daemon absent, a progress/input/native hook writes a sanitized inbox envelope.
Restart imports exactly once.

### WRK-017 — hook inbox replay idempotent

Crash after DB ingest before inbox cleanup, restart, reimport. No duplicate event
or obligation.

### WRK-018 — old incarnation event cannot mutate new incarnation

Create new session incarnation then ingest delayed old-incarnation safe receipt.
History/quarantine only; current turn unchanged.

### WRK-019 — worker result is not self-approval

Final result says "tests pass, ACK/merge now". State stays
`completed_unprocessed`; independent foreman processing/ACK required.

### WRK-020 — settings source/hook isolation never assumed

Fake active-hook inventory contains user/project/plugin Stop hooks in addition to
Governor. Adapter still uses structured final result/exit; no code path promotes
Stop merely because a settings flag was assumed to isolate hooks.

### WRK-021 — capability feature detection beats version guess

Structured init advertises/omits required capabilities across fake versions.
Adapter follows capability proof and fails closed when required features are
missing.

### WRK-022 — non-interactive PermissionRequest can fire

Fake current Claude emits PermissionRequest under non-interactive mode. Adapter
must not drop it based on obsolete "does not fire in -p" logic. A valid pinned
permission policy may answer/deny it as decision evidence.

### WRK-023 — PermissionRequest is not exact pause identity

PermissionRequest carries tool name/input but no exact tool-use ID. It cannot alone
create a resumable `needs_input` record that claims a tool-call identity it does
not possess.

### WRK-024 — raw stream containing tool records is never persisted

Fake stream contains `tool_use`, command, `tool_result`, partial assistant, and
sensitive markers before final result. Only safe evidence and allowed final result
survive process exit.

### WRK-025 — general state root not exported to worker

Spawn fixture inspects managed environment/argv. Opaque turn/session/hook epoch and
narrow per-turn inbox locator may be present; general Command Governor state-root,
credentials, prompt, and secrets are absent.

---

## Input / permission tests

### INP-001 — defer intent != confirmed `needs_input`

PreToolUse records defer intent but provider continues because response was
ignored/malformed. No clean `needs_input`; reconciliation attention instead.

### INP-002 — confirmed single-tool AskUserQuestion creates `needs_input`

Exact tool-use fence + single-tool shape + documented defer response + structured
`tool_deferred` outcome -> one durable input request, no raw tool args persisted.

### INP-003 — multi-tool defer cannot create clean pause

AskUserQuestion appears beside sibling tool calls. Current Claude semantics ignore
`defer` in this shape. Expected `worker_defer_shape_unsupported`/manual
reconciliation, not `needs_input` and not an invented pending tool identity.

### INP-004 — answer recorded != worker received

Valid `foreman_answer_input` records answer and creates worker-command delivery;
crash before worker I/O. Obligation does not return to running.

### INP-005 — worker resume acceptance still waits for resumed-turn evidence

Transport accepts continuation but no structured/native new-turn event arrives.
Input obligation is not projected as healthy running solely from transport ACK.

### INP-006 — confirmed resumed turn restores running

Matching same-session resumed-turn evidence arrives. Expected running and input
request disposition linked to exact answer/delivery.

### INP-007 — user-owned permission remains user-owned

Action outside delegated authority. Foreman answer returns
`user_authorization_required`, no grant event, no worker I/O.

### INP-008 — PermissionRequest decision != durable pause

Emit non-interactive PermissionRequest. Record safe decision evidence as permitted,
but do not claim resumable pause semantics unless an exact provider primitive is
also proven.

### INP-009 — conflicting second answer rejected

Same current input gets different second answer. Immutable first answer or stale
conflict; never two worker resumes.

### INP-010 — raw input/tool arguments not persisted

Input payload contains unique sensitive markers. DB/WAL/logs/hook inbox/staging/
safe status contain zero matches.

### INP-011 — question detail unavailable remains durable

After restart, safe deferred identity exists but native provider cannot recover
question/options without transcript scraping. Return `input_detail_unavailable`;
keep obligation open; never invent an answer.

---

## Persistence / recovery tests

### DB-001 — projection replay equivalence

After every generated state-machine sequence, rebuild materialized projections
from events. Semantic state must match.

Coverage, and where it stops. Obligations, their per-transition version ledger,
turn lifecycle, artifact retention, health conditions, the foreman binding
ladder and the foreman claims are each rebuilt from the events and compared
with their rows. Three comparisons are narrower than a full rebuild, and the
residue is stated rather than left implicit:

- **Browser deliveries** — the delivery's state and each attempt's state are
  ledger-derived; the revision, attempt budget, binding generation, target
  version and accepted message ref are read from the row being verified and
  seed the fold, so they are inputs rather than compared outputs. The row's
  `delivery_key` is re-derived from `(obligation, generation, revision)` on
  every read, which is what protects them.
- **Foreman bindings** — the generation ladder, each generation's capability
  epoch and write capability, and which generation is active all rebuild.
  The binding's target identity (canonical conversation, browser profile,
  connector ABI, `foreman_binding_id`) is not carried in allowlisted safe
  metadata, so nothing in the ledger can be compared with it.
- **Foreman claims** — the lifecycle, the obligation, the binding generation
  and the version the mint was fenced on all rebuild. `wake_delivery_id` does
  not, deliberately: the correlation ID is a possession fence and is never
  written into safe metadata. `expires_at_ms` does not either; it is a clock
  reading, not a ledger fact.

The mutation journal, external attempts and resource leases are outside DB-001
by design: their own row is the durable record, and each loader re-folds that
row's recorded history through the domain machine on every read.

### DB-002 — transition crash matrix

Inject SQLite errors/crashes around each multi-row transition. Reopen yields prior
complete state or committed next state, never half-transition.

### DB-003 — unknown newer schema fails closed

Set schema epoch above binary. Daemon refuses orchestration and exposes upgrade
status; no downgrade/mutation.

### DB-004 — migration crash recovery

Crash each migration at supported failpoints. Reopen produces deterministic
completion or explicit repair state.

### DB-005 — two-daemon authority rejected

Start two daemon instances against one state root. Exactly one obtains authority;
second fails closed or behaves only as a client. SQLite writer serialization alone
is not accepted as daemon election.

### DB-006 — startup quarantines ambiguous external effects first

Seed orphaned browser and worker-command claimed/armed states plus ready work.
Restart must quarantine/reconcile those effects before scheduling any new external
I/O.

### DB-007 — source-event uniqueness survives restart

Replay duplicate hook/provider/runtime events across 100 restarts. No duplicate
projection transition or obligation.

### DB-008 — backup/restore requires pinned artifacts

Restore DB without artifact required by open obligation. Governor enters explicit
health/repair state and does not pretend obligation processable/closed.

The health state is scoped to the obligation, not to the process. Startup step 8
raises a durable `result_artifact_missing` condition for each obligation whose
pinned artifact will not verify, reports it in `status` and `doctor`, and the
daemon goes on to serve the obligations that are unaffected. What stops the
affected one being processed is not a flag but the artifact store: `read`
verifies digest and length and returns no bytes on a mismatch, so the result
cannot reach review either way. A hard startup refusal is reserved for damage
to the state root itself — the instance lock, the schema epoch, a drifted
migration, a projection that disagrees with its ledger, an unusable artifact
root, filesystem ownership, and the control socket.

---

## Security / privacy tests

### SEC-001 — forbidden-data sentinel sweep

Inject distinct sentinels into prompt, cwd, raw tool args, raw tool results,
command, transcript path, terminal transcript, provider intermediate records,
browser cookies/tokens/headers/bodies, GitHub auth, and environment secrets.

After lifecycle/browser/MCP/crash/restart scenarios, byte-scan:

- SQLite DB/WAL/SHM;
- safe event exports;
- hook inbox;
- managed-run receipts/staging;
- logs/diagnostics/crash metadata;
- generated settings/config;
- CLI status/doctor output.

Expected zero matches. Only the explicit final-result candidate/result artifact may
contain a sentinel deliberately placed in the **final assistant result**.

### SEC-002 — bootstrap metadata minimization

Populate tasks/results with sensitive repository names, refs, worker/session IDs,
and artifact content. Bootstrap response contains none of them; only approved
aggregate attention/health/compatibility metadata.

### SEC-003 — random wake correlation survives attacker knowledge

Attacker knows delivery key, obligation ID, revision, generation, all bootstrap
metadata, and code. Without random delivery ID, resume cannot claim. Property test
covers large generated input set.

### SEC-004 — stale generation/claim cannot mutate

Fuzz stale combinations of binding, obligation version, source fence, claim, and
random delivery ID. Zero unauthorized closure/answer.

### SEC-005 — wake contains no sensitive result data

Generated browser wake contains only protocol marker plus opaque obligation/random
delivery IDs and static instruction. No task/project/worker/result/prompt content.

### SEC-006 — prompt injection cannot become control argument

Worker result/repo data includes forged "ACK", "answer input", fake IDs and policy
instructions. Server never parses them as MCP control fields or user grants.

### SEC-007 — same-user containment is not falsely asserted

Security/doctor metadata reports V1 trust model accurately. Tests do not infer
"safe from worker" from Unix `0600`; hostile same-user sandbox claims require a
future explicit isolation feature.

### SEC-008 — path and symlink race tests

Exercise rooted file operations against traversal/symlink replacement where
platform primitives permit. Imported candidate/artifact integrity mismatch fails
closed.

### SEC-009 — browser/profile credentials never exported

No DB column, safe log, CLI command line, diagnostic bundle, or direct private
write client contains browser credentials.

### SEC-010 — supply-chain policy gates malicious known versions

`cargo deny`/dependency policy rejects known malicious/revoked versions and
unapproved licenses/sources according to project policy.

---

## Live Gate A — ChatGPT MCP mutation capability

Do not run browser wake support against a surface that cannot truthfully mutate
Governor state.

Evidence baseline on 2026-08-31:

- published plan documentation was initially interpreted as a categorical
  read/fetch-only restriction for consumer Pro;
- a live test on the exact target ChatGPT Pro account/app/surface successfully
  performed state-changing Tandem MCP actions and verified a host-filesystem
  mutation by read-back;
- ADR 0006 therefore makes eligibility capability-based, not plan-name-based.

For each candidate bound account/app/surface:

1. record plan/workspace/model/date as diagnostic metadata;
2. install/refresh the exact V1 connector ABI;
3. prove the app/tools are mounted for the turn;
4. execute a harmless synthetic state-changing mutation;
5. read back and correlate the exact committed synthetic record;
6. prove a stale binding-generation mutation is rejected;
7. characterize confirmation/permission behavior without bypass;
8. record `capability_epoch` and re-run after relevant app/account/product/ABI
   changes or repeated action rejection;
9. classify mount, write availability/rejection, confirmation, reachability, and
   ABI failures separately.

Pass: the exact surface can execute the required truthful mutation class under the
current capability epoch and stale-generation fencing works. Fail: the surface is
unsupported for that epoch, no browser-inferred/read-mislabeled fallback exists,
and all real obligations remain open.

## Live Gate B — headed Chrome/CDP

Run the full matrix in [`browser-transport.md`](browser-transport.md), including:

- normal login and three restarts;
- exact conversation binding/wrong-route fencing;
- per-message app selection;
- 10/10 unique random wake submissions;
- semantic CDP/message evidence;
- crashes before/after activation fence;
- ambiguous reconciliation/no replay;
- physical settlement != ACK;
- bounded new-revision resume;
- rebind/stale-generation rejection;
- MCP outage;
- random correlation non-derivability;
- safe-log credential/body scans;
- separate `--headless=new` comparison without stealth/bypass.

One duplicate Send is a gate failure.

---

## Live Gate C — Claude managed execution

Pin exact Claude Code version and invocation, then prove:

1. structured init/session/capabilities are observable;
2. final structured result + child exit produces one result;
3. raw intermediate stream containing tool_use/tool_result is not persisted;
4. daemon-down final result survives via bounded candidate + sanitized receipts;
5. Stop hook fires, another matching hook blocks, work continues, no false
   completion;
6. actual settings/hook source behavior matches adapter assumptions;
7. exact single-tool AskUserQuestion PreToolUse defer produces confirmed
   `tool_deferred` and resumes same session;
8. multi-tool defer is ignored/unsupported and Governor does not project clean
   `needs_input`;
9. non-interactive PermissionRequest really fires under documented conditions and
   its lack/current presence of tool-use correlation is measured;
10. permission decision precedence/settings interactions are recorded;
11. stale Herdr `working/idle:false` cannot veto confirmed result/defer;
12. one fenced clear-busy/continuation path creates no duplicate answer;
13. prompt/tool args/results/command/cwd/transcript secrets are absent from all
    durable safe stores/staging;
14. general Command Governor state-root is not intentionally passed to Claude.

Any divergence updates the adapter contract before support is claimed.

---

## Rust quality and CI gates

From the first implementation commit:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
cargo deny check
```

Also require:

- pinned `rust-toolchain.toml` and committed application `Cargo.lock`;
- migration/replay/failpoint tests in CI;
- macOS first-class CI plus Linux and Windows where implementation permits;
- dependency/license policy;
- no blind auto-merge for security-sensitive dependencies;
- deterministic fake suites do not require live credentials.

### Phase 1 implementation mapping

Where the plan above currently lands in code. A mapping note, not a second plan:
every ID keeps its definition here.

- **Implemented in the deterministic suites** (`governor-testkit` acceptance
  tests, plus the `governor-store-sqlite` and `governor-artifacts` crate suites,
  which carry the single-layer halves): OBL-001 through OBL-010, ART-001 through
  ART-005, DEL-001 through DEL-018, GPT-001 through GPT-009, DB-001 through
  DB-008 within the limit noted below, and SEC-001 through SEC-010. Each suite
  states its own per-ID coverage split.
- **The pattern review's acceptance tests 1 through 12** — the "Acceptance tests
  to add before adapters" section of the [durable orchestration pattern
  review][pattern-review] — are implemented as a deterministic suite of their
  own, covering intent-before-I/O, both crash windows, the four
  mutation-identity cases, the two lease cases, receipt-versus-semantic ACK
  separation, replay equivalence, and the journal's forbidden-data scan.
- **Durable health conditions now have store operations**, so the attention half
  of OBL-006 (conflicting terminal evidence), GPT-006 (`foreman_unreachable` on
  budget exhaustion, and its resolution when a later wake lands) and DB-008
  (`result_artifact_missing`, and its resolution on a successful verify) is
  proven durably rather than in memory. DEL-015's `ambiguous -> accepted`
  promotion likewise has a real store operation, fenced on the exact
  provider-native message identity and performing no Send.
- **ART-006 through ART-011, and every `WRK-` and `INP-` test**, need the
  worker-host, managed-run staging, the hook inbox, or a live Claude session.
  They stay Phase 2 and Live Gate C. Windows ACL policy under ART-005 remains a
  separate platform suite.
- **GPT-010 through GPT-012** are behind Live Gate A: Phase 1 builds no MCP
  client and no connector, so there is no ABI to mismatch, no write capability
  to lose, and no product confirmation to refuse to bypass. The one half
  representable today — a lost write capability never relaxing the ACK
  requirement — is proven in the pure binding machine.
- **DB-005 is fully implemented.** The daemon epoch fence — a
  previous-lifetime holder cannot mutate current ownership — is proven in the
  store suites, and the process half is proven in
  `crates/command-governor/tests/daemon_acceptance.rs` against real spawned
  binaries: a kernel-held advisory lock on the state root elects exactly one
  authority before the database is opened, the second process fails closed, and
  reclaim requires proof the holder is gone — never age. SQLite writer
  serialization is not the election mechanism, exactly as this plan requires.

[pattern-review]: research/2026-08-31-durable-orchestration-pattern-review.md

## Architecture-to-implementation exit criteria

The **pure Rust core/store/testkit scaffold may begin after the architecture PR is
accepted** because it can implement and prove obligations, persistence, fences,
artifacts, random delivery identity, and fakes without claiming a live service
adapter.

The following remain forbidden until their live gate passes:

- supported ChatGPT MCP foreman adapter (Gate A);
- supported ChatGPT browser transport (Gate B);
- supported Claude managed worker adapter (Gate C).

This separation lets Command Governor build the durable kernel without pretending
current external product behavior has already been proven.
