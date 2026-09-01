# Rust Phase-1 crates as migration oracles for the Pi-native Command Governor harness

- **Repository (verified):** `/Volumes/Data/Developer/commandgovernor` — `pwd` and
  `git rev-parse --show-toplevel` both resolve here.
- **Branch:** `feat/pi-native-foundation` @ `8f81b4f`.
- **Date:** 2026-09-01. Read-only throughout; no repository file was modified.
- **Governing decision:** `docs/adr/0008-adopt-pi-native-command-governor-harness.md`.

---

## 0. Executive summary

The Rust scaffold is worth mining for four things, with sharply different values.

1. **The acceptance-ID catalog.** `docs/testing.md` is already a complete,
   implementation-independent specification of 99 numbered acceptance tests in
   eight families, plus 12 numbered pattern-review tests and six SES tests on a
   sibling branch. 64 of those IDs are implemented in `governor-testkit`. Adopt
   the IDs verbatim: an ID that survives the pivot is how a reviewer proves the
   pivot did not quietly drop a requirement.
2. **A small set of algorithms whose pre-images are durable identities** —
   `delivery_key`, the length-prefixed injective absorption underneath it, the
   mutation fingerprint, the resource identity, the worker-loadout digest.
   Frozen test vectors exist. Porting these wrong is silent, not loud.
3. **Ordering contracts encoded as types rather than review rules** — durable
   intent before any external I/O, durable bytes before durable disposition,
   claim before transport, arm before send. This is the largest fidelity loss in
   the migration: TypeScript has no affine types, so every one of these becomes
   *checked* rather than *proven*.
4. **The conformance harness itself.** `governor-testkit` is not a test-support
   crate, it is the shape of the conformance suite: a fault-injection taxonomy,
   a kill-window oracle, whole-store fingerprinting, two domain-separated
   seeded streams, and boundary fakes that *panic rather than act* when the
   durable state does not already authorise them. No gate obsoletes it. It is
   superseded only by a Pi-native successor existing.

Everything else — `rusqlite` specifics, WAL/`synchronous=FULL`, the DB-actor
thread model, migration DDL, the Unix-socket IPC shape, the four-tool `rmcp`
ABI, `chromiumoxide`/CDP — is topology that ADR 0008 §2/§7/§8 demotes.

### A material finding about branch state

**The ADR 0007 lineage/loadout implementation is not on `feat/pi-native-foundation`.**
`grep -ri 'loadout\|lineage'` over `crates/` on this branch returns zero matches.
It lives on two sibling branches:

- `feat/session-lineage-loadout-core` — `crates/governor-core/src/session.rs`
  (1547 lines), `src/digest.rs`, `tests/persisted_digest_vectors.rs`;
- `feat/session-lineage-loadout-store` @ `8cfbcd0` — schema epoch 2,
  `crates/governor-store-sqlite/src/ops/session.rs`,
  `migrations/0002_session_lineage_and_loadouts.sql`,
  `crates/governor-daemon/src/worker.rs`,
  `crates/governor-testkit/tests/ses_acceptance.rs` (779 lines), and
  SES-001..006 plus required invariants 18–22 in the docs.

Two consequences. First, ADR 0008 §4.6 — *"resumed loadouts are explicit and
least-authority; resume cannot silently broaden an old worker under new
defaults"* — is the reliability-contract item with the **most** implemented
oracle material and **none** of it on the branch the pivot is being built on.
Second, `SUPPORTED_SCHEMA_EPOCH` is `1` on this branch and `2` on the sibling
(`crates/governor-store-sqlite/src/migrate.rs:39`), which is itself a live
instance of exactly the drift the epoch gate exists to catch.

**These branches must be read before any Rust crate is archived.**
`git show feat/session-lineage-loadout-store:<path>` suffices; a merge is not
required. Leaving them unmerged *and* unarchived is the state most likely to
lose them.

---

## 1. Method and evidence base

**Documents read in full on this branch:** `docs/adr/0001`–`0008`;
`docs/testing.md`; `docs/state-machines.md`; `docs/data-model.md`;
`docs/worker-lifecycle.md`; `docs/mcp-contract.md`; `docs/browser-transport.md`;
`docs/threat-model.md`; `docs/architecture.md`; `docs/roadmap.md`;
`docs/research/2026-08-31-durable-orchestration-pattern-review.md`;
`docs/research/2026-09-01-pi-native-command-governor-harness-review.md`;
`docs/reviews/2026-08-31-architecture-review.md`.

**Read from sibling branches via `git show` (read-only):** the SES section of
`docs/testing.md`; `docs/data-model.md` §"Session lineage and worker loadouts"
and §"Worker spawn/resume authorization"; `docs/state-machines.md` required
invariants 18–22; `crates/governor-testkit/tests/ses_acceptance.rs`;
`crates/governor-daemon/src/worker.rs`; `crates/governor-core/src/session.rs`;
`crates/governor-core/src/digest.rs`;
`crates/governor-core/tests/persisted_digest_vectors.rs`;
`crates/governor-core/src/error.rs`.

**Crate mining:** four parallel read-only passes with disjoint file ownership —
`governor-core`; `governor-store-sqlite`; `governor-testkit`; and
`governor-artifacts` + `governor-daemon` + `command-governor`. Citations below
carry `file:line`. Sibling-branch citations are marked `@lineage-branch`.

---

## 2. The ADR 0008 gate map

| Gate | ADR 0008 wording | What a test in this gate looks like |
| --- | --- | --- |
| **P1** | pinned Pi release loads the distribution reproducibly; project/global resource precedence characterized; version drift detected | load under a known pin, a drifted pin, and a newer-than-binary epoch; assert the refusal is typed, visible, and non-mutating; *read the resolved configuration back from the runtime* rather than asserting the file said so |
| **P2** | durable subagent lifecycle: spawn, parallel children, role/tool restriction, child input wait, answer/resume, completion, parent restart, orphan handling, result recovery without screen state | drive a child to terminal or blocked, kill the parent, restart, assert the owed work and the child's least-authority loadout both survive |
| **P3** | repeated compaction does not erase exact policy/control constraints; dependent-session tests; memory worker failure does not corrupt task truth | compact N times, then require an action depending on a pinned control fact; separately fail the observer and assert the control plane is unmoved |
| **P4** | ChatGPT Web foreman closed loop: bind, send unique task/revision/delivery event, read the action, validate correlation, durably record disposition, execute exactly once, with injection before/during/after send and before/after disposition | the whole DEL/GPT/OBL family, re-expressed against a Pi transport |
| **P5** | implementer produces work, separate reviewer inspects evidence independently, foreman dispositions, no self-approval | WRK-019, the disposition matrix, the three-ACK-layer separation |
| **P6** | session/role/provider/model cost and cache metrics with provenance; optimize for correctness and fresh/total token efficiency, not cache-hit rate | every metric carries provenance; cost is append-only across forks; diagnostics are a closed set of field types |

Two mapping notes that recur below.

**P1 is broader than its wording.** "Package loading" and "version drift" are the
same machinery that DB-003 (unknown newer schema fails closed), DB-005 (one
authority per state root), and A1 (verify declared configuration by reading it
back) exercise. ADR 0008 §5 explicitly permits Pi-native durable sidecars, and
the moment one exists it inherits the whole "refuse to operate against state you
cannot interpret" family. That is why several store and daemon invariants land
in P1 rather than nowhere.

**P4 absorbs most of ADR 0003/0004.** MCP became optional (§7) and the transport
became replaceable (§8), but every *semantic* they carried — exact correlation,
stale-revision rejection, duplicate idempotence, durable disposition before side
effects — is restated in ADR 0008's "Foreman protocol direction". The tests
move; the assertions do not.

---

## 3. Invariant catalog

Format: **name** — GIVEN/WHEN/THEN — gate(s) — verdict — sources.
Verdicts: **SURVIVES** (product semantics, port it), **BORDERLINE** (port the
property, not the mechanism; reasoning given), **RUST-ONLY** (topology; §3.11).
Where `docs/testing.md` already has an ID, that ID is reused. Keep the IDs.

### 3.1 Delivery ambiguity and at-most-once semantics

The family ADR 0008 names first ("ambiguous-send reconciliation"), and the one
most at risk in the port: `pi-gpt` and `pi-oracle` have different evidence
surfaces than CDP and will tempt a weaker acceptance rule.

**DEL-003 — claim before any external I/O.** GIVEN a scheduled wake; WHEN the
transport is asked to do anything at all; THEN the store already shows the
attempt `claimed`, read through an *independent connection*. A dead attempt
refuses even navigation.
→ **P4**, **P2**. → **SURVIVES.** `docs/testing.md` DEL-003;
`docs/state-machines.md` required invariant 10;
`crates/governor-testkit/tests/del_acceptance.rs:190` and `:216`;
`crates/governor-core/src/outbound.rs:669` (`io_permit()` is `None` unless an
attempt `is_live()`); `crates/governor-store-sqlite/src/ops/delivery.rs:6-13`.
*Port note:* the Rust version is structural — `StorePorts` is lent only to the
pre-`BEGIN IMMEDIATE` phase and the permit type is unconstructible before
`COMMIT` (`crates/governor-store-sqlite/src/tx.rs:27-37`,
`src/writer.rs:262-291`). In TypeScript this must become a fake that reads the
committed store and panics.

**DEL-005 / DEL-007 — the activation fence is durable before the exact send, and
both physical worlds around it behave identically.** GIVEN a claimed attempt;
THEN `send_activation()` is `None` until `activation_armed` is committed, and
`AttemptAccepted` from `Claimed` is `illegal_delivery_transition`. GIVEN a crash
around the fence in the world where nothing was submitted **and** the world where
a message really went out; THEN both become `ambiguous`, both refuse a resend,
neither adds a submission — *because the system cannot tell them apart*.
→ **P4**. → **SURVIVES; top-tier.** `docs/testing.md` DEL-005, DEL-007;
`docs/browser-transport.md` §"At-most-once Send boundary" steps 5–6;
`crates/governor-testkit/tests/del_acceptance.rs:306`, `:348`;
`crates/governor-core/src/outbound.rs:698`;
`crates/governor-store-sqlite/src/ops/delivery.rs:6-13`.

**DEL-006 / DEL-016 / DB-006 — orphans become ambiguous *before* recovery, and
quarantine is all-or-nothing.** GIVEN a previous process left attempts
`claimed`/`activation_armed`; WHEN the harness restarts; THEN they convert to
`ambiguous` **inside the store's own open**, before any caller holds a store, so
no transport supervisor can observe a live attempt. A quarantine that stopped
early would authorise I/O for exactly the attempts whose fate was lost, so a
partial pass is a fail-closed refusal (`QuarantineIncomplete`), not a success
with orphans still holding authorisation.
→ **P2**, **P4**, **P1**. → **SURVIVES.** `docs/testing.md` DEL-006, DEL-016,
DB-006; `docs/architecture.md` §"Startup recovery order" step 4;
`crates/governor-store-sqlite/src/ops/recovery.rs:8-23`, `:33-40`, `:265-277`,
`:290-304`; `crates/governor-testkit/tests/del_acceptance.rs:936`, `:962`;
`crates/governor-testkit/tests/db_acceptance.rs:1009`, `:1069` (273 orphans —
a regression oracle for a former 256 cap that silently broke out and reported
success); `crates/governor-core/src/outbound.rs:847`.

**DEL-008 / DEL-009 — accepted and ambiguous are frozen forever.** GIVEN either
state; WHEN 20 process lifetimes pass a day apart, the browser crashes wholesale,
the daemon restarts five times across hours, an MCP outage occurs, or the turn
settles; THEN zero additional sends. Tested at +1ms, 10s, 24h and `i64::MAX/2`:
there is no timer input, so no amount of waiting unfreezes it.
→ **P4**. → **SURVIVES.** `docs/testing.md` DEL-008, DEL-009;
`docs/state-machines.md` required invariant 13;
`crates/governor-testkit/tests/del_acceptance.rs:418`, `:455`;
`crates/governor-core/src/outbound.rs:800`, `:828`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:701-733`.

**DEL-015 — exact reconciliation is the only promotion, and it emits nothing.**
GIVEN an ambiguous delivery; WHEN provider-native message identity in the exact
bound conversation under the *current* generation is observed; THEN promote to
`accepted` performing **no send** (no live attempt remains, so no permit exists
to act through); the promoted revision stays frozen; an exact repeat is
idempotent; and the promotion replays from its event. A wrong correlation ID and
a wrong conversation both return `unknown_delivery_id` — no oracle about which
half was wrong. A superseded generation is `stale_binding_generation`. The
ordinary outcome path never promotes; absence is not proof of no submission.
→ **P4**. → **SURVIVES; top-tier.** `docs/testing.md` DEL-015;
`crates/governor-store-sqlite/src/ops/delivery.rs:592-639`;
`crates/governor-testkit/tests/del_acceptance.rs:761`;
`crates/governor-core/src/outbound.rs:868`.
*Port note, important:* this promotion is deliberately **narrower** for generic
effects. A generic external-attempt `ambiguous` is **strictly terminal** with no
promotion path at all — `ResolveExternalAttempt` answers `Reconcile`, never a
permit. Browser deliveries are the sole exception because they have an exact
after-the-fact identity to check against.
`docs/data-model.md` §"Consequential external effects";
`crates/governor-store-sqlite/src/ops/effect.rs:36-42`. **A Pi-native port that
gives every transport a promotion path has weakened the contract.**

**DEL-004 / DEL-011 — the fence, not the severity, decides retryability.** GIVEN
a proven pre-submit failure (`TargetNotFound`, `StaleTarget`,
`WrongConversation`, `AppNotSelected`, `ComposerNotReady`, `NavigationBlocked`);
THEN a bounded retry may create the next attempt on the same revision, keeping
the same correlation ID. GIVEN arming has happened; THEN only
`ActivationRefused` and `TransportRejectedBeforeSend` are admissible as proof of
non-submission (`proves_no_submit_after_arming`), everything else is `ambiguous`,
and even a *proven* post-arm failure forbids a retry on that revision
(`retry_after_ambiguity_fence`). The same displacement report that was a safe
pre-fence failure is refused `failure_not_proven` after arming.
→ **P4**. → **SURVIVES; subtle and load-bearing.**
`docs/state-machines.md` §4 "Retry classification"; `docs/data-model.md`
§"Browser wake deliveries"; `crates/governor-core/src/outbound.rs:111-116`,
`:768`, `:782`; `crates/governor-testkit/tests/del_acceptance.rs:239`, `:542`.
*Why this one matters disproportionately:* a post-fence `failed` reads like a
weaker state than `ambiguous` and it is not. It is an equally frozen revision
that happens to carry a truthful outcome.

**DEL-014 — acceptance requires exact semantic evidence; weak signals are not
expressible.** GIVEN composer emptied, URL changed, Stop button appeared,
assistant started, or wake text in the DOM; THEN acceptance evidence cannot be
*constructed* — `AcceptedWakeEvidence` requires exact conversation identity and
exact provider message identity, `WeakBrowserSignal` has no conversion into it,
and there is no `DeliveryOutcome` variant that takes less. Each weak signal
becomes `ambiguous`, records no acceptance, and mints no claim.
→ **P4**. → **SURVIVES as a property; BORDERLINE as a list.** The enumerated
signals are CDP/DOM-specific; under `pi-gpt` the equivalents are "the call
returned 200" and "the conversation has more messages". The portable rule:
*acceptance requires an identity tying this intended message to this
conversation; any signal a different message would also produce is
insufficient.* `docs/testing.md` DEL-014;
`crates/governor-core/src/delivery.rs:212-265`;
`crates/governor-testkit/tests/del_acceptance.rs:717`;
`crates/governor-store-sqlite/src/ops/delivery.rs:387-397`.
*Consequence worth surfacing as an architectural finding:* a transport that
cannot supply exact message identity forces every send into `ambiguous`. That is
correct behaviour, and it may make the direct-API path strictly preferable to
the browser path.

**DEL-010 / DEL-012 — the target and the obligation snapshot are re-verified
immediately before send.** GIVEN a wake bound to conversation A at obligation
version V and source fact S; WHEN the route resolves elsewhere, the chat is
deleted, or the obligation moves to V+1 between staging and activation; THEN no
send occurs — four wrong resolutions plus a deleted chat all fail *before the
composer is mutated*, recorded as `wrong_conversation`; a superseded snapshot is
`stale_delivery_target` with zero rows changed.
→ **P4**. → **SURVIVES.** `docs/testing.md` DEL-010, DEL-012;
`crates/governor-core/src/delivery.rs:511-527`;
`crates/governor-testkit/tests/del_acceptance.rs:493`, `:616`.

**DEL-013 — one revision, at most one physical submission.** GIVEN a failed
attempt then a successful retry; WHEN five restarts plus reconciliation pressure
follow; THEN no second physical message; an accepted attempt is terminal.
`docs/browser-transport.md` states the operational form: *one duplicate Send is
a gate failure*, not a flaky test.
→ **P4**. → **SURVIVES.** `docs/testing.md` DEL-013;
`crates/governor-testkit/tests/del_acceptance.rs:663`.

**At most one revision per obligation may act at a time.** GIVEN any revision
still `pending` or `claimed` — **across binding generations** — ; WHEN a new
revision is created; THEN `delivery_revision_still_live`, zero rows changed.
GIVEN a failed revision with budget remaining; WHEN a successor exists; THEN
claiming on the older one is `delivery_revision_superseded`. **Budget remaining
is not authority to act.**
→ **P4**. → **SURVIVES.** A Phase-1 implementation discovery absent from the
ADRs, and the one an obvious implementation gets wrong.
`docs/data-model.md` §"Create/claim browser delivery";
`crates/governor-store-sqlite/src/ops/delivery.rs:762-793`, `:795-820`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:570-635`, `:637-699`.

**The generic external-effect protocol (pattern-review 1–3).** GIVEN any
consequential external write; THEN the intent row commits in its own transaction
before dispatch, `dispatched` is committed immediately before the adapter call,
and a crash with intent but no proven outcome is `ambiguous` plus a
`reconciliation_required` condition scoped to that attempt — never success,
never failure, never automatic replay. **The pre-dispatch and post-dispatch
crash windows resolve identically, deliberately**, including for a `Read` whose
repeat would be harmless: unknown never projects success. And "we never tried"
stops being admissible proof once the dispatch fence is committed. Progress
requires a *new* attempt, admitted only for a read, a proven-absent effect, or
an idempotent write reproducing the recorded contract **and** exact key — four
near-misses (different key, different contract, class downgraded, same key but
different destination) each refused with the same code, so no caller can vary one
field into a repeat. **There is no "probably safe" class.**
→ **P2**, **P4**. → **SURVIVES; top-tier.**
`docs/research/2026-08-31-durable-orchestration-pattern-review.md` tests 1–3;
`docs/data-model.md` §"Consequential external effects";
`crates/governor-core/src/effect.rs:11-17`, `:277-293`;
`crates/governor-core/tests/durable_execution_invariants.rs:416`, `:476`, `:928`;
`crates/governor-testkit/tests/research_acceptance.rs:97`, `:136`, `:208`.

**No-effect proof must fit the window.** An 8-case table over
`(NoEffectClass, dispatched)`: `NotAttempted` proves absence only *before* the
dispatch fence; `DestinationRefusedWithoutApplying` and
`PreconditionRejectedAtDestination` only *after* it (they are far-end facts);
`RejectedBeforeDispatch` in both. Mismatches → `effect_not_proven_absent`, zero
mutation.
→ **P2**, **P4**. → **SURVIVES.** `crates/governor-core/src/effect.rs:277-293`;
`crates/governor-core/tests/durable_execution_invariants.rs:928`.

### 3.2 Stale-revision rejection and correlation identity

ADR 0008 §4.3: *foreman events and replies are correlated to exact task/revision
identities; stale replies cannot close newer work.*

**OBL-003 — stale binding generation cannot ACK or resume.** GIVEN a claim under
generation N; WHEN the foreman is rebound to N+1 and generation N ACKs; THEN
`stale_binding_generation`, **zero rows changed**, artifact still pinned, exactly
one binding active.
→ **P4**. → **SURVIVES.** `docs/testing.md` OBL-003; ADR 0004 §"ACK semantics";
`docs/state-machines.md` required invariant 9;
`crates/governor-testkit/tests/obl_acceptance.rs:324`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:219-253`;
`crates/governor-core/src/binding.rs:375`, `crates/governor-core/src/claim.rs:519`.

**Unknown ≠ stale generation; no binding is not permission; generations are
never reused.** GIVEN active generation 1; WHEN generation 2 (never issued) is
presented; THEN `unknown_binding_generation`, distinct from
`stale_binding_generation`, so a rebind race is distinguishable from a fabricated
fence. GIVEN an unbound ledger; THEN `no_active_binding` fences everything. GIVEN
a displaced-then-rebound surface; THEN the new generation is strictly higher and
a displaced number is never reissued.
→ **P4**. → **SURVIVES.** `crates/governor-core/src/binding.rs:237-250`, `:359`,
`:401`, `:429`.

**OBL-004 — a displaced claim holder is told `stale_claim`, not
`expired_claim`.** GIVEN a claim that expired and was reclaimed by a different
claim; WHEN the original holder ACKs; THEN ACK fences the claim the *obligation*
currently records **before** rehydrating the presented row, so the honest answer
is that it was displaced. Zero rows changed; state still `Processing`; version
unchanged; artifact still pinned.
→ **P4**, **P5**. → **SURVIVES** as a property (a displaced holder must be told
it was displaced, not merely that it timed out); **BORDERLINE** as the specific
code choice. `docs/state-machines.md` §1 "Claim order inside ACK";
`docs/data-model.md` §"Foreman claims"; `docs/testing.md` OBL-004;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:1428-1470`;
`crates/governor-testkit/tests/obl_acceptance.rs:363`.

**OBL-009 / SEC-004 — every fence is checked, varied one field at a time.** GIVEN
a valid ACK; WHEN each of {obligation identity, version, source event, binding
generation, claim, disposition} is varied independently; THEN each is refused
with its **own** typed conflict code and zero rows changed, and the exact request
then closes the work. SEC-004 escalates to the full cross-product — 3 generations
× 3 versions × 2 sources × 2 claims × 2 dispositions = 72 combinations, 71
refused with zero mutation, 1 exact one closing.
→ **P4**. → **SURVIVES; strongest form of stale-revision rejection.**
`docs/testing.md` OBL-009, SEC-004;
`crates/governor-testkit/tests/obl_acceptance.rs:688`;
`crates/governor-testkit/tests/sec_acceptance.rs:362`.
*The one-field-at-a-time discipline is the value:* a conjunction test passes when
only one fence is actually checked.

**Zero mutation on rejection is structural, not per-operation discipline.** GIVEN
any refused transition; THEN nothing observable changed. In `governor-core` every
`apply` borrows `&self` and returns a new value. In the store,
*"a body that returns `Err` never reaches step 4 and the transaction is
dropped... A rejected fence therefore changes zero rows, and that is a property
of this function rather than of every operation remembering to undo itself."*
→ **P4**. → **SURVIVES as a design requirement.**
`crates/governor-core/src/lib.rs:24-26`;
`crates/governor-store-sqlite/src/writer.rs:29-34`.

**SEC-003 / DEL-001 / DEL-018 — deterministic identity is not possession.** GIVEN
an attacker holding the obligation ID, binding generation, revision, the
deterministic `delivery_key`, the wake payload digest, the full bootstrap
response and the source code; WHEN they attempt to claim; THEN every attempt
fails `unknown_delivery_id` with zero mutation, while the real correlation ID
succeeds — proving the guesses failed for the right reason. Exercised at scale:
~130 structured guesses in the core suite, 64 independently seeded state roots in
the testkit, and exactly 256 counted attacker attempts in SEC-003 (the count
itself asserted, so the property was actually exercised).
→ **P4**. → **SURVIVES.** This is the invariant with the clearest history of
being got wrong: architecture review **R3** records that the original design used
one deterministic hash for both jobs and *"claimed security it did not have"*.
`docs/reviews/2026-08-31-architecture-review.md` §R3; ADR 0003 §"Delivery
identity"; ADR 0004 §"Alternatives"; `docs/testing.md` DEL-001, DEL-018, SEC-003;
`crates/governor-core/tests/state_machine_invariants.rs:907`, `:994`;
`crates/governor-testkit/tests/del_acceptance.rs:56`, `:1018`;
`crates/governor-testkit/tests/sec_acceptance.rs:277`.

**A claim needs the accepted, current-generation wake and its correlation ID —
and the refusal discloses nothing.** GIVEN four distinct failure causes (wrong
generation, wrong obligation, wake not accepted, correlation mismatch); THEN all
four report the **same** `unknown_delivery_id`, so a connector in another
conversation cannot use the error to learn whether a delivery exists.
→ **P4**, **P5**. → **SURVIVES.** `crates/governor-core/src/claim.rs:8-11`,
`:255-265`, `:435`, `:463`, `:494`;
`crates/governor-store-sqlite/src/ops/delivery.rs:634-639`.

**DEL-017 / GPT-004 — a resume is a new revision with a new random
correlation.** GIVEN an accepted, settled, un-ACKed obligation past backoff; THEN
the resume is revision +1 with a new deterministic key and an independently
random correlation ID; the old revision stays immutable.
→ **P4**. → **SURVIVES.** `docs/testing.md` DEL-017, GPT-004;
`docs/state-machines.md` §7; `crates/governor-testkit/tests/del_acceptance.rs:978`;
`crates/governor-testkit/tests/gpt_acceptance.rs:156`;
`crates/governor-core/tests/state_machine_invariants.rs:788-798`.

**GPT-007 / GPT-008 / SEC-002 — bootstrap is low-information, structurally.**
GIVEN an unrelated conversation with the connector available; THEN it may learn
compatibility, active generation, health, and aggregate attention
kinds/counts/priority/age/wake state, and nothing else — the view is assembled
from `COUNT`/`MAX`/`MIN` only, and *no query selects an identity column*. The
**whole rendered value** is searched for 13 forbidden strings, so a field added
later that leaked one fails here rather than in review. A non-emptiness check
ensures "discloses nothing" isn't true for the wrong reason.
→ **P4**, **P6**. → **SURVIVES** as a property; the specific field list is
ABI-shaped and therefore **BORDERLINE** in detail. Origin: architecture review
**R6**. `docs/testing.md` GPT-007, GPT-008, SEC-002; ADR 0004 §"Bootstrap
confidentiality"; `crates/governor-testkit/tests/gpt_acceptance.rs:382`, `:440`;
`crates/governor-testkit/tests/sec_acceptance.rs:239`.
*Port note:* ADR 0008 §6/§7 removes the requirement that ChatGPT call a Governor
server at all. If Pi reads the reply directly there is no bootstrap surface and
the *disclosure* half becomes moot — but the *possession* half (SEC-003) does
not, because a Pi transport can still be pointed at the wrong conversation. Keep
SEC-003; re-scope GPT-007.

**Stale source fact is a fence independent of stale version.** GIVEN a caller
presenting the right version but a source fact the obligation has moved past;
THEN `stale_source_fence`, zero rows changed. Two independent fences: *which
revision* and *which underlying fact*. A Pi foreman reply must fail both ways.
→ **P4**. → **SURVIVES.**
`crates/governor-store-sqlite/tests/store_lifecycle.rs:194-217`;
`crates/governor-core/src/obligation.rs:1146`.

**WRK-018 / stale session incarnation.** GIVEN a worker re-attached as
incarnation 2; WHEN a delayed result from incarnation 1 arrives; THEN
`stale_session_incarnation`, retained for history/quarantine only, current turn
unchanged.
→ **P2**. → **SURVIVES.** `docs/testing.md` WRK-018;
`docs/state-machines.md` required invariant 8;
`crates/governor-core/tests/state_machine_invariants.rs:583`.
*Port note:* Pi sessions are resumable/forkable, so "which incarnation" is a
question Pi's own session identity may seem to answer — but ADR 0007 §4 is
explicit that a provider session string is never the identity fence. Adopting
Pi's primitive is correct **and** a Governor-owned incarnation fence is still
required.

**Stale command revision, stale foreman turn generation.** GIVEN a worker
continuation at revision R; WHEN resumed-turn evidence for another revision (or
a different answered input) arrives; THEN `stale_command_revision`. GIVEN a
foreman turn observed under generation 1; WHEN an observation from a stale
generation arrives; THEN it is rejected.
→ **P2**, **P4**. → **SURVIVES.**
`crates/governor-core/src/worker_command.rs:225-242`, `:432`;
`crates/governor-core/src/foreman_turn.rs:325`.

**Conflict codes are stable, unique, and machine-classifiable.** GIVEN the full
enumeration (46 kinds on this branch, 51 on the lineage branch); THEN every code
is distinct and `snake_case`, and callers branch on the code, never on a
formatted string. A test named `every_session_refusal_has_a_stable_code` exists
to keep them stable.
→ **P4**, **P6**. → **SURVIVES.**
`crates/governor-core/tests/state_machine_invariants.rs:1466-1518`;
`@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:759`. Full
vocabulary in §4.5.

### 3.3 Duplicate idempotence

**OBL-005 / DB-007 / B10 — a duplicate source identity is success returning the
first result, not a rejection and not a second transition.** GIVEN a confirmed
terminal source fact `(namespace, event_id, fence)`; WHEN it is replayed 25 more
times in one lifetime and 10× in each of 10 process incarnations (100 deliveries
in OBL-005, 100 restarts in DB-007); THEN every replay reports `duplicate: true`,
the version does not advance, and exactly one terminal event, one artifact row,
one obligation and three obligation transitions exist.
→ **P2**, **P4**. → **SURVIVES.** `docs/testing.md` OBL-005, DB-007;
`docs/state-machines.md` required invariant 3;
`crates/governor-store-sqlite/src/event.rs:1-13`, `:276-294`, `:302-351`
(`ON CONFLICT DO NOTHING` then read back the existing seq; the caller **must
not** apply a second transition); `crates/governor-store-sqlite/tests/store_lifecycle.rs:55-89`,
`:91-119`, `:121-152`; `crates/governor-testkit/tests/obl_acceptance.rs:433`;
`crates/governor-testkit/tests/db_acceptance.rs:1168`;
`crates/governor-core/tests/state_machine_invariants.rs:440`;
`crates/governor-core/src/fence.rs:357` (`SourceLedger`, the pure form of the
durable unique index).
*Port note:* the mechanism is cheap and portable — a unique constraint on a
derived non-secret source identity. **The derivation rule must port with it:**
*"a provider that lacks one must derive it deterministically from stable
non-secret facts such as turn ID + provider-native sequence/tool-use ID + event
class. Never hash prompt, transcript, tool arguments, or result content merely
to manufacture an event identity."* (`docs/data-model.md` §"Immutable event
ledger".)

**DEL-002 / D26 / D27 — duplicate scheduling converges on one row and one
correlation ID, and a bounded retry keeps both.** GIVEN a revision scheduled;
WHEN a *different process with its own CSPRNG* schedules the same logical
revision; THEN the deterministic key finds the same row, `created == false`, the
previously generated correlation ID is kept **for the revision's whole life**,
and exactly one physical revision exists. A candidate `delivery_id` is drawn
unconditionally before the transaction (a transaction body cannot reach a CSPRNG)
and **discarded on the found path**, so a duplicate schedule cannot rotate a live
wake's correlation ID.
→ **P4**. → **SURVIVES.** `docs/testing.md` DEL-002;
`docs/data-model.md` §"Create/claim browser delivery";
`crates/governor-store-sqlite/src/ops/delivery.rs:19-26`, `:110-118`, `:137-152`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:446-489`, `:491-549`;
`crates/governor-testkit/tests/del_acceptance.rs:131`.

**Mutation journal: completed replays, uncertain never redispatches, and reuse
for different work is a typed mismatch.** GIVEN `(actor_id, command_id,
fingerprint)` whose row is `completed`; WHEN an exact retry arrives; THEN the
recorded safe result is returned with **zero dispatch** (proven 5× with the
destination never reached, and across restart). GIVEN `received` or `uncertain`;
THEN `mutation_result_uncertain` forever, in this process and every later one —
and `MutationDisposition`/`MutationAdmission` have **no dispatch variant**, so
redispatch is inexpressible. GIVEN the same identity with a *different*
fingerprint; THEN `mutation_command_mismatch` — the recorded result is withheld
rather than misapplied. A different command id, or the same id under a different
actor, is genuinely new work. Startup turns any previous-epoch `received` row
into `uncertain`; an `uncertain` row may reach `completed` only through late
*proven* evidence, which is a record and never a retry.
→ **P2**, **P4**. → **SURVIVES.** Pattern-review tests 4, 5, 6;
`docs/data-model.md` §"Mutation-command journal";
`crates/governor-core/src/mutation.rs:684`, `:697`, `:745`, `:839`, `:847`;
`crates/governor-core/tests/durable_execution_invariants.rs:552`, `:576`, `:601`,
`:635`; `crates/governor-store-sqlite/src/ops/mutation.rs:23-27`, `:74-85`;
`crates/governor-store-sqlite/tests/store_durability.rs:282-332`, `:334-367`,
`:401-430`, `:432-468`; `crates/governor-testkit/tests/research_acceptance.rs:331`,
`:366`, `:399`.
*Port note:* `fingerprint` is the part a re-derivation loses. Without it a client
reusing a command id for a different operation is handed the first operation's
result — *"a wrongly replayed result dressed up as idempotency"*. It is a digest
of the fenced parameters, **never the parameters**.

**Idempotent ACK is exact-match only, on all five fields.** GIVEN a committed
closing ACK; WHEN an identical request repeats (version, source, generation,
claim, disposition all matching); THEN idempotent success, **not** a stale-version
conflict. A differing caller cannot ride the earlier ACK's idempotency, and a
different stale ACK cannot rewrite the disposition. The obligation's-claim check
is skipped only in the already-closed case, which is precisely the idempotent
repeat.
→ **P4**. → **SURVIVES.** ADR 0004 §"ACK semantics";
`docs/mcp-contract.md` §"ACK validation"; `docs/data-model.md` §"Foreman claims";
`crates/governor-core/src/obligation.rs:322-340` (`CommittedAck::matches`), `:1206`;
`crates/governor-core/src/claim.rs:670`.

**Duplicate delivery events, orphan quarantine, and health raises are all
idempotent.** Re-arming an armed attempt, a repeat `AttemptAccepted` with the
same evidence, a repeat `AttemptFailed` with the same class, a repeat
`AttemptAmbiguous`, and `OrphanQuarantined` with nothing live all report
`Duplicate`. A second recovery pass reports `ambiguous_attempts == 0` and the
condition count stays 1 — no duplicate attention. Raising an already-open
condition of the same kind and scope is idempotent; resolving an absent one is a
no-op; each ambiguous attempt gets **its own** condition scoped to the exact
attempt.
→ **P2**, **P4**, **P6**. → **SURVIVES.**
`crates/governor-core/src/outbound.rs:491`, `:541`, `:577`, `:593`, `:902`;
`crates/governor-core/src/health.rs:311`, `:347`, `:372`, `:412`;
`crates/governor-store-sqlite/src/ops/recovery.rs:228-245`;
`crates/governor-store-sqlite/tests/store_durability.rs:228-238`.

**WRK-017 — hook-inbox replay is idempotent across the cleanup crash window.**
GIVEN a crash after ingest but before inbox cleanup; WHEN the harness restarts
and reimports; THEN no duplicate event or obligation.
→ **P2**. → **SURVIVES as a property; BORDERLINE as a mechanism.** The inbox is
a Claude-hook-specific durability shim (ADR 0005). Pi's extension event surface
plus a durable task spool may make it unnecessary — but the property (*an
out-of-process observation deposited while the consumer is down is imported
exactly once*) is exactly what a Pi-native supervisor must prove.
`docs/testing.md` WRK-016, WRK-017.

**Concurrency converges.** GIVEN concurrent identical writers; THEN one
transition. GIVEN concurrent independent writers; THEN each commits exactly once.
Replay still matches in both cases.
→ **P2**, **P4**. → **SURVIVES.**
`crates/governor-store-sqlite/tests/writer_stress.rs:17-19`, `:58-60`.

### 3.4 Durable disposition before side effects

**The permit is produced strictly after the intent is observable to an
independent reader.** GIVEN an intent recorded; WHEN the permit is handed to an
adapter; THEN that adapter, reading through *its own connection*, finds the row —
or panics rather than acting. This is mechanised, not asserted. GIVEN a crash
before the intent is durable; THEN no permit, zero rows, and nothing to
reconcile (`ambiguous_attempts == 0`). One logical operation cannot produce two
intent rows (a plain `INSERT` against a primary key, never an upsert), therefore
cannot produce two permits; the permit is a single-use, non-`Clone`, by-value
capability consumed on use.
→ **P2**, **P4**. → **SURVIVES; top-tier.** This is ADR 0008's *"durable
disposition before worker side effects"*, and the **mechanisation** — the adapter
genuinely looks — is the part worth copying.
`docs/data-model.md` §"How 'no external I/O inside a transaction' is enforced";
pattern-review test 1; `crates/governor-store-sqlite/src/tx.rs:27-37`;
`crates/governor-store-sqlite/src/writer.rs:262-291`;
`crates/governor-store-sqlite/src/ops/effect.rs:22-27`, `:29-34`, `:82-84`,
`:129-135`; `crates/governor-store-sqlite/tests/store_durability.rs:74-142`,
`:144-174`; `crates/governor-core/src/effect.rs:1072`, `:1084`, `:1103`, `:1121`;
`crates/governor-core/tests/durable_execution_invariants.rs:369`;
`crates/governor-testkit/tests/research_acceptance.rs:97`.

**No external I/O inside a state transaction, enforced by shape.** GIVEN a
state-changing transaction; THEN no browser, network, worker, provider or
filesystem call occurs while it is held. Everything ambient — clock, CSPRNG,
identity minting — lives behind one `StorePorts` value lent only to the phase
that runs *before* `BEGIN IMMEDIATE`; the transaction body's signature takes no
ports, so inside a transaction there is nothing to call. A third phase runs
strictly after `COMMIT` returns and exists for exactly one thing: surrendering
the acceptance that authorises an external permit. The crate performs no
filesystem, network or process I/O at all, and **a test scans the crate's own
source** for `std::fs`, `std::net`, `std::process`, `SystemTime`, `getrandom`,
`reqwest`, `tokio`.
→ **P1**, **P2**, **P4**. → **SURVIVES as a rule; BORDERLINE as a mechanism.**
The type-level half does not port; the source-scan trick is cheap and does.
`docs/data-model.md` §"How 'no external I/O inside a transaction' is enforced";
`crates/governor-store-sqlite/src/ports.rs:1-29`, `:38-44`;
`crates/governor-store-sqlite/tests/store_privacy.rs:607-642`;
`crates/governor-core/src/lib.rs:20-23`.

**ART-001 — durable bytes before durable disposition, made unfalsifiable.** GIVEN
crash injection at every artifact failpoint crossed with every reachable store
failpoint (32 cells in the testkit, 7+3 in the crate suite); THEN the forbidden
outcome — *a committed `completed_unprocessed` referencing bytes that are not
durable and verifiable* — never occurs, checked **before any recovery and again
after a restart**; the obligation completes exactly when the publication did.
Allowed residue: an unreferenced orphan file. Forbidden: a referenced missing
one. The ordering is enforced by a type: `PublishedArtifact` has private fields
and **exactly one construction site**, the line after the directory fsync, and
`DurableArtifact` is obtainable only from it — so the transaction that opens
`completed_unprocessed` cannot execute unless the bytes are already durable.
→ **P2**. → **SURVIVES; top-tier.** `docs/testing.md` ART-001;
`docs/data-model.md` §"Crash-safe result publication";
`crates/governor-artifacts/src/lib.rs:56-62`;
`crates/governor-artifacts/src/store.rs:139-200`, `:293-365`;
`crates/governor-artifacts/tests/artifact_durability.rs:82-199`, `:203-260`;
`crates/governor-testkit/tests/art_acceptance.rs:52`.
*The generalisable idea:* **make "durable" a token only the durability step can
mint.** That is language-agnostic even where affine types are not.

**Immutability has no overwrite path, because the primitive refuses.** GIVEN a
key already published; WHEN publish targets it; THEN `link(2)` fails `EEXIST` →
`AlreadyPublished`, and the original bytes are byte-identical afterwards.
`rename(2)` was rejected *specifically because it silently replaces* — verified
empirically on this platform — so it cannot express "immutable, no overwrite
path", while `link(2)` makes immutability a property the filesystem enforces
rather than one the caller is trusted not to violate.
→ **P2**. → **SURVIVES.** `docs/data-model.md` §"Crash-safe result publication"
(Deviation, step 5); `crates/governor-artifacts/src/store.rs:31-41`, `:321-331`;
`crates/governor-artifacts/tests/artifact_integrity.rs:190-214`.
*The portable lesson:* the atomic-publish primitive must be the one that
**refuses**, not the one that clobbers.

**Publication verifies the bytes that are there, not the bytes handed in.** GIVEN
bytes substituted after the directory fsync; THEN the post-condition re-read
denies the proof with `Integrity`, no transaction follows, and the file becomes
an orphan. Closes short writes, unsurfaced device errors, and same-user rewrite
in one pass.
→ **P2**. → **SURVIVES.** `crates/governor-artifacts/src/store.rs:341-356`;
`crates/governor-artifacts/tests/artifact_integrity.rs:264-299`.

**ART-003 / ART-012 — integrity failure returns no bytes at all, and missing is
not empty.** GIVEN a tampered, truncated or extended artifact; THEN the read
errors and the caller receives **zero bytes** — bytes and error are not both
representable — and the obligation stays open, claimed and pinning. GIVEN a
deleted artifact; THEN it reads as `Missing`, never as zero bytes. The read is
bounded at `expected_len + 1`: enough to detect growth, never enough for a
tampered row to drive allocation; a recorded length beyond the root's own bound
is refused before opening.
→ **P2**, **P5**. → **SURVIVES.** `docs/testing.md` ART-003;
`crates/governor-artifacts/src/store.rs:367-422`, `:392-422`, `:399-406`;
`crates/governor-artifacts/tests/artifact_integrity.rs:48-133`, `:111-133`,
`:136-165`, `:168-187`;
`crates/governor-testkit/tests/art_acceptance.rs:260`.
*Why this matters for P5 specifically:* a reviewer that receives a truncated
result together with a warning will use the truncated result. An LLM reviewer
certainly will.

**ART-002 — an open obligation pins retention, and pinning is derived, never
asserted.** GIVEN GC attempted before ACK, during a claim, after physical
settlement, after claim expiry, and across a restart — **with retention grace set
to zero**, so survival proves pinning rather than a timer; THEN all report
`Pinned` and the bytes remain. Only a valid closing disposition releases.
`retention_state` is recomputed from the obligations that actually reference the
artifact on every transition, so nothing can release an artifact an open
obligation still needs.
→ **P2**, **P4**. → **SURVIVES.** `docs/testing.md` ART-002;
`docs/state-machines.md` required invariant 2;
`docs/data-model.md` §"Who writes `eligible_for_delete_at_ms`";
`crates/governor-artifacts/src/gc.rs:1-16`, `:190-225`;
`crates/governor-artifacts/tests/artifact_retention.rs:68-169`;
`crates/governor-testkit/tests/art_acceptance.rs:127`;
`crates/governor-store-sqlite/src/replay.rs:310-315`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:371-401`;
`crates/governor-core/src/artifact.rs:257`, `:264`, `:274`.

**The retention delay is applied once, by the closing transaction, and never
recomputed by the sweep.** The sweep's configured grace is deliberately **not
consulted** by `collect`, so changing the knob cannot retroactively move the
deletion time of already-closed work. The release instant is written with
`COALESCE`, so a repeated or idempotent ACK cannot push an already-released
artifact's deletion further out. An eligible artifact with **no** recorded
instant is kept forever (`ReleaseInstantUnknown`) — the grace cannot be
evaluated, and failing closed there costs disk while guessing costs a result.
This is the live state after user cancellation.
→ **P4**, **P6**. → **SURVIVES.** Two policy authorities would eventually
disagree about when bytes disappear.
`docs/data-model.md` §"Who writes `eligible_for_delete_at_ms`";
`crates/governor-artifacts/src/gc.rs:27-35`, `:36-41`, `:123-142`;
`crates/governor-artifacts/tests/artifact_retention.rs:172-231`, `:234-306`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:1143-1207`.

**Orphans are quarantined, never deleted; quarantine never overwrites earlier
evidence; a future mtime reads as age zero and is therefore kept.** A slow
publication and a crashed one look identical, so an unreferenced file older than
the orphan grace is *moved*, and nothing in the crate deletes a quarantined file.
Two orphans with the same name are two pieces of evidence, so a free name is
found rather than letting `rename(2)` destroy the first. A clock jump or a
deliberate `utimes` must not license quarantine. The sweep is idempotent and its
clock is injected, never `SystemTime::now` — a sweep whose behaviour depends on
how fast the machine ran cannot be tested.
→ **P2**, **P6**. → **SURVIVES.**
`crates/governor-artifacts/src/gc.rs:18-25`, `:216-223`, `:227-281`, `:307-328`,
`:358-377`; `crates/governor-artifacts/tests/artifact_paths.rs:344-373`;
`crates/governor-artifacts/tests/artifact_durability.rs:263-300`, `:303-335`.

**OBL-001 — completion cannot disappear before ACK.** GIVEN a confirmed terminal
result with durable bytes; WHEN the daemon restarts three times, the runtime
session closes and is deleted, a ChatGPT turn settles, and a foreman claim
expires; THEN the obligation stays `completed_unprocessed` and open, its artifact
stays pinned, and only a fully fenced ACK closes it — with the **whole-database
fingerprint** asserted unchanged across the runtime-observation and settlement
steps.
→ **P2**, **P4**, **P5**. → **SURVIVES.** This is ADR 0001's central invariant
and ADR 0008 §4.1/§4.2. `docs/testing.md` OBL-001;
`crates/governor-testkit/tests/obl_acceptance.rs:66`;
`crates/governor-core/tests/state_machine_invariants.rs:338`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:255-279`.

**OBL-002 — restart preserves every open attention state.** GIVEN an obligation
driven to each of `Created`, `Running`, `Failed`, `CompletedUnprocessed`,
`ClaimedByForeman`, `Processing`; WHEN the process restarts; THEN state, version,
source fence, claim and artifact are byte-identical and projection verification
passes. An expired internal claim may return to the prior attention state but
never closes work.
→ **P2**, **P4**. → **SURVIVES.** `docs/testing.md` OBL-002;
`crates/governor-testkit/tests/obl_acceptance.rs:194`.
*Coverage honesty worth porting:* the `needs_input` half is **not** proven
durably on this branch — the evidence classifies but no store write path reaches
it, and `obl_acceptance.rs:281` says so rather than skipping silently.

**Closing is enumerated, and only a disposition closes.** GIVEN any obligation
event; THEN the only events producing a closed state are `ForemanAcked` (with a
valid disposition), `CancelledByUser`, and `Superseded`. Every accepted
transition strictly increases the version, which is what makes a caller's
`expected_version` a real fence. An obligation held by a live claim refuses a
second claim (`obligation_already_claimed`).
→ **P4**, **P5**. → **SURVIVES.** `docs/state-machines.md` required invariant 1;
`crates/governor-core/src/obligation.rs:8-12`, `:1011`, `:1286`, `:1311`;
`crates/governor-core/src/claim.rs:697`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:342-369`.

**The disposition matrix.** GIVEN an obligation claimed from attention state A;
WHEN disposition D is presented; THEN it is accepted only if `D.closes(A)`:
`completed_unprocessed` → {Accepted, RejectedNeedsRework, Abandoned};
`failed` → {RejectedNeedsRework, FailureAcknowledged, Abandoned};
`needs_input` → {Abandoned} only. A success disposition cannot close a failure, a
failure disposition cannot close a result, and an outstanding input request
closes only by abandoning it — because answering it is not a closure; the worker
may not have received the answer.
→ **P4**, **P5**. → **SURVIVES.** ADR 0004 §"The disposition set";
`docs/data-model.md` §"Obligations";
`crates/governor-core/src/obligation.rs:128-146`, `:1301`;
`crates/governor-core/tests/state_machine_invariants.rs:1117`.

**ACK enters from `processing` only; undeliverable work stays owed.** GIVEN
`foreman_resume` minted a claim but the artifact or input detail could not then
be handed over (the read happens *after* the claim transaction and can fail);
THEN the obligation stays `claimed_by_foreman`, no handoff event exists, and ACK
is refused with an illegal-transition conflict and zero mutation. A live claim
cannot be expired; an expired claim cannot deliver a handoff.
→ **P4**, **P5**. → **SURVIVES.** A Phase-1 clarification of a diagram ambiguity
— precisely what a re-derivation from ADRs alone would lose.
`docs/state-machines.md` §1; `crates/governor-store-sqlite/tests/store_lifecycle.rs:1276-1299`,
`:1301-1358`.

**Claim expiry is coordination, not a decision about the work.** GIVEN a lapsed
claim; WHEN expired; THEN the obligation returns to **exactly** the attention
state the claim was taken from, the version advances, the work stays open, the
claim is released durably, no closing disposition is recorded, and the artifact
stays pinned with no deletion instant. Expired claims are **history, not
deletions** — two rows survive a reclaim, the first `expired`, the second `live`.
→ **P4**, **P5**, **P6**. → **SURVIVES** (review provenance must be append-only).
`docs/state-machines.md` §1 "Expiry, as implemented";
`crates/governor-store-sqlite/src/ops/claim.rs:20-25`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:1209-1274`, `:1396-1426`;
`crates/governor-core/tests/state_machine_invariants.rs:1182`.

**OBL-007 / OBL-008 / GPT-001..003 / research-10 — three distinct facts, and no
accumulation of receipts closes work.** GIVEN a wake accepted, then the assistant
settling, then a successful resume that returns **every artifact page** and then
disconnects; THEN the obligation is open in all three cases and reclaimable after
the claim lapses. GIVEN five acknowledged mutation receipts *and* an accepted
delivery *and* a completed external effect; THEN the work is still
`completed_unprocessed` and open, before and after reopen — structurally, because
there is **no code path** from receipt-ACK to obligation-ACK.
→ **P4**, **P5**. → **SURVIVES; top-tier.** `docs/testing.md` OBL-007, OBL-008,
GPT-001..003; `docs/state-machines.md` required invariant 14;
`crates/governor-testkit/tests/obl_acceptance.rs:619`, `:642`;
`crates/governor-testkit/tests/gpt_acceptance.rs:80`, `:100`, `:120`;
`crates/governor-testkit/tests/research_acceptance.rs:470`, `:513`;
`crates/governor-store-sqlite/tests/store_durability.rs:472-566`;
`crates/governor-store-sqlite/src/ops/mutation.rs:29-35`;
`crates/governor-core/src/foreman_turn.rs:3-7`.

**The three ACK layers stay separate, structurally.** Layer 1 `ReceiptAck` →
journal retention/compaction only; layer 2 claim → responsibility; layer 3
semantic disposition → closure. `ReceiptAck` carries only
`(ActorId, MutationCommandId)` — no obligation, claim, generation or disposition
— implements no conversion toward an ACK request, and no obligation event accepts
one. A receipt ACK requires a committed result and works only for its own
identity.
→ **P4**, **P5**. → **SURVIVES; this is the single most important invariant to
port.** `docs/research/2026-08-31-durable-orchestration-pattern-review.md` §"The
three ACK layers must remain separate"; pattern-review tests 9, 10;
`docs/data-model.md` §"Mutation-command journal";
`crates/governor-core/src/mutation.rs:25-45`, `:755`, `:767`, `:807`;
`crates/governor-core/tests/durable_execution_invariants.rs:779`, `:818`, `:843-857`.
*Why:* ADR 0008 lists `@geminixiang/pi-task-protocol` as a first candidate and it
has acknowledgements — **layer 2**. If the foreman disposition is mapped onto a
package's built-in ACK, the product's defining invariant is lost at the moment of
composition, and every later gate tests the wrong thing. **This needs an explicit
conformance test in the composition review, not a code comment.** Note also that
ADR 0008 §6, by removing MCP, makes exactly this confusion easier to fall into:
"the transport confirmed delivery" starts to look like "the foreman reviewed".

**GPT-006 — the resume budget exhausts safely.** GIVEN the configured automatic
resumes spent; THEN exactly one physical send per resume, one durable
`foreman_unreachable` condition scoped to the **obligation** (not the daemon),
50 further timer ticks scheduling nothing and changing nothing, the obligation
open indefinitely (checked a month later), the condition surviving restart, and
an **accepted delivery resolving it** — because a landed delivery is the evidence
the foreman was reachable. Operator-driven resume is distinguished from the
exhausted automatic budget.
→ **P4**. → **SURVIVES.** No infinite wake loop, no silent drop.
`docs/testing.md` GPT-006; `docs/state-machines.md` §7;
`crates/governor-testkit/tests/gpt_acceptance.rs:241`.

**GPT-005 — never overlap an active or unknown foreman turn.** GIVEN a turn in
`Starting`, `Active` or `ObservationLost`; THEN each blocks a wake and names
which; zero deliveries created, transport untouched, whole-database unchanged.
`IdleUnknown` and `Settled` permit one. **Unknown blocks, like active.**
→ **P4**. → **SURVIVES.** `docs/testing.md` GPT-005;
`docs/state-machines.md` §6; `crates/governor-testkit/tests/gpt_acceptance.rs:202`;
`crates/governor-core/src/foreman_turn.rs:297`, `:311`.

**GPT-009 — the exact wake claims once, deterministically.** GIVEN the exact
accepted wake; THEN one claim; WHEN 10 repeat attempts follow; THEN each is
refused with the **same** code every time (determinism, not order-dependence); a
stale version with the right correlation ID is refused; and an expired claim
yields to exactly one successor.
→ **P4**, **P5**. → **SURVIVES.**
`crates/governor-testkit/tests/gpt_acceptance.rs:485`, `:561`.

### 3.5 Configuration, version and drift gating — the P1 core

**A1 — a declared setting must be read back from the runtime, not assumed from
having been set.** GIVEN a store issues its required connection policy; WHEN it
verifies; THEN every setting is re-queried **from the engine** — and on a second
independent connection — and any disagreement is a fail-closed refusal, never a
log line. The stated reason: *"Issuing the `PRAGMA` statements is not the same as
having them in force — `journal_mode` silently refuses to change for an in-memory
database or one held open by another connection."*
→ **P1**. → **SURVIVES; this is the single most directly transferable invariant
in the workspace.** Gate P1's *"project/global resource precedence
characterized"* is this rule applied to Pi: the resolved set of extensions,
roles, skills and settings must be **reported and asserted against an expected
manifest**, not inferred from the flags passed.
`crates/governor-store-sqlite/src/open.rs:1-9`, `:98-99`, `:121-154`;
`crates/governor-store-sqlite/tests/store_policy.rs:18-44`, `:39-43`.
It has a documented precedent in the worker layer: **WRK-020** requires that no
code path promotes a signal *"merely because a settings flag was assumed to
isolate hooks"* (`docs/testing.md` WRK-020;
`docs/worker-lifecycle.md` §"Configuration isolation"). Same failure mode, new
substrate.

**A2 — a declared setting must be proven to have effect, not merely to be
reported on.** GIVEN foreign keys report as on; WHEN a dangling reference is
inserted; THEN the engine actually rejects it. *"The pragma being on is only
interesting if the engine acts on it."*
→ **P1**. → **SURVIVES.** The Pi analogue: prove a loaded extension's restriction
actually blocks the thing it claims to block.
`crates/governor-store-sqlite/tests/store_policy.rs:46-66`.

**DB-003 / A3 — a newer state version is refused before anything else is read or
written.** GIVEN a database at epoch 99 and a binary supporting epoch 1; WHEN
opened; THEN `SchemaEpochTooNew { found: 99, supported: 1 }` — no downgrade, no
mutation, the event count and the epoch string both unchanged after the refusal —
and it is refused on **every** open (three rounds, not a one-shot). The gate runs
first and reads nothing else, *"so an older binary cannot so much as inspect a
newer database's tables."*
→ **P1**, **P3**. → **SURVIVES; the highest-value single invariant for the
foundation PR.** `docs/testing.md` DB-003; `docs/data-model.md` §"SQLite policy";
`crates/governor-store-sqlite/src/migrate.rs:22-26`, `:96-106`;
`crates/governor-store-sqlite/tests/store_policy.rs:113-150`, `:138-149`;
`crates/governor-testkit/tests/db_acceptance.rs:871`;
`crates/governor-daemon/src/doctor.rs:348-359` (the doctor's reporting half:
`check name=schema_epoch result=newer_than_this_binary detail=found_N_supported_M`).

**A4 — version compatibility is a monotonic epoch, decoupled from the migration
counter.** Each migration carries `version` *and* `epoch` separately.
→ **P1**. → **SURVIVES.** A Pi package set needs a compatibility epoch distinct
from a version number, so *"many pins, one compatibility generation"* is
expressible. `crates/governor-store-sqlite/src/migrate.rs:42-55`.

**A5 / A6 — applied state-shape definitions are content-hashed, and both drift
directions fail closed.** GIVEN a migration recorded with checksum X; WHEN the
binary carries a different definition of the same version; THEN
`MigrationChecksumMismatch`. GIVEN a recorded version this binary does not
implement; THEN `UnknownAppliedMigration` — a refusal, not a skip. All
fail-closed errors share one `is_fail_closed()` predicate the daemon branches on.
→ **P1**. → **SURVIVES, and generalises hard.** This *is* ADR 0008's *"pinned Pi
release loads the distribution reproducibly ... version drift detected"*:
content-hash the loaded extension set and refuse on drift; a distribution that
finds state written by a package it no longer carries must refuse, not silently
ignore. `crates/governor-store-sqlite/src/migrate.rs:57-64`, `:115-127`;
`crates/governor-store-sqlite/src/error.rs:124-133`;
`crates/governor-store-sqlite/tests/store_policy.rs:91-111`;
`crates/governor-daemon/src/doctor.rs:438-455` and
`crates/governor-daemon/src/startup.rs:612-627` (the taxonomy, identical in both:
`schema_epoch_too_new`, `migration_checksum_mismatch`,
`unknown_applied_migration`, `connection_policy`, `corrupt_value`,
`repair_needed`, `quarantine_incomplete`, `writer_gone`,
`unreadable_needs_owner_recovery`).
→ The *distinctions* — "you are behind" vs "you are ahead" vs "history was
rewritten" — all have Pi analogues in package pinning. The specific SQLite
conditions are **BORDERLINE**.

**DB-004 / A7 — applying a version step is atomic with recording that it was
applied.** GIVEN a crash at each migration failpoint; THEN the interrupted
migration hands back **no store**, the point actually fired, reopening applies it
cleanly, and a third open *verifies* rather than reapplies. *"There is no window
in which the schema has moved but the ledger of migrations has not."*
→ **P1**. → **SURVIVES** (transactional DDL is RUST-ONLY; the property is not).
`docs/testing.md` DB-004; `crates/governor-store-sqlite/src/migrate.rs:12-19`,
`:171-197`; `crates/governor-store-sqlite/tests/store_policy.rs:152-170`;
`crates/governor-testkit/tests/db_acceptance.rs:914`.

**A8 — one process lifetime, one monotonic epoch, advanced once, before recovery
reads it.** GIVEN three successive opens; THEN epochs 1, 2, 3, each advanced in
its own transaction before recovery.
→ **P1**, **P2**, **P4**. → **SURVIVES.** A Pi harness needs a process-generation
fence so pre-restart intents are distinguishable from this process's.
`docs/data-model.md` §"Meta and migrations";
`crates/governor-store-sqlite/src/store.rs:163-170`;
`crates/governor-store-sqlite/tests/store_policy.rs:172-184`.

**A9 — the startup order is fixed and unskippable by construction.** Open and
prove policy → epoch gate → migrate → advance epoch → **verify replay
equivalence** → **quarantine lost effects** → only then hand back a usable store.
Steps 5 and 6 are unskippable because *"`Store` is only reachable through
`OpenStore::start`, and there is no constructor that takes a connection."*
→ **P1**, **P2**, **P4**. → **SURVIVES — the ordering *and* the structural
unskippability.** An extension that can be constructed without having run its
recovery pass has this invariant only as documentation.
`crates/governor-store-sqlite/src/store.rs:1-19`.
The daemon layer's own thirteen-step order sits above it
(`docs/architecture.md` §"Startup recovery order";
`crates/governor-daemon/src/startup.rs:1-33`, `:158-336`), with the recorded
deviation that Phase 1 runs replay validation *before* quarantine inside one
uninterleavable open, while the binding requirement — quarantine before any *new
external* I/O — is preserved either way. **That distinction (which orderings are
load-bearing and which are incidental) is exactly what a port needs told
explicitly.**

**Every startup step is a typed refusal, never a warning, and a refusal means
nothing was scheduled.** Codes: `authority_held`, `lock_holder_still_alive`,
`state_root_invalid`, `store_refused`, `ipc_unavailable`, …
→ **P1**. → **SURVIVES.** Machine-classifiable refusal codes are exactly what a
conformance harness asserts on.
`crates/governor-daemon/src/startup.rs:34-37`;
`crates/governor-daemon/src/error.rs:145-153`.

**A knowable-up-front impossibility is preflighted before durable state
changes.** GIVEN a socket path too long to bind; THEN it is checked **before the
store opens**, so an impossible pathname cannot advance the daemon epoch and run
recovery before failing — proven end-to-end: after the refusal the database file
does not exist.
→ **P1**. → **SURVIVES.** *Validate everything knowable before you mutate
anything* is a first-class P1 property, and it generalises directly to Pi package
preconditions. `crates/governor-daemon/src/startup.rs:176-180`;
`crates/governor-daemon/src/ipc.rs:44-49`, `:144-153`;
`crates/command-governor/tests/daemon_acceptance.rs:477-507`.

**Waiting is bounded, because waiting forever would hide a two-authority
situation.**
→ **P1**, **P2**. → **SURVIVES.** `crates/governor-store-sqlite/src/open.rs:20-23`.

**SEC-010 — the dependency policy is proven to be *in force*, not merely
written.** GIVEN `deny.toml`; THEN it has all four gating sections **and CI
actually runs** `cargo deny` and `cargo audit`. The honest framing is that this
catches *"a policy file that quietly stops being run."*
→ **P1**. → **SURVIVES, translated.** ADR 0008 requires every Pi dependency to be
*"pinned, reviewed, licensed, and exercised by Command Governor conformance
tests"*; this is the test that the pinning policy is enforced.
`docs/testing.md` SEC-010; `crates/governor-testkit/tests/sec_acceptance.rs:708`.

### 3.6 Restart recovery, single authority, and process identity

**F41 — restart quarantines every effect whose fate was lost, before scheduling
anything new.** Three families, one rule each: a live delivery attempt →
`ambiguous`; a `received` mutation → `uncertain`; a recorded intent with no
outcome → `ambiguous` plus `reconciliation_required` scoped to the exact attempt.
**Automatic replay: never, in all three.** Quarantine records uncertainty and
fabricates no terminal state — the task itself is untouched, still open, and
replay still matches exactly. Conservative quarantine is preferred to guessing:
*duplicate avoidance wins over guessing*, and both crash windows give the same
answer.
→ **P2**, **P3**, **P4**. → **SURVIVES; top-tier.** `docs/testing.md` DB-006;
`crates/governor-store-sqlite/src/ops/recovery.rs:8-23`, `:17-20`;
`crates/governor-store-sqlite/tests/store_durability.rs:241-264`, `:249-264`,
`:570-638`.

**DB-005 — exactly one authority per state root, and reclaim requires proof.**
GIVEN two real OS processes against one state root; THEN exactly one obtains
authority *before the database is opened*, and the second fails closed with a
machine-classifiable class naming the holder, never becoming ready. **Store-level
writer serialization is explicitly not the election mechanism** — two daemons
would both open legitimately, both advance the epoch, both run quarantine, both
replay, and the store would merely *order* them. Serialization is not exclusion.
Reclaim requires proof the holder is gone, never age.
→ **P1**, **P2**. → **SURVIVES as a requirement on any Pi-native durable
helper.** ADR 0008 §5 permits helpers that outlive the interactive process; the
moment one exists, this applies to it. The topology (a `daemon.lock` file) is
Rust-only; *"the store's concurrency control is not your authority protocol"*
transfers verbatim to Pi sidecars sharing a task store.
`docs/testing.md` DB-005; `docs/threat-model.md` §"Threat: two daemons become
authorities"; `crates/governor-daemon/src/lock.rs:1-18`;
`crates/command-governor/tests/daemon_acceptance.rs:236-269`.
**And the falsifying test is the valuable one:** started against a state root
with **no database yet**, the second daemon still refuses — there is no writer to
serialize against — and the epoch afterwards is still `1`, proving no second
authoritative open occurred (`daemon_acceptance.rs:271-296`). That is a
measurement that distinguishes two mutually exclusive mechanisms rather than
confirming one.

**The kernel-held lock is the authority; the record is corroboration; the lock
file is never unlinked.** No timeout, no lease to expire; the kernel releases on
exit, panic, kill or power loss. Unlinking is what makes lock files race — two
processes could hold kernel locks on different inodes and both believe they are
authoritative.
→ **P1**. → **BORDERLINE → SURVIVES.** The mechanism is POSIX-specific; the
*separation of authority from diagnosis* is the portable design.
`crates/governor-daemon/src/lock.rs:19-34`, `:36-44`, `:133-166`.

**The four-way reclaim decision.** Held → `AuthorityHeld`, fail closed, no
waiting, no partial authority. Free + record says `held` + the holder's
incarnation still re-derives → `LockHolderStillAlive`: **ambiguity is not a
takeover**. Free + `released`, or the PID resolves to nothing, or start identity
mismatched → reclaim, *reporting how it differed*. Bytes that are not a lock
record → refuse, do not overwrite. Kill → the record still says `held`, and the
next start reclaims and reports it; clean stop releases, marks `released`,
removes the socket, and the next start is unremarkable.
→ **P1**, **P2**. → **SURVIVES.** Directly reusable for any Pi durable-task
supervisor doing orphan reconciliation.
`crates/governor-daemon/src/lock.rs:46-64`, `:172-205`, `:460-597`;
`crates/command-governor/tests/daemon_acceptance.rs:298-324`, `:326-351`.

**Process identity is PID plus a *re-derivable* start identity, and "cannot tell"
is never treated as "gone".** A random per-process nonce would distinguish our own
runs but could never answer *"is the process that wrote this record still the one
under that number?"* — the question stale reclaim turns on. Linux
`/proc/<pid>/stat` field 22; macOS `ps -o lstart=`, hashed to an opaque token.
`start_ref` returns `None` for both "no such process" and "platform cannot
answer", **deliberately undistinguished**, because a caller treating the second
as the first would be reclaiming on ignorance.
→ **P2**. → **SURVIVES.** A crisp instance of *do not build inference on a signal
you cannot validate*. PID reuse is a real hazard for Pi subagent orphan handling.
`crates/governor-daemon/src/incarnation.rs:1-29`, `:71-87`, `:78-128`.

**Lease fencing (pattern-review 7, 8).** GIVEN a lease held by slot 4242 /
start-a; WHEN a recycled slot 4242 / start-b presents the **correct persisted
token**; THEN both renew and release fail `stale_process_incarnation`, classified
`SlotReused`, and the real holder still owns the resource. GIVEN a forged token →
`stale_lease_token`; an unrelated process holding the right token →
`stale_process_incarnation`; the right holder from a superseded lifetime →
`stale_daemon_epoch`. Each fails both renew and release and changes nothing.
**Even after expiry**, an older epoch acquiring is `stale_daemon_epoch`: expiry is
a *liveness hint, not an authority* — takeover is what invalidates the old token,
not the passage of time. A takeover mints a fresh token; two acquisitions of the
same resource by the same process under the same epoch produce different tokens;
`Debug` renders `LeaseToken(<redacted>)`.
→ **P1**, **P2**. → **SURVIVES.** Exactly the orphan/takeover semantics Gate P2
needs for subagent process supervision.
`docs/data-model.md` §"Resource leases";
`crates/governor-core/src/lease.rs:20-27`, `:29-35`, `:791`, `:805`, `:829`,
`:859`, `:878`, `:903`, `:952`, `:965`, `:980`, `:998`;
`crates/governor-core/tests/durable_execution_invariants.rs:649`, `:684`, `:720`,
`:747`; `crates/governor-store-sqlite/src/ops/lease.rs:21-31`;
`crates/governor-store-sqlite/tests/store_durability.rs:653-707`, `:709-788`.
*Two details easy to lose:* the canonical resource **name is never stored** — a
path, socket location or profile directory is forbidden durable control-plane
data, so identity is a namespace plus a digest of the name; and the possession
token is raw bytes with no text form, *"so it cannot reach a log line through a
formatter."*

**Scoped failure vs fatal failure is an explicit taxonomy.** **Fatal** = damage to
the *state root*: the instance lock, the schema epoch, a drifted migration, a
projection that disagrees with its ledger, an unusable artifact root, filesystem
ownership, and the control socket. **Scoped** = one obligation's artifact fails
verification: raise a durable `result_artifact_missing` condition, record a safe
diagnostic, and keep serving — refusing the whole daemon would take every
unrelated obligation down with it. Proven end-to-end: 3 open obligations, 1
pinned artifact deleted → the daemon runs, `artifacts.unverified=1`,
`health.kind.result_artifact_missing=1`, all 3 obligations still listed and
reachable, the affected one still open.
→ **P1**, **P2**, **P5**. → **SURVIVES; the highest-value operational judgement
in the workspace.** "One bad artifact halts the world" and "one bad artifact is
silently skipped" are both wrong; this splits them correctly and names the small
set of state-root damage that does justify refusing to start.
`docs/testing.md` DB-008; `crates/governor-daemon/src/startup.rs:38-57`,
`:551-596`; `crates/command-governor/tests/daemon_acceptance.rs:356-435`.

**DB-008 — and what keeps a damaged obligation unprocessable is a mechanism, not
a flag.** The health condition is *visibility*; the actual enforcement is that
the artifact read verifies digest and length on every read and hands back **no
bytes at all** on mismatch — *"not by a flag anyone has to remember to check."*
The condition is idempotent in both directions (a verified artifact resolves it,
an unverifiable one raises it), survives restart, and is cleared **only on a
successful verify, never a guess**. A missing file is not a corrupt ledger, so
the store still opens.
→ **P1**, **P2**, **P5**. → **SURVIVES; the strongest single answer to "how do
you stop unreviewable work reaching review".** `docs/testing.md` DB-008;
`crates/governor-daemon/src/startup.rs:52-57`, `:561-590`;
`crates/governor-testkit/tests/db_acceptance.rs:1232`;
`crates/command-governor/tests/daemon_acceptance.rs:420-434`.

**An uninterpretable committed reference refuses startup rather than silently
dropping out of the reference set.** GIVEN a `storage_ref` that passes generic
column validation but is not a legal storage key; THEN it would otherwise vanish
from the orphan sweep's `committed` set and let the sweep quarantine bytes the
ledger still references — so it is a corrupt-value refusal instead, preserving
the bytes in place, unquarantined. **The reclassification set must be complete
before anything is reclassified.**
→ **P1**, **P2**. → **SURVIVES.** This is the structural ancestor on this branch
of the "verify the launch configuration, and never sweep it away" rule.
`crates/governor-daemon/src/startup.rs:258-275`;
`crates/command-governor/tests/daemon_acceptance.rs:437-475`.

**F42 — a recovery pass that cannot complete must refuse to serve.** The identity
pool is sized from a pre-transaction count; exhaustion rolls back with
`QuarantineIncomplete` and the daemon refuses. *"An attempt left `claimed` or
`activation_armed` still satisfies `io_permit`, so a quarantine that stopped
early would authorise browser I/O for exactly the attempts whose fate the restart
lost — invariant 12, inverted."*
→ **P1**, **P2**, **P4**. → **SURVIVES.** Generalised: *never return success from
a recovery pass with orphans still holding authorisation.*
`crates/governor-store-sqlite/src/ops/recovery.rs:33-40`, `:265-277`, `:290-304`;
`crates/governor-testkit/tests/db_acceptance.rs:1069`.

**Liveness is a round trip, never a file check; a running daemon that does not
answer is a failed check.** A socket file can outlive the process that bound it.
Lock says held + socket silent → `daemon_reachable=socket_did_not_answer`.
→ **P1**, **P2**, **P6**. → **SURVIVES.** Directly applicable to Pi
session-liveness checks. `crates/governor-daemon/src/ipc.rs:327-334`;
`crates/governor-daemon/src/doctor.rs:224-231`.

**ART-006 / ART-007 / ART-008 — the result survives the authority being absent,
and the transport shim owns no truth.** GIVEN the worker-host runs while the
authoritative daemon is absent; THEN it writes one bounded final-result candidate
plus sanitized receipts and exits; on restart exactly one confirmed terminal
result becomes one immutable artifact and one open obligation. GIVEN a crash at
every point before a complete final result and matching exit receipt; THEN
reconciliation/failure attention, never `completed_unprocessed` from partial
bytes. GIVEN valid candidate and receipts with the daemon never started; THEN no
task or obligation projection exists until the authority imports and reconciles
them.
→ **P2**. → **SURVIVES as properties; BORDERLINE in topology.** ADR 0008 §2
removes `governor-worker-claude`-style adapters and §5 permits extension-owned
durable sidecars, so the shim survives conceptually as whatever Pi-native helper
outlives the interactive process. **ART-008's real content — the shim has no
protocol path that writes lifecycle state — is the load-bearing half.**
`docs/testing.md` ART-006..008; ADR 0005; `docs/threat-model.md` §"Threat:
worker-host becomes a second control plane".

### 3.7 Replay determinism and projection equivalence

**DB-001 / G51 — every derived view is rebuildable from the authoritative log,
and a disagreement fails closed on startup and keeps failing closed.** GIVEN 24
seeded branching lifecycle sequences; THEN projections are verified after **every
step**, not only at the end, and again in a fresh process. GIVEN a projection row
edited behind the ledger's back; THEN `RepairNeeded` naming the disagreeing
column — obligation state, a renumbered binding ladder, a flipped binding
activity flag, a claim lifecycle — *and it does not repair itself*. **All**
disagreements are collected, not just the first, *"so an operator sees the shape
of the damage rather than one symptom."*
→ **P1**, **P2**, **P3**. → **SURVIVES.** The tamper tests are the valuable part:
each names a *specific* corruption nothing else would catch. Pattern-review 11;
`docs/testing.md` DB-001; `crates/governor-store-sqlite/src/replay.rs:1-16`,
`:14-16`; `crates/governor-store-sqlite/tests/store_lifecycle.rs:770-808`,
`:810-845`, `:847-880`, `:882-932`;
`crates/governor-testkit/tests/db_acceptance.rs:60`;
`crates/governor-core/tests/state_machine_invariants.rs:1414`.

**G57 — the fenced value replay checks against is the one the caller actually
presented.** Recording `expected_version` *"lets replay **check** the fold rather
than feed the machine whatever version the fold happens to be at, which would
make the compare-and-swap trivially true on every replay."* Each transition's
version is checked against an immutable per-transition record, so a **missing**
transition cannot hide either; the verifier re-folds prefix-by-prefix.
→ **P4**. → **SURVIVES; a measurement-validity invariant.** This is CLAUDE.md's
*"a measurement that cannot come back negative is not a measurement"* encoded in
a schema. **The conformance harness must record the presented fence, not the
derived one.** `crates/governor-store-sqlite/src/event.rs:220-224`;
`crates/governor-store-sqlite/src/replay.rs:19-22`, `:226-267`.

**G53 — what is *not* replay-derived is named explicitly, with reasoning, and
re-proved at a different boundary.** Three residues are enumerated with the reason
each cannot fold and what protects it instead: delivery scheduling fields (the
row's `delivery_key` is re-derived from `(obligation, generation, revision)` on
every read); binding target identity (not carried in allowlisted safe metadata,
so nothing in the ledger can be compared with it); claim correlation ID
(a possession fence, never written to safe metadata) and `expires_at_ms` (a clock
reading, not a ledger fact). The mutation journal, external attempts and resource
leases are outside DB-001 by design — each loader re-folds its own row's recorded
history through the domain machine on every read and refuses a row no legal
sequence of transitions can reach.
→ **all gates**. → **SURVIVES as a documentation discipline.** *A harness that
claims "we replay everything" without enumerating what it cannot compare has made
the weaker claim look like the stronger one.* This document already does the
enumeration; carry it over.
`docs/testing.md` DB-001 (coverage and residue);
`crates/governor-store-sqlite/src/replay.rs:26-58`;
`crates/governor-store-sqlite/src/lib.rs:26-40`.

**G56 / C21 — replay compares *decoded* values, and the compare-and-swap folds
the ledger rather than trusting the projection row.** An unknown stored label is
corruption, not a silent mismatch. *"The obligation's current state is folded
from its events rather than trusted from its projection row, so the value a
transition is applied to was necessarily built by the state machine."*
→ **P1**, **P3**, **P4**. → **SURVIVES** as reframed: *the value a fence is
checked against must be derived from the authoritative record, not from a
convenience cache a memory or summary layer could have touched.* That reframing
makes it a genuine P3 invariant.
`crates/governor-store-sqlite/src/event.rs:369-379`;
`crates/governor-store-sqlite/src/replay.rs:433-438`, `:813-817`, `:856-861`.

**G58 — a verified-through watermark is recorded and readable by the next
process, and is reported as history rather than as a check this process
performed.** *"A watermark far behind the ledger head says the last process
stopped before it could finish verifying."* The doctor emits it as a **note**,
explicitly not a check, because proving replay equivalence writes the watermark
and a read-only diagnosis may not write.
→ **P1**, **P6**. → **SURVIVES; a clean instance of refusing to claim a check you
are structurally unable to perform.**
`crates/governor-store-sqlite/src/replay.rs:133-141`;
`crates/governor-store-sqlite/src/store.rs:108-112`;
`crates/governor-daemon/src/doctor.rs:361-376`.

**Determinism is itself a meta-invariant.** One seed produces an identical event
stream **and** identical whole-database state; eight seeds produce eight distinct
correlation IDs and eight distinct durable states; identity and randomness are
**two domain-separated streams**, asserted independent for 1024 seeds — *"or
every possession-fence assertion is vacuous."* The restart seed stride is
`1_000_003`, because a real CSPRNG never repeats across restarts and a harness
that did would hide exactly the bug the suites hunt: two "different" correlation
IDs that are in fact equal.
→ **P1**. → **SURVIVES; the whole suite rests on it.**
`crates/governor-testkit/tests/determinism.rs:103`, `:115`, `:126`, `:147`;
`crates/governor-testkit/src/rng.rs`; `crates/governor-testkit/src/harness.rs:45`.

### 3.8 Loadout immutability, fail-closed resume, lineage durability

ADR 0008 §4.6. **Every source in this subsection is `@lineage-branch`.**

**SES-003 / I78 — immutability is the primary key, not a rule.**
`capability_profiles`, `delegation_policies`, `model_policies` and
`worker_loadouts` are keyed on `(identity, digest_hex)`, and *"this module
contains no `UPDATE` statement for any of them. Editing a role file therefore
**inserts a second snapshot**, and the composite foreign keys mean every loadout
that embedded the first one still resolves to the first one."* GIVEN a capability
profile widened under the same identity; THEN the profile table holds **two**
rows, the launch snapshot still reads back as its original capability set, the
widened fence is refused, and the original fence still resumes under the original
set. Making the profile writer replace entries in place must break the test.
→ **P2**, **P5**. → **SURVIVES; top-tier.** ADR 0007 §4 and §"Alternatives /
Resume workers from the current role definition";
`@lineage-branch docs/data-model.md` §"Session lineage and worker loadouts";
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:9-19`;
`@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:305`.

**I80 — one loadout per incarnation, forever.** Rebinding an incarnation to the
same snapshot converges; rebinding to a *different* one is a typed refusal
(`SessionIncarnationAlreadyBound`), not an update — *"widening a live session's
sandbox is exactly what the fence exists to prevent; a new revision needs a new
incarnation."*
→ **P2**. → **SURVIVES.**
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:~690-700`;
`@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:392`.

**SES-001 / I81 — the fence is re-proved under the write lock: byte-identical or
nothing.** The composition root verifies outside the transaction (reads the row,
re-derives the digest, re-hashes the config bytes); the store then re-reads **the
same facts under the write lock** and refuses on any difference — identity
mismatch → `LoadoutIdentityMismatch`, digest mismatch → `LoadoutDigestMismatch`,
config drift → `ManagedConfigUnverifiable` — *"which is what makes the
outside-the-transaction verification sound rather than merely optimistic."* GIVEN
a fence the binding does not name (the state a superseding write between
verification and commit would leave); THEN refusal with **zero rows changed**.
→ **P2**, **P1**. → **SURVIVES; top-tier**, and it directly answers CLAUDE.md's
*"confirm the check can SEE the thing"*.
`@lineage-branch docs/testing.md` SES-001;
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:~836-845`,
`:~890-925`; `@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:40`,
`:110`.

**SES-002 — a missing or corrupt managed configuration fails closed, and the
check is a byte read, not a metadata comparison.** Two arms: the file **deleted**,
and the file **rewritten in place**. Both refuse the resume, raise a
session-scoped `managed_config_missing`, produce no permit and no intent, and
leave the loadout tables byte-identical. *"The corrupt arm is the one that
matters: the row's `sha256_hex` is unchanged, so a metadata-only check passes.
Replacing the byte read with the recorded metadata must break it."* The
verification step therefore reads the bytes and hashes them **now**, and
`ManagedConfigVerified::verify` demands a digest and length *observed now* —
passing the recorded digest straight back in would prove nothing.
→ **P2**. → **SURVIVES; the sharpest single test in the repository.**
`@lineage-branch docs/testing.md` SES-002;
`@lineage-branch docs/data-model.md` §"Worker spawn/resume authorization" step 3;
`@lineage-branch crates/governor-daemon/src/worker.rs` module docs;
`@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:166`, `:278`.

**I40 — spawning a worker is a non-idempotent write with *no* idempotency
contract, so no automatic retry is admissible.** GIVEN a spawn intent quarantined
by restart; THEN a worker process **may exist**; the answer is reconciliation —
find the process, or start a new incarnation with its own loadout binding — *"and
never a silent respawn under the old intent. There is no operation here that
resolves a quarantined spawn into a second permit, and there is no code path that
could produce one."*
→ **P2**. → **SURVIVES; arguably the single most important invariant for Gate
P2.** Encode it before writing a subagent spawner.
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:21-35`.

**Capabilities and delegation are whitelist-only; an empty set grants nothing.**
*"There is deliberately no implicit/default capability set. `new(id, [])` grants
nothing, which prevents an omitted profile from becoming 'full tools.'"* A worker
may spawn only roles its delegation policy lists; omitting a child role cannot
mean "give it the default profile".
→ **P2**. → **SURVIVES.** ADR 0007 §6;
`@lineage-branch crates/governor-core/src/session.rs` module docs and
`CapabilityProfile`.

**Two constructors, deliberately not interchangeable.** `WorkerLoadout::resolve`
is a resolve-time computation over freshly resolved parts;
`CommittedLoadout::rehydrate` is the **only** path from a persisted row, and it
re-derives the digest and refuses a row whose safe fields no longer agree with it.
Only a `CommittedLoadout` can admit a resume, so *"a loadout assembled at run time
cannot stand in for the one a session was launched under."* The digest is derived
from the rows that are written, never accepted from the caller — *"two copies of
one set are two things that can disagree."*
→ **P2**, **P5**. → **SURVIVES.**
`@lineage-branch crates/governor-core/src/session.rs:22-35`;
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:255-263`.

**I82 — and the limits of that check are stated rather than overclaimed.**
Rehydration is a *self-consistency* check: *"It does not, and cannot, prove the
row is one this store wrote: authenticity is the schema's job ... and the resume
path's, through the `ManagedConfigVerified` witness that comes from bytes the row
does not control."*
→ **P2**, **P5**. → **SURVIVES, including the honesty about scope.**
`@lineage-branch crates/governor-store-sqlite/src/replay.rs:155-170`.

**SES-004 — lineage survives restart and is rebuilt from the event alone.** GIVEN
A → B → C; THEN replay rebuilds both edges from `session_lineage_recorded` alone,
holding across **100 restarts** with parents, delegating turns and relations
checked each round; re-issuing an identical record converges without writing a
row or an event. Removing `parent_turn` or `relation` from the event's allowlisted
metadata must break it — the parent identity travels in the metadata, *"which is
what makes `compare_lineage` a genuine fold rather than a re-read of the row it is
checking."* The event is scoped to the **child**: scoping to the parent would make
one parent's slice grow without bound while the child said nothing about its own
provenance.
→ **P2**, **P6**. → **SURVIVES.** ADR 0007 §5 — *"the runtime may lose its
process tree and still not erase this lineage."*
`@lineage-branch docs/testing.md` SES-004;
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:~786-800`;
`@lineage-branch crates/governor-store-sqlite/src/replay.rs:257-300`;
`@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:435`.

**SES-005 — lineage is acyclic at every hop count, the walk is bounded, and
"did not finish looking" is a separate answer from "no cycle".** A → B → C is
three legal constructor calls; making C the parent of A closes a cycle only a walk
of the whole chain can see, refused `session_lineage_cycle` with zero rows
changed. The one-hop self-parent case is *a different guard in a different layer*
and is asserted independently. `MAX_LINEAGE_DEPTH = 64` *"is not an optimisation
and must not be removed"* — the recursive walk has no cycle detection of its own,
and a restored backup is not proof no cycle is already in the table. Exhaustion is
reported **separately** from the cycle answer: *"a walk that stopped at the bound
did not finish looking, so reporting 'no cycle' from it would be reporting a
conclusion the query did not reach."*
→ **P2**. → **SURVIVES.** The depth rationale is SQLite-shaped, but any store
needs *some* bound, and *exceeding it is a typed refusal, never a truncation* is
a general conformance principle worth lifting into the Pi suite's design notes.
`@lineage-branch docs/testing.md` SES-005;
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:82-91`,
`:~980-1050`; `@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:543`,
`:602`.

**SES-006 — parent-turn ownership is derived by the store, never presented by the
caller, and an unknown turn is refused identically.** GIVEN session A as parent
with a turn drawn from unrelated session Z; THEN the foreign key on
`parent_turn_id` is satisfied and the schema alone permits the row, so a two-hop
`turns → session_incarnations → sessions` join refuses it inside the edge-insert
transaction. An unknown turn identity is refused identically **so probing reveals
nothing**, and the parent's own turn is still accepted.
→ **P2**, **P5**. → **SURVIVES.** The "refused identically" half is a security
property a naive port drops.
`@lineage-branch docs/testing.md` SES-006;
`@lineage-branch crates/governor-store-sqlite/src/ops/session.rs:~930-975`,
`:946-951`; `@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:693`.

**I86 — managed configurations are pinned with no releaser, and that is asserted
rather than left as writer habit.** `retention_state` is always `pinned` and
`eligible_for_delete_at_ms` always NULL, enforced by CHECK constraints and
asserted by a replay comparison — *"this is the half that would catch a future
migration relaxing them without a releaser to go with it."* A configuration is
pinned by any loadout any session was ever launched under, and epoch 2 defines no
releaser, so releasing one would be a guess.
→ **P2**, **P3**. → **SURVIVES.**
`@lineage-branch crates/governor-store-sqlite/src/replay.rs:203-255`.

**ADR 0007 §7 — child completion creates durable parent-facing work.** GIVEN a
parent delegates; THEN a durable child session, lineage and child obligation exist
**before** the external spawn is authorised; a confirmed final result plus durable
artifact creates a parent/foreman-facing result obligation requiring explicit
disposition; a child that needs input parks in a durable input state and routes by
policy; and parent death, runtime restart or UI closure cannot strand or erase
that request.
→ **P2**. → **SURVIVES.** This is the exact scope of Gate P2 and the invariant
that decides whether a chosen Pi subagent package is adoptable as-is.

**Sibling required invariants 18–22** restate the above as five numbered product
rules and should be carried across verbatim alongside 1–17
(`@lineage-branch docs/state-machines.md:458-474`).

### 3.9 Memory is not authority; compaction is type-aware

ADR 0008 §4.5 and §9; Gate P3. This family is almost entirely **specification**
— it has the least implemented oracle and therefore the highest risk of being
re-derived loosely.

**Three layers, and only one is authority.** Layer A = immutable events,
projections, obligations, fences. Layer B = a **deterministic, disposable**
control capsule, a bounded exact rendering of current control facts regenerable
from A at any time. Layer C = model-generated observational memory. A is never
model-summarised or compacted away; B can always be regenerated; C is advisory.
→ **P3**. → **SURVIVES.** ADR 0007 §8.
*Port note:* **layer B has no Rust implementation — it is spec only.** It is also
the natural answer to "what does Pi re-inject after its own compaction", and
therefore probably the first genuinely new Pi-native component. Building it is a
prerequisite for making P3 testable at all.

**Observational memory cannot do seven things.** GIVEN any observer or
consolidator output; THEN it cannot close or ACK an obligation, authorise a
capability, replace a current source/version/generation fence, reconstruct secret
possession/correlation values, override a user-owned decision, or convert silence
or stale runtime state into terminal truth.
→ **P3**, **P4**. → **SURVIVES.** ADR 0007 §8. Each clause is a separate negative
test and they are cheap to write.

**Attention is never terminal state, structurally.** GIVEN any health condition;
THEN it closes nothing, releases no artifact, moves no turn, schedules no wake —
*"and structurally it cannot"*, because the only row it writes is the condition
table and the only machine it drives has no path elsewhere. Attention is refused
for already-closed work (zero events appended) and must name the artifact the
task actually pins. `suspected_stall` is deliberately **not** an obligation state
but a condition layered on `running`, so it has no way to become a closing state.
→ **P3**, **P4**, **P6**. → **SURVIVES; top-tier for ADR 0008 §4.2.** A degraded
or attention signal must never be readable as completion.
`docs/state-machines.md` §1; `crates/governor-core/src/health.rs:3-6`;
`crates/governor-core/src/obligation.rs:14-16`;
`crates/governor-store-sqlite/src/ops/health.rs:1-8`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:1513-1532`, `:1534-1570`.

**Pinned, non-generatively-compacted fact classes.** GIVEN repeated compaction;
THEN these remain exact: lifecycle/obligation state; identity/version/generation/
source fences; capability and delegation policy; user-owned decisions and explicit
requirements; accepted external-effect ambiguity state; current result/input
artifact identities; and the exact safety policy needed to authorise an action.
If a provider's own compaction cannot guarantee preservation, Governor re-injects
the deterministic control capsule rather than trusting the summary.
→ **P3**. → **SURVIVES.** ADR 0007 §10.

**Memory is evaluated by downstream action, not recall.** ADR 0007 §11 names seven
required scenario families, and they map one-to-one onto Gate P3 cases: parent
delegates → child finishes → restart → parent processes **exactly once**; worker
compacts/restarts → prior user constraint still enforceable; observer stale by
several events → the capsule prevents authorising an old action; provider fork →
lineage preserved but **cost does not roll back**; loadout definition changes
after spawn → resume uses the original profile or fails closed; repeated
compaction → control/capability/safety facts exact; memory worker crashes or
exceeds budget → control plane correct and backlog observable rather than
blocking.
→ **P3**, with the fork/cost case also **P6**. → **SURVIVES.**

**Observer work is watermarked and admission-controlled.** GIVEN out-of-order
observer completion; THEN explicit coverage ranges make it acceptable, and a
memory view states the highest authoritative sequence it covers so a consumer can
distinguish *"memory is current through N"* from control truth at N+k.
Construction must not delay a latency-sensitive ACK, input answer, or other
control-path mutation solely to make advisory memory fresh. Consolidation is
triggered by measured marginal cost/size/backlog, not a fixed interval.
→ **P3**, **P6**. → **SURVIVES.** ADR 0007 §9.

**H60/H61/H62 — the durable model has nowhere to put a summary, because there is
no writer that could put one there.** Safe metadata accepts exactly four value
shapes: bounded token, opaque identity, bounded integer, closed-set label. **No
method takes a `&str`, a JSON value, or anything free-form.** The reader is not a
general parser: exactly one accepted shape — a flat object of strings, integers
and booleans — with nesting, arrays, floats, nulls, trailing junk and duplicate
keys all malformed. Its unit tests use literally
`{"tool_input":{"command":"rm -rf /"}}`, `{"messages":[…]}`, `{"cost":0.42}`,
`{"cwd":null}`. Keys outside the event kind's allowlist are discarded on read.
→ **P3**. → **SURVIVES; top-tier.** This is ADR 0008 §4.5 enforced structurally:
**reject the compaction or summary document at the parser, not at the schema.**
`docs/data-model.md` §"Immutable event ledger";
`crates/governor-store-sqlite/src/safe_metadata.rs:1-22`, `:16-22`, `:113-121`,
`:249-280`, `:305-324`, `:399-405`, `:415-434`;
`crates/governor-store-sqlite/src/event.rs:192-243`.

**A fact is never stored twice in two places that can disagree.** Health-condition
scope lives in the event's own scope columns and *not* also in metadata —
*"there is no second copy of either to drift"*; the ledger event's scope is read
from columns rather than duplicated; the loadout digest is derived rather than
accepted.
→ **all gates**. → **SURVIVES**, and it is the design rule most likely to be
violated when composing several Pi packages that each want to own a copy of
session state. `crates/governor-store-sqlite/src/event.rs:238-241`, `:360-365`;
`crates/governor-store-sqlite/src/ops/health.rs:18-26`.

### 3.10 Independent review, user-owned decisions, analytics provenance

**WRK-019 — a worker result is not self-approval.** GIVEN a final result that says
"tests pass, ACK/merge now"; THEN the state stays `completed_unprocessed` and
independent foreman processing and ACK are required.
→ **P5**. → **SURVIVES.** `docs/testing.md` WRK-019; ADR 0008 §4.8.

**SEC-006 — prompt injection cannot become a control argument, and it is
*unrepresentable*, not merely rejected.** GIVEN a worker result that tries hard to
be a control message — forged "ACK", "answer input", fake IDs, policy
instructions; THEN it is stored verbatim as bytes, reaches no control path, and
the obligation is exactly as owed — because every fence is a typed identity or a
charset-restricted token, so the text cannot *be* a control argument. The injected
text reaches no ledger column. Control fields are trusted protocol; worker
results, GitHub issues/diffs/comments and repository files are untrusted data,
labelled as such and structurally separated.
→ **P4**, **P5**. → **SURVIVES.** `docs/testing.md` SEC-006;
`docs/mcp-contract.md` §"Prompt-injection/data boundary";
`docs/threat-model.md` §"Threat: prompt injection";
`crates/governor-testkit/tests/sec_acceptance.rs:552`.
*Port note — the most significant NEW risk the pivot introduces.* Under ADR 0008
§6, Pi reads the foreman's **free-form prose reply** rather than receiving a typed
MCP call. The parser that extracts a `FOREMAN_ACTION` from prose is an injection
surface the MCP topology did not have: a worker result quoted back into the
foreman conversation could contain a well-formed action envelope. **Gate P4 needs
its own conformance test: an action envelope appearing inside quoted untrusted
content must not be accepted as the foreman's disposition.**

**INP-007 — user-owned decisions are not foreman-answerable.** GIVEN an input
request classified user-owned (destructive, credential-sensitive, materially
broader, or unknown); THEN `foreman_may_answer()` is false and the answer returns
`user_authorization_required`, with no grant event and no worker I/O. Only
delegated ordinary engineering work is foreman-answerable. A worker request never
creates its own authorisation; a hook "allow" is never authority to widen a
recorded user or managed restriction.
→ **P4**, **P5**. → **SURVIVES.** ADR 0008 §4.7; `docs/testing.md` INP-007;
`docs/mcp-contract.md` §"Authorization"; `docs/threat-model.md` §"Threat: worker
input widens user authority"; `crates/governor-core/src/input.rs:155-168`, `:668`.

**INP-004..006 / INP-009 — an answer recorded is not an answer received.** GIVEN a
valid recorded answer and a created continuation; WHEN the harness crashes before
worker I/O; THEN the obligation does not return to `running`. **Even transport
`Accepted` on the continuation does not restore `running`** — only confirmed
resumed-turn evidence for the exact command revision, a live incarnation and the
exact answered input does. GIVEN a differing second answer to the same current
request; THEN `conflicting_input_answer` — never two continuations. An answer must
fit the declared shape. Worker continuations run the identical
`pending → claimed → accepted|failed|ambiguous` machine, and matching resumed-turn
evidence promotes an *ambiguous* continuation — it is exactly the exact
reconciliation ambiguity waits for.
→ **P2**, **P4**. → **SURVIVES.** `docs/testing.md` INP-004..006, INP-009;
`docs/data-model.md` §"The structured answer set";
`crates/governor-core/src/input.rs:580`, `:593`, `:611`, `:635`;
`crates/governor-core/src/worker_command.rs:205-256`, `:357`, `:372`, `:388`,
`:396`, `:446`; `crates/governor-core/tests/state_machine_invariants.rs:1207`,
`:1234`.

**INP-011 — question detail unavailable stays durable and unanswered.** GIVEN a
restart after which the safe deferred identity exists but the provider cannot
recover the question or options without transcript scraping; THEN return
`input_detail_unavailable`, keep the obligation open, and **never invent an
answer**.
→ **P2**, **P3**. → **SURVIVES.** `docs/testing.md` INP-011;
`docs/worker-lifecycle.md` §"Forbidden durable persistence".

**GPT-011 / GPT-012 — capability loss preserves work; product confirmation is not
bypassed.** GIVEN a binding that was write-capable whose writes become
unavailable; THEN no browser or assistant event closes work, the loss is recorded
as attention (`mcp_write_capability_missing`), repeated identical observations are
idempotent, a stale generation cannot record it, and obligations remain durable.
GIVEN a write action requiring a user-owned or unsupported confirmation state;
THEN Command Governor does not automate around it, reclassify the tool read-only,
or mark ACK successful.
→ **P4**, **P5**. → **SURVIVES as policy.** The specific failure-class names
(`app_tools_not_mounted`, `write_action_unavailable`, `write_action_rejected`,
`confirmation_required`, `connector_unreachable`, `connector_abi_mismatch`) are
**BORDERLINE** — ADR 0008 §7 may leave `connector_abi_mismatch` without a
referent — but *the distinction between a mount/runtime failure and an actual
capability denial* survives any transport, and the underlying rule generalises to
**a degraded transport must not lower the bar for closing work**.
ADR 0006 §"Tool-mount failures are distinct from write denial";
`crates/governor-core/src/binding.rs:436`, `:453`, `:466`.

**Capability is empirically gated and epoch-fenced.** GIVEN a transport surface;
THEN support is decided by a harmless synthetic mutation and read-back on the
**exact bound surface**, recorded under a `capability_epoch`, revalidated after
connector/account/product/ABI changes or repeated rejection — and a previously
successful probe never authorises silent fallback when writes later stop working.
→ **P4**, **P1** (epoch/drift detection is the same shape as version drift).
→ **SURVIVES as a principle.** ADR 0008 §7 explicitly retains ADR 0006's
empirical capability-testing principle for any shipped MCP adapter while dropping
the universal gate. **Given ADR 0008 §8 makes both `pi-gpt` and `pi-oracle`
undocumented and replaceable, the epoch discipline arguably matters *more* after
the pivot than before:** an undocumented backend is precisely the thing whose
capability drifts silently. ADR 0006; `docs/threat-model.md` §"Threat: connector
plan/capability mismatch".

**Analytics provenance and non-rollback.** GIVEN any collected metric; THEN it
carries provenance indicating whether it is provider-reported, measured locally,
derived, or estimated. GIVEN a provider session forked or a UI branch changed;
THEN cost **never rolls backward** — actual resource consumption is an
append-only accounting fact.
→ **P6**. → **SURVIVES.** ADR 0007 §2, with a direct scenario in §11.

**Authoritative numbers are visibly distinguished from derived ones.** When a
daemon is running, its own half of the report is fetched over the socket and
rendered with a `live.` prefix.
→ **P6**. → **SURVIVES.** Provenance marking in the analytics surface, exactly
ADR 0008 P6. `crates/governor-daemon/src/doctor.rs:110-118`, `:144`;
`crates/command-governor/tests/daemon_acceptance.rs:317-322`, `:396-412`.

**Diagnosis never takes authority, creates nothing, and repairs nothing.** A
shared-and-immediately-released lock probe; a `READ_ONLY` database open;
`symlink_metadata` only. *"A diagnostic that migrated a schema or advanced an
epoch would be a second daemon under another name."* Proven against a bare
directory (no database, no lock, no artifact root created) and against a corrupt
database (bytes unchanged after diagnosis). The one mutation it performs — a
zero-byte probe to learn the effective UID — is **named, not buried**, removed
immediately, and its failure is itself a reported finding.
→ **P1**, **P6**. → **SURVIVES; critical for Pi.** A "check my harness" command
must not initialise it. `crates/governor-daemon/src/doctor.rs:1-38`, `:24-30`,
`:170-240`, `:461-520`; `crates/governor-daemon/src/layout.rs:254-284`;
`crates/command-governor/tests/daemon_acceptance.rs:566-623`.

**Safe diagnostics are a closed set of field *types*, not a convention.** The log
field builder accepts `&'static str` classes, integers, counts, flags, opaque
identities and bounded tokens. **There is deliberately no `&str` accessor** —
adding one would be the change that reopens the hole, and it would be visible in
review. The log is bounded and rotated inside the state root (1 MiB, one
generation): unbounded growth inside the state root is a defect in its own right.
→ **P6**. → **SURVIVES; the single most transferable analytics-provenance idea.**
`crates/governor-daemon/src/logging.rs:1-31`, `:46-55`.

**Startup evidence is reported as counters.** Daemon epoch, incarnation, whether
the lock was reclaimed, schema epoch, migrations applied, projection verification,
quarantine counts, artifacts verified/unverified/quarantined.
→ **P6**. → **SURVIVES.** `crates/governor-daemon/src/startup.rs:106-134`.

**Distinct exit codes are part of the contract, and the parser is total.**
0 healthy / 1 usage / 2 refused / 3 unhealthy / 4 not running — documented in
`--help`, and a test asserts the usage text names every one. An unknown flag is a
usage error, never a silently ignored argument.
→ **P1**, **P6**. → **SURVIVES.** *"Unknown key in config is an error"* is a real
Pi-native loading hazard. `crates/command-governor/src/main.rs:44-52`;
`crates/command-governor/src/cli.rs:13-14`, `:90-96`, `:106-168`, `:209-236`,
`:246-260`.

**The mutating surface is an explicit enumeration a reviewer can see at once.**
*"Deliberately an explicit enum rather than a boxed closure: this list is the
store's mutating surface."*
→ **P5**. → **SURVIVES.** For a Pi harness this becomes the reviewable inventory
of every composed package's mutating capability — directly answering ADR 0008's
stated cost, *"overlapping packages can create conflicting authorities if
installed carelessly."* `crates/governor-store-sqlite/src/writer.rs:90-93`.

**One authority for durable writes, with bounded intake.** *"An unbounded queue
would let a runaway producer queue work the single writer can never drain, and the
backpressure is more useful than the queue."*
→ **P2**. → **BORDERLINE.** "One SQLite connection on one OS thread" is
RUST-ONLY; *there is exactly one authority for durable writes and its intake is
bounded* survives, and is doubly important under ADR 0008.
`crates/governor-store-sqlite/src/writer.rs:1-15`, `:335-337`.

### 3.11 Worker-lifecycle evidence rules

Claude-adapter-shaped, and ADR 0008 §2 removes `governor-worker-claude`. But the
*epistemics* generalise, and ADR 0008 names `agent_settled` *"a better generic
harness seam"* — a claim that needs testing, not assuming.

**Evidence class and fence decide, not the newest timestamp.** Ordered precedence:
structured run outcome (1) > strong native signal (2) > stop candidate (3) >
permission decision (4) > runtime observation (5) > PTY heuristic (6). *"Newest
timestamp wins" is explicitly rejected.*
→ **P2**. → **SURVIVES as a principle.**
`docs/worker-lifecycle.md` §"Authority classification";
`docs/state-machines.md` §2; `crates/governor-core/src/worker_evidence.rs:26-63`,
`:427`.

**WRK-003 / WRK-004 — a vetoable signal is never terminal.** GIVEN our stop hook
fires, another matching hook returns `decision: block`, work continues, our hook
fires again, and only then the final structured result and matching child exit
arrive; THEN the classification is stop-candidate-only with `candidates: 2`, and
completion occurs exactly once, only after the result and exit are proven and the
artifact is durable. `ConfirmedFinalResult` has **no public constructor**, so a
completion built from a stop callback is unreachable.
→ **P2**. → **SURVIVES as the generalised rule** — *a lifecycle signal another
subscriber can veto is candidate evidence, not a terminal fact* —
**BORDERLINE as written**, since it names Claude's `Stop` hook.
*The port question is precisely whether Pi's `agent_settled` is genuinely
non-vetoable* (*"emitted only after a full session-level run is settled and no
automatic retry, compaction retry, or queued continuation remains"*). If it is,
this test becomes a **regression guard on that Pi guarantee**; if not, the
candidate/terminal split survives intact. Port it either way and point it at
`agent_settled`. `docs/testing.md` WRK-003, WRK-004; ADR 0005;
`crates/governor-core/src/worker_evidence.rs:457`, `:463`;
`crates/governor-core/tests/state_machine_invariants.rs:469`, `:480`.

**WRK-006 / WRK-007 — completion needs both halves, correlated.** A truncated
final result is never completion; a successful exit without a final result is not;
a final result without a trustworthy exit is not; an exit receipt for **another
run** does not count; `SessionEnd` alone is session end, not success; a stop
failure needs a corroborating exit.
→ **P2**. → **SURVIVES as a property** (terminal success requires two independent
confirmations of the same fenced run); **BORDERLINE** in that "child exit receipt"
presumes a subprocess topology.
`crates/governor-core/src/worker_evidence.rs:492`, `:511`, `:525`, `:540`, `:559`,
`:565`. And **no evidence is indeterminate, not failure** (`:597`).

**WRK-001/002/010/011 / OBL-006 — stronger fenced truth wins, and the
disagreement is *recorded*.** GIVEN a confirmed completion or confirmed deferred
input while the runtime still reports `working / idle:false`; THEN project the
stronger confirmed state **and** open a `runtime_state_conflict` — the
disagreement is surfaced, not silently resolved by timestamp — then reconcile
transport with **one** fenced clear/interrupt, verify safety, perform **one**
continuation, and if still inconsistent preserve the obligation and expose
reconciliation failure rather than creating a duplicate worker. Contradictory
terminal evidence opens attention and **never a second obligation**; repeats
converge to one condition; non-contradictory evidence cannot open one.
→ **P2**, **P6**. → **SURVIVES as the precedence principle;
BORDERLINE/RUST-ONLY for the Herdr specifics**, which ADR 0008 §2 removes.
`docs/testing.md` WRK-001, WRK-002, OBL-006;
`docs/worker-lifecycle.md` §"Stale Herdr `working` conflict";
`crates/governor-core/src/worker_evidence.rs:583`;
`crates/governor-store-sqlite/src/store.rs:621-641`;
`crates/governor-testkit/tests/obl_acceptance.rs:496`.

**WRK-012..015 — the watchdog creates attention, never terminal state.** GIVEN no
verified progress past a threshold with no confirmed terminal or input boundary;
THEN exactly one stall attention; the worker remains running; no synthetic
failure, completion, interrupt or monitor session is emitted; later verified
progress resolves it; a confirmed boundary resolves rather than concludes; and
**a backwards clock cannot manufacture a stall** because elapsed time is clamped
at zero. High-rate equivalent progress is deterministically coalesced without
growing unbounded rows or moving the watchdog clock incorrectly. The outcome type
has **no variant** for completion, failure, interruption or spawning.
→ **P2**, **P6**. → **SURVIVES.** `docs/testing.md` WRK-012..015;
`docs/state-machines.md` §9 and required invariant 16;
`crates/governor-core/src/watchdog.rs:132`, `:170`, `:183`, `:196`, `:228`;
`crates/governor-core/src/time.rs:111`;
`crates/governor-core/tests/state_machine_invariants.rs:872`.

**WRK-021 — capability feature detection beats version guessing.** GIVEN
structured init advertising or omitting required capabilities across versions;
THEN the adapter follows the capability proof and fails closed when required
features are missing.
→ **P1**, **P2**. → **SURVIVES**, and it maps unusually well onto Gate P1: this is
the same discipline as *"pin a release and detect version drift"*, applied to a
provider instead of a substrate. `docs/testing.md` WRK-021.

**INP-002/003 / WRK-023 / INP-008 — never mint a resumable pause identity you
cannot later address exactly.** GIVEN a single-tool defer with an exact tool-use
fence, a documented defer response **and** a structured `tool_deferred` outcome;
THEN one durable input request with no raw tool arguments persisted. GIVEN a
multi-tool shape; THEN `worker_defer_shape_unsupported` / manual reconciliation —
attention only, never `needs_input` and never an invented pending tool identity.
GIVEN a single-tool shape with the exact fence but **no structured proof**; THEN
unconfirmed, a reconciliation condition. GIVEN a permission-request event carrying
tool name and input but no exact tool-use ID; THEN it cannot supply the native
input reference a clean defer requires, and it ranks below both strong native
signals and stop candidates. `ConfirmedDefer` has no public constructor.
→ **P2**. → **SURVIVES as the generalised rule; BORDERLINE in the specifics**,
which are pinned Claude semantics as of 2026-08-31 — and architecture review
**R2** records that an earlier version of this contract was already stale once.
`docs/testing.md` INP-002, INP-003, WRK-023, INP-008;
`docs/worker-lifecycle.md` §"AskUserQuestion / non-interactive defer";
`crates/governor-core/src/input.rs:536`, `:556`;
`crates/governor-core/tests/state_machine_invariants.rs:508`, `:562`.

**WRK-020 — never assume hook or settings isolation.** See §3.5 (A1): this is the
same invariant, and its Pi-native reading is Gate P1's resource-precedence
characterisation.

### 3.12 Privacy and forbidden persistence

ADR 0007 §1 keeps the Phase-1 prohibition while explicitly widening what analytics
may exist. Both halves matter.

**H63 / SEC-001 — forbidden content is unrepresentable before any I/O, and the
honest converse is tested too.** GIVEN prose, paths, shell commands, JSON
documents, transcripts and auth headers; THEN each fails the safe-token charset
(ASCII alphanumerics plus `- _ . : @ + =`; whitespace, control characters, quotes
and `/` refused; max 128 bytes; empty refused; **the rejected value is never
echoed in the error**). But a **token-shaped** secret — a session cookie, an
`sk-proj` key, a `ghp_` credential — *is* representable, and the tests say so
explicitly rather than claiming a guarantee the crate does not have.
→ **P3**, **P5**. → **SURVIVES — and the intellectual honesty is the transferable
part.** `crates/governor-core/src/fence.rs:19-63`, `:308`, `:330`, `:342`;
`crates/governor-core/tests/state_machine_invariants.rs:1534`;
`crates/governor-core/tests/durable_execution_invariants.rs:1020`;
`crates/governor-store-sqlite/tests/store_privacy.rs:64-86`, `:78-85`.

**SEC-001 — the sweep, and its honest split.** GIVEN a 14-sentinel corpus (cwd,
prompt, raw tool arguments, raw tool result, shell command, transcript path,
terminal transcript, provider stream record, auth header, response body, cookie,
API token, GitHub credential, environment secret) plus one designated final-result
sentinel that *is* allowed to be durable; WHEN a full composed lifecycle including
a restart runs; THEN a byte scan of **every file** under the state root — database
**plus WAL plus SHM**, objects, incoming, quarantine, logs — plus CLI stdout and
stderr for five commands finds zero matches outside the designated artifact. The
split is recorded per sentinel:
- **10 are structurally unrepresentable** (they carry a space, quote, newline,
  brace or `/`), so the scan for them proves only that nothing *else* wrote them;
- **4 are token-shaped and are actually injected** through the exact public field
  where each would be confused for something legitimate — a GitHub credential
  where an issue ref belongs, an API key where a worker turn ref belongs, an
  environment secret in a display name, a session cookie where a provider message
  identity belongs — and the assertion is **confinement, not absence**: each must
  reach exactly its own `(table, column)` and nothing else. **Reaching *nothing*
  also fails**, because that would mean the lifecycle never injected it and the
  sweep proved nothing at all.
A unit test asserts the corpus's own `token_shaped` labels agree with the actual
charset, so the fixture cannot drift from reality. A positive control ensures a
clean sweep is not a broken scanner. WAL inclusion is deliberate: *"a value that
was written and later overwritten still leaves its bytes in the WAL, so scanning
only the main file would be too weak."* The column search uses `instr` rather than
`LIKE` so a needle containing `_` or `%` does not silently widen. Every text
column of every table is enumerated **from the engine's own introspection**, so a
column added later is searched automatically — and the set of places a value could
live is **pinned as a lock, not a description**, so adding one forces a deliberate
review.
→ **P1**, **P2**, **P3**, **P4**, **P6**. → **SURVIVES, and the *methodology* is
the transferable asset.** A port that keeps only the byte scan and drops the
representability analysis will believe it has proven more than it has.
`docs/testing.md` SEC-001 (which already documents this split in prose);
`crates/governor-testkit/src/sentinels.rs:136`, `:389`;
`crates/governor-testkit/tests/sec_acceptance.rs:62`, `:166`;
`crates/governor-store-sqlite/tests/store_privacy.rs:88-125`, `:166-227`,
`:229-237`, `:521-557`, `:553-557`.

**A value the run itself generates cannot be in a static corpus.** The wake
correlation ID is swept separately: it **must** be persisted in the store and must
appear on **no output surface** — stdout, stderr, log lines, rendered errors —
including on the refusal path where a projection mismatch is reported. The helper's
own documentation warns to pass only output surfaces, *"because scanning the
database file would fail for the one reason that is correct."*
→ **P4**, **P6**. → **SURVIVES; the methodology (sweep the run's own generated
secret) is the reusable part.** `docs/testing.md` SEC-001;
`crates/command-governor/tests/daemon_acceptance.rs:794-944`;
`crates/governor-testkit/src/sentinels.rs`.

**H67 — operator-facing text names the non-secret deterministic key, never the
possession fence.** GIVEN both halves of a delivery projection tampered; THEN each
mismatch row starts with the `delivery_key` hex, contains no correlation-ID hex,
and **the rendered message is asserted on too**, because it reaches stderr and the
log. The correlation ID type has no `Display` implementation for exactly this
reason; its `Debug` renders `DeliveryId(<redacted>)`; and equality is
constant-time-ish (accumulating XOR, no early return).
→ **P4**, **P6**. → **SURVIVES.** *What you publish for diagnosis must not be what
grants authority* — a P6 provenance rule as much as a P4 security one.
`crates/governor-store-sqlite/src/replay.rs:804-812`;
`crates/governor-store-sqlite/tests/store_lifecycle.rs:934-1002`;
`crates/command-governor/tests/daemon_acceptance.rs:946-1003`;
`crates/governor-core/src/delivery.rs:637`, `:658`.

**SEC-009 — credentials reach no column, and no column is even *named* for one.**
GIVEN six credential classes; THEN none reaches any column, **no column is named**
`cookie`/`credential`/`secret`/`password`/`bearer`, and the whole state root is
clean including artifacts.
→ **P4**. → **SURVIVES; directly relevant to `pi-gpt`/`pi-oracle`,** both of which
handle live session cookies and bearer tokens. Cheap to port: a byte scan over a
state directory plus a column-name check, neither depending on Rust or SQLite.
`docs/testing.md` SEC-009; `crates/governor-testkit/tests/sec_acceptance.rs:642`.

**SEC-005 — the wake carries no result data, structurally.** The payload is a
protocol marker plus two opaque identities plus a static instruction, **and the
renderer takes nothing else**. Asserted against eight named leaks and the whole
forbidden corpus. The stored payload digest is computed *by the store* from the
already-created correlation ID plus the scheduling tuple and protocol label, so no
caller can put worker output in that column even by accident.
→ **P3**, **P4**. → **SURVIVES.** Bounded reference payload, not content.
`docs/testing.md` SEC-005; `docs/data-model.md` §"Browser wake deliveries";
`crates/governor-testkit/tests/sec_acceptance.rs:487`.

**Allowlist serialisation, not generic dumps.** Each event kind has an explicit
serialiser with bounded allowed fields; unknown fields are discarded; progress
persists only identity, time and safe class; input records store opaque identity,
classification and answer *shape*, never raw arguments or question text; every
event carries a schema version stamp; event kinds are a closed set and a kind with
no replay rule cannot be written.
→ **P2**, **P4**, **P6**. → **SURVIVES.** The specific 28 kinds are topology; the
property — *the protocol's event vocabulary is closed and every member has a
defined fold rule* — survives and should be encoded, because ADR 0008's
`FOREMAN_EVENT`/`FOREMAN_ACTION` envelope is exactly such a vocabulary.
`docs/data-model.md` §"Immutable event ledger";
`crates/governor-store-sqlite/src/event.rs:11-13`, `:29`, `:36-107`, `:150-185`,
`:192-243`, `:307`.

**There is deliberately no free-text answer variant.** GIVEN a foreman answer;
THEN it is exactly one of `Choice { index }`, `Boolean { value }`,
`OpaqueToken { token }` or `Declined` — a count and a kind, never option text.
*"A column that accepts a sentence is where prompts, tool arguments and eventually
credentials end up."*
→ **P3**, **P4**. → **SURVIVES as a fail-closed policy with a named open
question.** `docs/data-model.md` §"The structured answer set" already records that
if a foreman genuinely must type a sentence back to a worker, the model has no
sanctioned place for it, and resolving that means either establishing the protocol
never needs prose or defining a bounded, explicitly classified prose field with
its own retention and redaction rules.
*This becomes urgent under ADR 0008 §6*, because a ChatGPT Web foreman replies in
prose by construction, and the Pi-native `FOREMAN_ACTION` envelope has an
`instructions / delegation / question` field — **exactly the prose field Phase 1
refused to add.** See §7.
`crates/governor-core/src/input.rs:15-17`, `:170-201`.

**Identities are opaque and never parsed; nothing is ambient.** Every identity is a
distinct type; no field of the inner UUID is read; no meaning is derived from
version or timestamp bits; comparison is equality and ordering only; malformed
persisted text fails closed **without echoing it**. No function reads a clock or
an entropy source: every time-dependent transition (claim expiry, stall threshold,
resume backoff) takes the instant as an argument — which is what makes the
machines replayable and the tests deterministic.
→ **P1**, **P2**, **P3**. → **SURVIVES.**
`crates/governor-core/src/id.rs:1-12`, `:239`, `:266`;
`crates/governor-core/src/lib.rs:20-23`;
`crates/governor-core/src/time.rs:1-6`; `crates/governor-core/src/random.rs:1-6`.

**ART-004 / SEC-008 — path and key safety.** The daemon allocates storage keys and
**workers never supply paths** — there is no path type anywhere in the public
surface. A key is a validated single component: `.`/`..`, hidden names and `:` are
refused, and length is capped at 96 bytes so key plus staging suffix stays inside
`NAME_MAX`. A tampered persisted reference never becomes a path (two layers:
separators and absolutes are not representable as a token at all; dot-shaped
values are refused by key validation), and files outside the root are provably
untouched. A symlink planted at a key fails closed with `ELOOP` rather than
redirecting a read; a symlink at a staging name refuses the write via `O_EXCL` and
the link target is unmodified. **Hard links are handled on both sides:** a name
planted at a key is a name whatever inode backs it → `AlreadyPublished`; a read
where `nlink != 1` → refusal, because a second name means someone can rewrite
bytes in place. Layout components are refused if symlinked or non-directory,
including the root. The orphan sweep moves the name and never follows the link.
→ **P1**, **P2**. → **SURVIVES** wherever the Pi-native harness owns a filesystem
store, which ADR 0008 §5 explicitly permits. `docs/testing.md` ART-004, SEC-008;
`crates/governor-artifacts/src/key.rs:1-23`, `:45-96`, `:162-216`;
`crates/governor-artifacts/src/fs_secure.rs:56-99`, `:131-147`, `:142-146`,
`:175-187`; `crates/governor-artifacts/src/root.rs:52-67`, `:68-86`;
`crates/governor-artifacts/tests/artifact_paths.rs:44-88`, `:91-117`, `:120-151`,
`:154-221`, `:245-304`, `:307-341`, `:389-406`, `:409-416`;
`crates/governor-testkit/tests/sec_acceptance.rs:631`.
*Note the deliberate asymmetry:* an over-permissive **existing** root is **repaired**
to 0700 rather than refused, because refusing would strand the artifacts inside it
and adoption after restart is the normal case — while unsafe **shapes** are
refused outright. Repair modes; refuse shapes.

**ART-005 — owner-only modes hold regardless of host umask, and the trust model is
asserted including what it does *not* claim.** Directories 0700 and files 0600 are
forced by explicit `chmod`/`fchmod` **after** creation, because `mode(0o600)`
under `umask 0777` yields mode **000** — the wrong mode entirely, not merely a
laxer one — proven by re-executing the test under `sh -c 'umask 0777'`. The
durable authority is hardened the same way: SQLite creates files under the host
umask (0644 on a default account), so the daemon `chmod`s the database and its
`-wal`/`-shm` sidecars and audits them, and the doctor reports a world-readable
database as a finding. Modes survive publication, restart repair (**repair must
not widen**) and an orphan sweep into quarantine. And a test **demonstrates that
the same OS user can read, rewrite and unlink the file**, establishing that what
the store buys against that actor is *detection, not prevention*.
→ **P1**. → **SURVIVES.** The umask finding is exactly the kind of thing a
reimplementation silently gets wrong, and *"a dependency's file creation obeys the
umask, so harden after it runs"* applies to every Pi package that writes state.
`docs/testing.md` ART-005; `crates/governor-artifacts/src/fs_secure.rs:1-33`;
`crates/governor-artifacts/src/lib.rs:69-84`;
`crates/governor-artifacts/tests/artifact_permissions.rs:79-188`, `:191-239`;
`crates/governor-daemon/src/startup.rs:206-215`, `:513-527`;
`crates/command-governor/tests/daemon_acceptance.rs:738-790`.

**SEC-007 — security claims are asserted and cannot silently inflate.** The trust
model is emitted as data, **unconditionally**: `trust.model=os_user_account`,
`owner_only_file_modes=true`, `protects_from_other_os_users=true`,
`same_user_containment=false`, `ipc_peer_credential_check=false`,
`ipc_boundary=owner_only_directory_mode` — not conditional on anything, because
the guarantee is not either. Plus: **no output may contain the word "sandbox"**,
and a test asserts the security document still describes a same-user trust model
and does not claim a hostile same-user sandbox.
→ **P1**, **P6**. → **SURVIVES** as an executable honesty check; **BORDERLINE**
only in that the file-content check is repo-specific. A future change that starts
depending on same-user containment fails a test instead of shipping.
`docs/testing.md` SEC-007; architecture review **R5**;
`crates/governor-daemon/src/doctor.rs:145-155`;
`crates/command-governor/tests/daemon_acceptance.rs:656-688`;
`crates/governor-testkit/tests/sec_acceptance.rs:610`.

### 3.13 RUST-ONLY (topology; recorded, not elaborated)

Listed so the migration PR can delete them without a second review.

- `rusqlite` / bundled SQLite, `journal_mode=WAL`, `synchronous=FULL`, busy
  timeout, foreign-key pragma, `STRICT` tables, `CHECK` placement, index and
  partial-index syntax, `OpenFlags`, `TransactionBehavior::Immediate`,
  `OptionalExtension`, `PRAGMA table_info` introspection, `WITHOUT ROWID` sort
  semantics, `SQLITE_ABORT` injection mechanics, WAL/`-shm` file layout,
  SQLite's transactional-DDL guarantee. (ADR 0002; ADR 0008 §"Supersession map".)
- The single-writer actor thread, its typed command channel, `sync_channel(64)`,
  `std::thread` vs Tokio, and the ADR 0002 Tokio-boundary rules.
- Migration DDL and the `schema_migrations` table shape — **except** the epoch
  and checksum concepts, which survive (§3.5).
- The `cg1` line protocol, Unix-domain-socket / named-pipe selection, and the
  CLI-as-IPC-client topology — **except** path-length preflight and
  liveness-is-a-round-trip, which survive (§3.6).
- The four-tool `rmcp` ABI, its response envelope, paging cursor scoping, and
  `BootstrapView`'s field list (ADR 0008 §7). **Except** the ABI-stability
  *policy* — *breaking argument or tool semantics require a new connector ABI and
  an explicit refresh, never an invisible mutation under an old conversation* —
  which is a real invariant if any MCP path ships, and the bootstrap
  *aggregate-only* property (§3.2).
- `chromiumoxide`/CDP driver selection, DOM selector hierarchy, and the
  headed-vs-headless comparison matrix (ADR 0003; ADR 0008 §8).
- `hex_encode`; the `Transition::Advanced`/`Duplicate` enum shape and
  `or_unchanged` combinator (the *concept* — duplicate is neither error nor
  advance — is §3.3); `Box<VerifiedBindingTarget>` (a clippy accommodation);
  `#[non_exhaustive]`, `#[must_use]`, `thiserror` derives; the
  `cast_possible_truncation` expectation; field-by-field accessor surfaces;
  `debug_names_the_family_and_display_is_canonical`.
- `Delivery::pending` panicking on a zero attempt budget — a construction assert.
  The *rule* (a zero budget is a configuration error, not a runtime state)
  survives.
- Serde-free hand-rolled JSON writer/parser internals — the *accepted shape*
  survives (§3.9), the implementation does not.
- Rust CI tool names (`cargo fmt/clippy/test/audit/deny`, `rust-toolchain.toml`,
  workspace lint policy). **The discipline survives directly** into Gate P1:
  pinned toolchain, committed lockfile, license/source policy, no blind auto-merge
  for security-sensitive dependencies — and SEC-010 proves the policy is *in
  force* (§3.5).
- `thiserror`/`anyhow` split. The underlying rule — a domain conflict must remain
  **machine-classifiable** — survives; see §4.5.

### 3.14 BORDERLINE, consolidated — the largest fidelity loss

**Type-level capability tokens.** `IoPermit`, `SendActivation`,
`ExternalExecutionPermit`, `DurableIntentAccepted`, `GrantedPermit`,
`ConfirmedFinalResult`, `ConfirmedDefer`, `ConfirmedResumedTurn`,
`PublishedArtifact`/`DurableArtifact`, `LeaseHolderProof`, `ResumePermit`,
`ManagedConfigVerified`, `CommittedLoadout`. Each is non-`Clone`, has no public
constructor, is reachable from exactly one place, and is consumed **by value**.
Together they make several central rules *unrepresentable* rather than merely
discouraged: a send without a durable arm, a spawn without a durable intent and a
re-proved snapshot, a completion built from a stop callback, a disposition built
from weak evidence.

**None of this survives into TypeScript as a compile-time guarantee.** Every rule
each token encodes is fully testable at runtime, so the recommendation is: **port
every rule as a runtime conformance assertion, mechanised through boundary fakes
that read the committed store and panic rather than act (§5), and state the
weakening explicitly in the migration PR.** The Pi-native version is *checked*,
not *proven*. That is the single largest fidelity loss in the migration and it
should be recorded as a decision, not discovered later.

Other consolidated borderlines, each argued at its entry above: the specific
acceptance-evidence shape (§3.1 DEL-014) and the architectural consequence that a
transport without exact message identity forces every send to ambiguous; the
connector-ABI fields on the binding (§3.10); *"exactly one active binding"* as an
assumption a multi-conversation Pi harness may not inherit — the generation fence
generalises to per-binding, but the singleton should be re-decided rather than
assumed; the safe-token charset and 128-byte limit, which are tuned to
Claude/ChatGPT opaque identifiers and should be re-derived from the Pi identifiers
actually encountered; and the two-stage `claimed`/`activation_armed` fence, which
the core crate itself flags as transport-specific — for a Pi-native transport,
decide per transport whether a separate arming fence is observable, because if it
is not, everything post-claim collapses to `ambiguous` on restart.

---

## 4. Algorithms and exact artifacts to preserve before anything is archived

These cannot be re-derived from prose without risk.

### 4.1 `delivery_key` derivation — port verbatim

`crates/governor-core/src/delivery.rs:35-73`.

```rust
pub const WAKE_KEY_DOMAIN: &str = "command-governor/wake-key/v1";
pub const DELIVERY_ID_BYTES: usize = 32;

pub fn derive(
    obligation: ObligationId,
    generation: BindingGeneration,
    revision: DeliveryRevision,
) -> Self {
    let mut hasher = Sha256::new();
    let domain = WAKE_KEY_DOMAIN.as_bytes();
    let domain_len = u64::try_from(domain.len()).expect("domain label length fits in u64");
    hasher.update(domain_len.to_be_bytes());   // length-prefix the domain label
    hasher.update(domain);
    hasher.update(16u64.to_be_bytes());        // obligation id: 16 bytes
    hasher.update(obligation.as_uuid().as_bytes());
    hasher.update(8u64.to_be_bytes());         // generation: u64 BE
    hasher.update(generation.get().to_be_bytes());
    hasher.update(4u64.to_be_bytes());         // revision: u32 BE
    hasher.update(revision.get().to_be_bytes());
    Self(hasher.finalize().into())
}
```

SHA-256, domain-separated, **every** component length-prefixed, persisted as
lowercase hex (64 chars), explicitly **non-secret and authorising nothing**. The
`WAKE_KEY_DOMAIN` string is a protocol break if changed.

The random half shares **no input** with it: `DeliveryId::generate` taking a
secure-random port is the only mint, there is no constructor accepting scheduling
metadata, and exposure requires an explicitly named accessor. Identical RNG
streams with wildly different scheduling metadata produce identical IDs (metadata
is not an input); different streams with identical metadata produce different IDs.
Width ≥ 192 bits is a compile-time assertion.

**Revision numbering rule** (`delivery.rs:396-419`): a next revision increments
within the same binding generation but **restarts at first when the generation
changes** — the generation component of the key keeps the two families distinct.

Stated identically in ADR 0003 §"Delivery identity", ADR 0004 §"Exact binding and
wake correlation", `docs/state-machines.md` §4, `docs/data-model.md` §"Browser
wake deliveries", `docs/mcp-contract.md`, `docs/browser-transport.md`,
`docs/architecture.md`, `docs/threat-model.md`; store-side derivation at
`crates/governor-store-sqlite/src/ops/delivery.rs:130-136`, used at `:936`.

### 4.2 Length-prefixed injective absorption — the reason 4.1 is safe

`@lineage-branch crates/governor-core/src/digest.rs`; the same construction is
inlined in `delivery.rs` and `mutation.rs` on this branch
(`crates/governor-store-sqlite/src/ops/delivery.rs:876-905` for the store's copy).

Every digest pre-image in the workspace — the wake key, the mutation fingerprint,
the resource identity, the worker loadout — is SHA-256 over a domain label
followed by **length-prefixed** fields:

- `absorb(hasher, bytes)` — big-endian `u64` length, then the bytes;
- `absorb_uuid` — the 16 raw bytes, whole; **no field of the UUID is read or
  given meaning**;
- `absorb_u64` / `absorb_u32` — big-endian, **and the width is part of the
  pre-image**: widening a counter later is a protocol break, not a silent
  re-encoding.

The stated reason: *"concatenated alone, `"ab" + "c"` and `"a" + "bc"` are one
pre-image, so two distinct tuples would share a digest that persisted state treats
as an identity."* Its own tests assert
`digest_of(["ab","c"]) != digest_of(["a","bc"])`,
`digest_of(["","a"]) != digest_of(["a"])`, and `u64(1) != u32(1)`.

**This is the single most portable and most silently-losable algorithm in the
repository.** A TypeScript reimplementation doing
`sha256(domain + obligationId + generation + revision)` produces a
different-but-plausible key and collides on adjacent-field reflows.

### 4.3 Frozen digest vectors — copy these into the Pi-native suite

`@lineage-branch crates/governor-core/tests/persisted_digest_vectors.rs`, which
also classifies them:

- **Never change** — wake key, mutation fingerprint, resource identity. *"A row
  keyed on one of them, or a unique index over one, keeps meaning only while the
  pre-image that produced it stays byte-for-byte the same... **A failure in this
  class is never fixed by re-recording the vector.**"*
- **Frozen for mutation coverage** — the worker-loadout digest, pinned because
  `ResumePolicy` has one variant and no per-field difference test can otherwise
  prove its code is absorbed.

| Pre-image | Frozen digest |
| --- | --- |
| `MutationFingerprint::derive(kind="ack_obligation", ["ab","c"])` | `dbc1aa23f46cc9a7d98b012fdafc99e07f471c01ab9c15b322dd9601b4c38c87` |
| `ResourceIdentity::canonical(ns="profile-dir", "/tmp/a b")` | `6a865fa73000ba9cac23420ca84cf22d049497d55c5054cad59cdd3df728abd5` |
| `DeliveryKey::derive(obligation=uuid(0x1234), generation=9, revision=3)` | `6c4e0a797fec80a064cb8f613645485671f0e516b12632a0a953b476a4ce7333` |
| `WorkerLoadout::resolve(...)` (spec in the test) | `8b4ab6da89ae9546870f2d158e323baacb2b4e2600747a88909299f0e020e73c` |

**Preserve the inputs as well as the outputs.** The `["ab","c"]` choice is
deliberate: it *"shares a naive concatenation with `["a","bc"]`, so a lost length
prefix moves this vector."*

### 4.4 The other derivations

**`MutationFingerprint::derive`** (`crates/governor-core/src/mutation.rs:114-136`,
`:873`) — SHA-256 over a domain label, the command kind, a `u64` parameter count
and each parameter, every part length-prefixed. **It is a digest, never the
parameters:** the journal records *that* the operation was the same, never what it
contained.

**`ResourceIdentity::canonical`** (`crates/governor-core/src/lease.rs:71-92`,
`:766`) — domain-separated, length-prefixed SHA-256 over
`(domain, namespace, canonical_name)`. The **caller** canonicalises (symlinks,
case, relative segments) so two spellings produce one identity; the name is hashed
and dropped, and the path is not recoverable from the record.

**`WorkerLoadout::resolve`** — derives the digest from the rows that are written,
never from a caller-supplied value.

### 4.5 The two state machines, in full

**Delivery / at-most-once** (`crates/governor-core/src/outbound.rs:1-18`,
`:26-74`, `:439-638`). Aggregate: `Pending`, `Claimed`, `Accepted`, `Failed`,
`Ambiguous`; `is_frozen() = Accepted | Ambiguous`;
`is_terminal() = Accepted | Failed | Ambiguous`. Attempt: `Claimed`,
`ActivationArmed`, `Accepted`, `Failed`, `Ambiguous`;
`is_live() = Claimed | ActivationArmed`.

| From | Event | To | Guard |
| --- | --- | --- | --- |
| non-frozen, last attempt `Failed` or none | `AttemptClaimed` | new attempt `Claimed` | frozen → `delivery_revision_frozen`; last attempt armed → `retry_after_ambiguity_fence`; last attempt live → `illegal_delivery_transition`; `used >= budget` → `retry_budget_exhausted` |
| `Claimed` | `ActivationArmed` | `ActivationArmed` (sticky `armed=true`) | re-arm → `Duplicate` |
| `ActivationArmed` | `AttemptAccepted{evidence}` | `Accepted` (frozen) | from `Claimed` → `illegal_delivery_transition`; same evidence again → `Duplicate` |
| `Claimed` | `AttemptFailed{class}` | `Failed` | any pre-submit class |
| `ActivationArmed` | `AttemptFailed{class}` | `Failed` | **only** `ActivationRefused` or `TransportRejectedBeforeSend`; else `failure_not_proven` |
| `Claimed`/`ActivationArmed` | `AttemptAmbiguous{reason}` | `Ambiguous` (frozen) | repeat → `Duplicate` |
| any live attempts | `OrphanQuarantined` | all live → `Ambiguous`, reason `OrphanedByRestart` | none live → `Duplicate`; accepted/failed revisions untouched |
| `Ambiguous` | `ReconciledAccepted{evidence}` | `Accepted` | **the only escape**; produces no external effect (no live attempt remains); same evidence on `Accepted` → `Duplicate`; differing → `delivery_revision_frozen`; other states → `illegal_delivery_transition` |

`FailureClass`: `TargetNotFound`, `StaleTarget`, `WrongConversation`,
`AppNotSelected`, `ComposerNotReady`, `NavigationBlocked`, `ActivationRefused`,
`TransportRejectedBeforeSend` — only the last two satisfy
`proves_no_submit_after_arming()`. `AmbiguityReason`: `OrphanedByRestart`,
`ObservationLost`, `EvidenceInconclusive`, `ActivationTimedOut`.

Capability gating: `io_permit()` yields `Some` only while an attempt is live;
`send_activation()` only when the last attempt is exactly `ActivationArmed`.
Neither has a public constructor.

**Generic external effect** (`crates/governor-core/src/effect.rs:11-17`,
`:47-56`):

```text
intent_recorded --(call_dispatched)--> intent_recorded[dispatched]
       |                                      |
       |                                      +--> completed          (terminal)
       +--> failed_before_effect (terminal, proof required)
       +--> ambiguous           (terminal, never an automatic retry)
```

The crate carries the correspondence table explicitly:
`Claimed ↔ IntentRecorded`, `IoPermit ↔ ExternalExecutionPermit`,
`ActivationArmed ↔ dispatched` flag, `SendActivation ↔ permit consumed`,
`Accepted ↔ Completed`, `Failed+FailureClass ↔ FailedBeforeEffect+NoEffectClass`,
`Ambiguous ↔ Ambiguous`, retry budget ↔ `admit_retry` on the class. It also states
that the browser machine's two-stage fence is transport-specific and that the
generic machine *"has no business carrying it"* (`effect.rs:58-61`).

### 4.6 Decision procedures worth porting as functions

1. **`Delivery::retry_conflict`** (`outbound.rs:392-422`) — four-way precedence:
   frozen → armed → last-attempt-live → budget. **The ordering matters**: the
   frozen check must come first, and the armed check must precede the budget
   check, or a fence-crossed attempt could retry into budget it still has.
2. **`FailureClass::proves_no_submit_after_arming` /
   `NoEffectClass::proves_no_effect_after_dispatch` /
   `observable_before_dispatch`** (`outbound.rs:111-116`, `effect.rs:277-293`) —
   the window-fitting proof predicates.
3. **`ExternalAttempt::admit_retry`** — exact-contract reproduction: destination
   **and** contract **and** key **and** class.
4. **`Disposition::closes(AttentionState)`** (`obligation.rs:128-146`) — the 3×4
   compatibility matrix.
5. **`BindingLedger::fence`** (`binding.rs:237-250`) — three-way
   `Equal`/`Less`/`Greater` → `Ok` / stale / unknown.
6. **`CommittedAck::matches`** (`obligation.rs:322-340`) — five-field exact-repeat
   ACK idempotency.
7. **`WorkerEvidenceClass::precedence` + `ManagedRunEvidence::classify`**
   (`worker_evidence.rs:26-63`) — six-level arbitration and the two-receipt
   correlated-completion rule.
8. **`evaluate_defer_boundary`** (`input.rs`) — three-way shape/response/proof
   decision producing durable / unsupported / unconfirmed.
9. **`watchdog::evaluate`** (`watchdog.rs`) — stall/resolve with backwards-clock
   clamping.
10. **`gc::decide`** (`crates/governor-artifacts/src/gc.rs:129-142`) — the entire
    retention policy as one pure function: four states, **no grace parameter**,
    fail-closed on a missing release instant. Fourteen lines, testable without a
    filesystem. Port directly.
11. **Orphan classification, grace, non-clobbering quarantine naming, and
    future-mtime→age-zero→keep** (`gc.rs:242-328`, `:358-377`).
12. **Lock staleness detection** (`crates/governor-daemon/src/lock.rs:172-205`) —
    kernel lock as sole authority, record as corroboration, four-way decision with
    `LockHolderStillAlive` for the ambiguous case. The algorithm most directly
    reusable for Pi durable-task orphan reconciliation.
13. **Re-derivable process incarnation**
    (`crates/governor-daemon/src/incarnation.rs:78-128`) — PID plus a hashed
    start-time token, `SlotReused` classification, and the deliberate refusal to
    distinguish "gone" from "cannot tell".
14. **Pre-sized quarantine workload**
    (`crates/governor-store-sqlite/src/ops/recovery.rs:265-277`, `:290-304`) —
    count orphans outside the transaction, mint exactly that many identities, fail
    the whole pass closed if the pool is short.
15. **Prefix-fold transition verification**
    (`crates/governor-store-sqlite/src/replay.rs:226-267`) — re-fold `slice[..n]`
    for each n and compare against the recorded per-transition version.
16. **Bounded ancestor walk with separate cycle / depth-exhausted outcomes**
    (`@lineage-branch .../ops/session.rs`).
17. **Flat-object-only metadata parser**
    (`crates/governor-store-sqlite/src/safe_metadata.rs:249-324`).
18. **Introspection-driven sentinel scan**
    (`crates/governor-store-sqlite/tests/store_privacy.rs:521-557`) — enumerate
    every column from the engine so a later-added column is covered automatically.
19. **`ForemanClaim::rehydrate` / `BrowserWake::rehydrate`**
    (`claim.rs:178-201`, `delivery.rs:435-453`) — validating loaders that
    **re-derive** identity and fail closed: a persisted wake's deterministic key is
    re-derived from `(obligation, generation, revision)` and must equal the stored
    one (`WakeKeyMismatch`); a persisted claim's provenance is re-proved against
    the frozen accepted wake (accepted state, same generation, same obligation,
    correlating delivery id, non-backwards lifetime) or `ClaimProvenanceMismatch`.
    **A row whose identity cannot be re-derived must not authorise anything.**

### 4.7 Ordering contracts to transcribe verbatim

- **Crash-safe artifact publication**, eight steps — `docs/data-model.md`
  §"Crash-safe result publication"; implementation
  `crates/governor-artifacts/src/store.rs:1-52`, `:293-365`: bound check →
  `open(O_CREAT|O_EXCL|O_WRONLY|O_NOFOLLOW, 0600)` + `fchmod` → write → fsync →
  `link` → `unlink(staging)` → `fsync(objects/)` → **re-read verify** → mint proof.
- **Worker spawn/resume authorisation**, eleven steps —
  `@lineage-branch crates/governor-daemon/src/worker.rs` module docs and
  `@lineage-branch docs/data-model.md` §"Worker spawn/resume authorization".
  Read outside the transaction; re-check the same facts inside one; permit only
  after `COMMIT`; hand the adapter both permits **by value**. Includes the
  "why the permits are not fields" argument.
- **Create/claim browser delivery** — `docs/data-model.md`, including the
  three-part resolution of found-vs-created, the candidate correlation ID drawn
  before the transaction and discarded on the found path, and the
  live/frozen/past-fence/budget-exhausted conflict taxonomy.
- **Startup recovery order**, thirteen steps — `docs/architecture.md`, with the
  note distinguishing the binding requirement from the incidental ordering, plus
  the store's own six-step open (§3.5 A9).
- **Mutation-command transaction protocol** — `BEGIN IMMEDIATE` / insert unique
  `(actor_id, command_id, received)` / `COMMIT` / only then dispatch / commit the
  completed safe result before replying.
- **Claim expiry re-points the accepted wake**, guarded on
  `target_source_event_seq` — `docs/data-model.md` §"Foreman claims". Subtle,
  load-bearing, and impossible to re-derive: expiry bumps the obligation version,
  which would leave the only accepted wake permanently stale, so the same
  transaction re-points it, fenced on the source fact — which advances only on
  accepted worker events — still being current. It is what keeps OBL-008's
  reclaim path alive.
- **The three-phase write op** (`prepare` / `commit` / `finish`) —
  `crates/governor-store-sqlite/src/tx.rs:1-88`, runner `src/writer.rs:262-291`.
  Ports available only in phase 1; the durability assertion reachable only in
  phase 3. This is the structural mechanism behind "durable disposition before
  side effects".

### 4.8 The stable conflict-code vocabulary

44 codes on `feat/pi-native-foundation`
(`crates/governor-core/src/error.rs`; enumerated and asserted distinct and
`snake_case` at `tests/state_machine_invariants.rs:1466-1518`), 51 on
`@lineage-branch`. **Port these as the refusal vocabulary of the Pi-native
harness** — they are the machine-classifiable half of ADR 0002's error policy, and
a conformance test can assert on a code where it cannot assert on a message.

```
attempt_already_completed         attempt_already_dispatched
attempt_permit_mismatch           conflicting_input_answer
delivery_revision_frozen          delivery_revision_still_live
delivery_revision_superseded      effect_not_proven_absent
execute_requires_durable_intent   expired_claim
failure_not_proven                foreman_turn_not_quiescent
illegal_attempt_transition        illegal_delivery_transition
illegal_input_transition          illegal_lease_transition
illegal_mutation_transition       illegal_obligation_transition
invalid_disposition               loadout_digest_mismatch          *
loadout_identity_mismatch    *    managed_config_unverifiable      *
mutation_command_mismatch         mutation_not_completed
mutation_result_uncertain         no_active_binding
no_current_claim                  no_current_lease
no_session_loadout           *    obligation_already_claimed
obligation_closed                 parent_turn_not_owned_by_parent_session *
resource_already_leased           retry_after_ambiguity_fence
retry_budget_exhausted            retry_requires_idempotency_contract
session_incarnation_already_bound * session_lineage_cycle          *
session_lineage_too_deep     *    stale_binding_generation
stale_claim                       stale_command_revision
stale_daemon_epoch                stale_delivery_target
stale_lease_token                 stale_obligation_version
stale_process_incarnation         stale_session_incarnation
stale_source_fence                unknown_attempt
unknown_binding_generation        unknown_delivery_id
```
`*` = lineage-branch only.

Plus the daemon's startup/doctor refusal codes (`authority_held`,
`lock_holder_still_alive`, `state_root_invalid`, `store_refused`,
`ipc_unavailable`, `schema_epoch_too_new`, `migration_checksum_mismatch`,
`unknown_applied_migration`, `connection_policy`, `corrupt_value`,
`repair_needed`, `quarantine_incomplete`, `writer_gone`,
`unreadable_needs_owner_recovery`) and the health-condition kinds
(`suspected_stall`, `foreman_unreachable`, `mcp_write_capability_missing`,
`browser_binding_displaced`, `result_artifact_missing`, `projection_mismatch`,
`runtime_state_conflict`, `input_detail_unavailable`,
`worker_defer_shape_unsupported`, `reconciliation_required`, and
`@lineage-branch managed_config_missing`).

### 4.9 The acceptance-ID catalog

`docs/testing.md` defines 99 IDs across eight families — OBL 1–10, ART 1–11,
DEL 1–18, GPT 1–12, WRK 1–25, INP 1–11, DB 1–8, SEC 1–10 — plus SES 1–6 on the
sibling branch and the pattern review's numbered tests 1–12. **64 are implemented
in `governor-testkit`**, with per-file coverage tables mapping test → ID → status,
including IDs another crate proves and IDs deferred to a later gate.

Every ID's *definition* lives in `docs/testing.md`; the code only maps to it. That
separation is what makes the catalog portable: the Pi-native suite can re-implement
the mapping note without touching the definitions.

Implemented here: OBL 001–010; ART 001–005; DEL 001–018; GPT 001–009; DB 001–008;
SEC 001–010; pattern-review 1–6 and 9–12 (7 and 8 named-only, proven twice
elsewhere); plus a `determinism` family. Deferred with reasons stated: ART 006 and
011, WRK and INP entirely (need the worker host, managed-run staging, the hook
inbox or live Claude — Phase 2 and Live Gate C), GPT 010–012 (Live Gate A).

---

## 5. The harness contract — what `governor-testkit` actually specifies

No gate obsoletes this crate. It is the shape of the conformance suite, and a
Pi-native successor must reproduce these capabilities or later gates inherit a
harness that cannot express their failures.

**A real state root, never in-memory.** *"[In-memory] cannot express WAL,
`synchronous=FULL`, or reopening the same bytes, and all three are under test."*
The Pi-native equivalent is a real session/task store directory, not a mock.
Capabilities: open / open-at-a-given-instant / open-with-a-failpoint-hook; an
**independent read-only inspection connection**; raw byte access to the store
**and its sidecars**; a walk over every regular file under the root; an open
counter. The log directory is created **empty at setup** so the sentinel sweep
already covers a surface the daemon will later write to, rather than needing to be
widened by someone who remembers to.
`crates/governor-testkit/src/harness.rs`;
`crates/governor-store-sqlite/tests/support/mod.rs:1-5`.

**Seed/restart isolation.** `SEED_STRIDE = 1_000_003`; each open derives
`seed + generation * STRIDE`, *"because a real CSPRNG never repeats across
restarts and a harness that did would hide exactly the bug the suites hunt"* — two
"different" correlation IDs that are in fact equal.
`crates/governor-testkit/src/harness.rs:45`.

**Two domain-separated seeded streams.** A hand-rolled SplitMix64 (ten lines, no
dependency, identical bytes on every machine and toolchain), with **identity and
randomness as separate streams**: a testkit whose ID source and CSPRNG drew from
one counter would make DEL-001 untestable, because the two would agree by
construction and a bug deriving the correlation ID from an identity would pass.
Independence is asserted for 1024 seeds. Plus seeded storage keys carrying the
seed in the name so two roots in one scenario cannot collide, and a fixed key for
immutability tests. `crates/governor-testkit/src/rng.rs`;
`crates/governor-testkit/tests/determinism.rs:147`.

**Clock control.** A shared, manually advanced instant: a clone handed to the store
and a clone kept by the test read the same value, so time can move while the store
is open. Two modes — frozen, and stepping 1 ms per *reading* (so total elapsed is a
function of how many instants were asked for, never of wall time). The default
start is `1_000` ms: small and far from any real epoch value, *so a timestamp
leaked from a real clock into a fixture is obvious*. Drives claim expiry, retention
grace and orphan grace deterministically.
`crates/governor-testkit/src/clock.rs`;
`crates/governor-store-sqlite/tests/support/mod.rs:34-59`.

**Fault-injection taxonomy — named points, not ad-hoc kills.** Two symmetric hook
traits, deliberately shaped alike so one matrix walks both halves of a composed
operation.

*Store failpoints*, keyed on `(operation_name, point)`:
`AfterEventAppend`, `AfterProjectionUpdate`, `BeforeCommit` (the three ledger
windows); `AfterIntentInsert` (intent recorded, before dispatch);
`AfterMutationReceived`, `AfterMutationResult` (the journal's two halves);
`BeforeMigrationRecorded` (a separate list, reachable only during migration).
Crossed with **23 named write operations** in the store suite and **27** in the
testkit — a matrix, not a hand-picked subset.

*Artifact failpoints*: every step of crash-safe publication (staging, fsync,
publish, parent fsync).

Three design properties to port:
- **Fires exactly once at an exact target.** A different point or operation is
  inert; the second hit of the same target is inert.
- **"Never fired" is a real answer, not a failed injection.** It means the
  operation does not pass through that point; the same assertions still apply. The
  runner *returns* whether it fired rather than asserting it.
- **Unknown variants fail closed.** The label function panics on a variant not in
  the list, so a crash window added upstream surfaces as a panic in the first
  matrix that reaches it, never as a silently smaller matrix.
`crates/governor-testkit/src/failpoints.rs:35-57`, `:66-78`, `:86`;
`crates/governor-store-sqlite/tests/support/mod.rs:102-130`.

**The kill-window oracle — the single most portable idea in the crate.** Five
steps, and the order matters:
1. build the prefix with the crash **already armed** (it targets one named point
   of one named operation, so the prefix is unaffected);
2. fingerprint the whole store through an independent connection;
3. run the operation under test;
4. **fingerprint again before reopening** — so recovery cannot mask a
   half-transition; a rejected operation must have changed *nothing at all*;
5. reopen, and require replay verification to succeed.

Step 5 is what makes **replay the oracle** rather than a hand-written expected
state. A `restart_loop(harness, times, body)` covers the 100-restart cases.
`crates/governor-testkit/src/restart.rs:93`.

**Whole-store fingerprinting.** Several requirements are stated as *nothing was
mutated*. Counting rows in the one table a test happens to think about proves far
less than it looks like — a rejected ACK that advanced a version, stamped a
deletion instant, or touched a claim row would pass. So the comparison is **every
table, every column, every row, rendered and sorted**, driven by the store's own
introspection so a table added later is compared automatically. Process-scoped
tables (daemon epoch, projection watermark) are excluded because they move on every
open and comparing them across a restart would report a non-mutation. Column search
uses `instr` rather than `LIKE` so a needle containing `_` or `%` does not widen the
search. The lineage suites use the same idiom (`assert_unchanged`) in preference to
a narrow count, for the same stated reason.
`crates/governor-testkit/src/dump.rs`;
`@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:14-17`.

**Boundary fakes that *enforce* rather than assert — the crate's central idea.**
Three fakes hold their **own read-only connection to the committed store** and
**panic rather than acting** when it does not already show the required state:

| Fake | Refuses to act until |
| --- | --- |
| fake browser | the attempt is `claimed`; Send additionally needs `activation_armed` |
| fake external destination | the intent row is committed **and** the dispatch fence is set |
| foreman bootstrap | — it *cannot* disclose an identity, because no query selects one |

Reading through a **second** connection is what makes "the store shows" mean
*committed*, not merely written inside a transaction the writer still holds. A
daemon that reordered the claim transaction after navigation, or armed the fence
after Send, takes the panic on the first cell of the matrix. **This is what makes
DEL-003, DEL-005 and pattern-review-1 properties of the boundary rather than
assertions a test remembered to write**, and it is the most important thing to
reproduce in a Pi-native harness — where the equivalent is a transport shim that
reads the durable task store before it will emit anything.
`crates/governor-testkit/src/lib.rs:39-52`; `src/browser.rs`; `src/effect.rs`;
`src/foreman.rs`; `crates/governor-store-sqlite/tests/store_durability.rs:74-99`.

The fake browser also models the page as data — bound vs resolved conversation,
app selected, composer ready, target present, send behaviour — records **every
physical submission** and every method call in order, supports mid-scenario
displacement (between staging and activation), and offers an
`assert_untouched()`. Send behaviour enumerates the four outcome classes:
`Submit`, `RefuseActivation` (synchronous, provably nothing sent),
`LoseObservation` (a message may be on the wire), `WeakSignalOnly(signal)`. And
**`may_have_submitted()` returns false only for a synchronous refusal —
everything else is recorded as a physical send regardless of what the adapter
could observe.** That asymmetry is the harness encoding the invariant.

**Composed scenario fixtures.** Thin wrappers driving the *real* operations, so a
suite reads as a sequence of domain facts: an "accepted work" prefix (project,
task, session, turn, binding, worker start, real artifact publication, scheduled
wake, exact acceptance evidence) in one call; a "handed over" extension to
`processing`; already-lapsed and live claim constants plus a deterministic
lapse helper. `publish_result()` is the composition that matters: **it publishes
real bytes to disk first and only then commits** — the file-before-database
ordering itself, not a stand-in for it. And
`assert_no_completion_without_durable_bytes()` is **the forbidden outcome**, the
terminal assertion every crash-matrix cell ends with.
`crates/governor-testkit/src/scenario.rs`.

**Sentinels.** See §3.12 for the corpus and the two-reasons split. The design
points a port must keep: each sentinel records *which of two different rules* keeps
it out; a unit test asserts those labels against the actual charset so the fixture
cannot drift from reality; the token-shaped four are pushed through real public
fields and asserted **confined**, with reaching *nothing* also failing; and a
separate helper sweeps values the run itself generates across **output surfaces
only**, warned in its own documentation because scanning the database would fail
for the one reason that is correct.
`crates/governor-testkit/src/sentinels.rs:136`, `:389`.

**What the crate refuses to claim — port the discipline.** Phase 1 has no store
write path to `needs_input`, no worker-command projection, no health raise other
than startup reconciliation, and no reconciliation-of-record promotion. Where a
documented test needs one, the suite implements the pure or fake half **and says
so in its coverage table**. *None of them pretends the durable half passed.* Every
acceptance file opens with that table, so coverage is auditable without grepping.
`crates/governor-testkit/src/lib.rs:54-61`.

---

## 6. Per-crate migration notes

Format: what it is → which gate(s) passing make it obsolete → what must be
preserved first → archive verdict.

### `governor-core` — pure domain model and state machines

*Contents.* Typed identities and fences; the obligation, delivery/attempt, claim,
input, worker-command, binding, foreman-turn, lease, mutation and external-effect
machines; `DeliveryKey::derive`; `SafeToken`; the `Conflict` vocabulary; the
health model; the watchdog; the evidence-precedence table; the execution-permit
seam. No clock, entropy, network, process or database access. On the sibling
branch it additionally carries `session.rs` (loadouts, lineage,
capability/delegation whitelists, `CommittedLoadout::rehydrate`/`admit_resume`),
`digest.rs`, and the frozen vectors.

*Obsoleted by:* **P2 and P4 together, and not before.** The obligation machine is
the foreman loop's state; the delivery/attempt machine is the at-most-once
contract; the loadout/lineage machine is Gate P2's least-authority requirement.
P1 alone touches almost none of it.

*Preserve first — highest priority in the workspace:*
1. `digest.rs` absorption rules and `tests/persisted_digest_vectors.rs` (§4.2,
   §4.3) — inputs as well as outputs.
2. `DeliveryKey::derive` including `WAKE_KEY_DOMAIN` and the revision-numbering
   rule (§4.1).
3. The `Conflict` code vocabulary (§4.8).
4. `tests/state_machine_invariants.rs` (1550 lines) and
   `tests/durable_execution_invariants.rs` (1048 lines) as the transition-legality
   oracle. **These are the cheapest tests in the repository to re-express in any
   language, because they need no substrate at all.**
5. The **retry-classification precedence** and the **at-most-one-live-revision**
   rule (§3.1) — implementation discoveries absent from the ADRs.
6. The whitelist-only default (`new(id, [])` grants nothing) and the
   **two-constructor split** — a loadout assembled at run time cannot stand in for
   the one a session was launched under (§3.8).
7. `lib.rs:51-101`, which carries **two ready-made tables**: the 17 numbered
   state-machine invariants mapped to their enforcing type, and the 8
   durable-execution rules mapped likewise. `lib.rs:71-74` names which invariants
   (2, 3, 10, 11, 12) have a **durable half** owned by the store and daemon that
   this crate deliberately does not enforce. Copy both tables into the Pi-native
   design notes.

*Verdict:* **archive last.** Roughly 80 % product semantics, 20 % language.

### `governor-store-sqlite` — the durable authority

*Contents.* Schema and migrations; the single-writer actor; the three-phase write
op; per-domain write operations; the replay/projection verifier; the safe-metadata
codec; validating loaders; the connection-policy prover; recovery/quarantine.

*Obsoleted by:* **P1 and P2.** P1 gives version/epoch drift detection and the
"refuse to operate against state you cannot interpret" family; P2 gives durable
subagent state. ADR 0008 §5 permits a Pi-native durable sidecar, so "obsolete"
here means *this SQLite implementation*, not *durable state*.

*Preserve first:*
1. **A1/A2** — verify declared configuration by reading it back from the runtime,
   and prove a declared setting has effect (§3.5). The most directly transferable
   invariant in the workspace, and the literal content of Gate P1.
2. **The epoch gate and the checksum/unknown-version drift taxonomy** (§3.5),
   including `is_fail_closed()` as a single predicate to branch on.
3. **The replay-equivalence method and its stated residue** (§3.7 G53), including
   the enumeration of what cannot be compared and why.
4. **G57** — record the *presented* fence, not the derived one, or the
   compare-and-swap is trivially true on every replay.
5. **The ordering contracts** in §4.7, especially the three-phase write op and the
   claim-expiry wake re-point.
6. **Retention derivation** (§3.4) — derived not set, `COALESCE` on the release
   instant, no instant means keep forever.
7. **The `StorePorts` shape as a design idea** — making "no I/O inside a
   transaction" a property of the code's shape. The Rust mechanism does not port;
   replace it with an explicit source scan plus a panicking double, **and record
   that substitution as a deliberate weakening.**
8. **The source-identity derivation rule** for the ledger's uniqueness fence:
   stable non-secret facts only, never content (§3.3).
9. **The privacy structure** — four value shapes, no free-form accessor, a
   flat-object-only parser, per-kind allowlists, and `tests/store_privacy.rs`'s
   pinned column inventory as *a lock, not a description* (§3.9, §3.12).

*Verdict:* **archive after P1 and P2 pass**, and only once the Pi-native store has
its own DB-001/002/003/004/006/008 equivalents.

### `governor-artifacts` — immutable private result-artifact store

*Contents.* Content-addressed store with crash-safe publication, verify-on-read,
retention/GC, rooted no-follow path handling, forced owner-only modes.

*Obsoleted by:* **P2 with a durable result store**, plus **P5** (a reviewer can
read the exact result). **Pi's session persistence alone does not obsolete it** —
ADR 0008 §5 explicitly permits extension-owned durable artifacts, and this crate
*is* that. If a chosen Pi task/artifact package stores results, it must satisfy
these invariants before this crate is archived.

*Preserve first:*
1. **The durability-proof token** (§3.4) — a private-field value with one
   construction site on the far side of the barrier, consumed by the transaction.
   *Make "durable" a token only the durability step can mint.*
2. **The publication ordering and `link`-not-`rename`** with the `EEXIST`-versus-
   silent-replace reasoning (§4.7, §3.4).
3. **The read-back post-condition** and the **bounded verified read** at
   `expected_len + 1`.
4. **`gc::decide` verbatim** (§4.6 item 10) and the orphan/quarantine rules.
5. **The ART-001 crash matrix**, the ART-002 four-attempts-at-GC test with grace
   set to zero, ART-003's no-bytes-on-mismatch, the ten path-safety cases, and
   ART-005's hostile-umask child-process technique.
6. **The trust-model test that asserts what the system does *not* protect
   against** — detection, not prevention (§3.12).

**Highest transfer risk in this crate:** `link`-not-`rename` and hard-link
detection. A reimplementation will reach for `rename` and skip `nlink`.

*Verdict:* **archive after P2** — but of the six crates this one has the highest
ratio of portable specification to Rust-specific code, so if the Pi harness keeps
a filesystem artifact store, its *tests* are directly reusable as a specification
and only the implementation is replaced.

### `governor-testkit` — deterministic fakes, failpoints, scenarios, acceptance suites

*Obsoleted by:* **none of P1–P6 individually.** It is superseded only by a
Pi-native conformance harness existing.

*Preserve first:* effectively all of §5, and in priority order: the
fault-injection taxonomy with named points; the kill-window oracle; whole-store
fingerprinting; the two domain-separated seeded streams and the seed stride; the
restart primitive; the boundary fakes that panic rather than act; the sentinel
corpus with its representability self-check; and the coverage-table discipline
that states which half of an ID is unproven.

*Verdict:* **never archive without a successor.** Port the harness contract first
and the suites incrementally, family by family, **keeping the IDs**. This is the
crate whose loss would be least visible and most damaging.

### `governor-daemon` — composition library (layout, lock, incarnation, startup, doctor, IPC, logging)

*Obsoleted by:* **P1** for layout/precedence/version-drift, **P2** for
spawn/resume authorisation. The IPC surface is obsoleted by the pivot itself
(ADR 0008 §2), not by a gate. The composition-root role largely dissolves into
Pi's own startup; the **election** role dissolves only if no Command Governor
durable sidecar ever runs concurrently — and the moment one does, §3.6 applies to
it.

*Preserve first:*
1. `@lineage-branch src/worker.rs`'s **eleven-step ordering** and its "why the
   permits are not fields" argument — the clearest statement anywhere of *read
   outside, re-check inside, permit only after commit*.
2. **The four-way reclaim decision** including `LockHolderStillAlive` for the
   ambiguous case, and the never-unlink rule (§3.6).
3. **Re-derivable process incarnation**, and the deliberate refusal to distinguish
   "gone" from "cannot tell" (§3.6).
4. **The scoped-versus-fatal taxonomy** and the "mechanism, not a flag" point
   (§3.6) — the highest-value operational judgement in the workspace.
5. **The read-only-diagnosis discipline**: creates nothing, repairs nothing, the
   one probe named rather than buried, and the watermark reported as a *note*
   rather than a check it cannot perform (§3.10).
6. **Version-drift check shape** and the drift taxonomy (§3.5).
7. **Resource precedence in `layout.rs`** — Gate P1 names *"project/global resource
   precedence characterized"* and `default_location()` (`layout.rs:113-133`) is
   the existing characterisation, with three P1-relevant details: an **empty**
   environment variable does not count; the fallback chain is explicit and
   ordered (`CG_STATE_ROOT` → `XDG_STATE_HOME/command-governor` → the platform
   per-user location); and exhaustion is a **usage error rather than a guess at
   `/`**. Every derived path is asserted to stay inside the root
   (`layout.rs:352-369`).
8. **The `live.` provenance prefix** and the **closed-set logging design with no
   free-string accessor** (§3.10).
9. **Preflight-everything-knowable-before-mutating** (§3.5), generalised beyond
   socket paths to any Pi package-loading precondition.

*Verdict:* **archive after P1 and P2**, preserving the two ordering documents and
the reclaim decision table as Pi-native design inputs.

### `command-governor` — CLI entry point and real-process acceptance suite

*Obsoleted by:* **P1** delivering an equivalent Pi-native entry point with
classified refusals and stable exit codes. The thinnest crate and the first that
can go.

*Preserve first:*
1. **`tests/daemon_acceptance.rs` almost in its entirety** — the only place two
   *real OS processes* contend. **DB-005's falsifying test has no substitute:**
   started against a state root with no database yet, the second daemon still
   refuses, and the epoch afterwards is still 1.
2. **The CLI-output sentinel sweep including the refusal path** — the wake
   correlation ID must appear on no output surface *even when a projection
   mismatch is being reported*. Error paths are where redaction usually fails.
3. **The exit-code contract** and the **total-parser rule** (§3.10).
4. The principle that **a client surface is a projection of durable truth, never
   an authority** (ADR 0001 decision 10).

*Verdict:* **archive first**, provided the process-level tests above are re-homed.

### One observation that spans all six

Several of these invariants are valuable specifically because they are
**falsifiable**: the trust-model test asserts what the system does *not* protect
against; the doctor refuses to claim a check it cannot perform; DB-005's process
test distinguishes two mutually exclusive mechanisms rather than confirming one;
the sentinel injection fails if a sentinel reaches *nothing*. **A Pi-native
conformance harness that only asserts happy paths will lose exactly the properties
that were hardest to get right.**

---

## 7. The ten invariants to encode first (Gate P1 foundation PR)

Gate P1's literal scope is substrate pinning, package loading, resource
precedence and version drift. But a harness that can only express P1's assertions
will not be able to express P2–P6's, and retrofitting fault injection and restart
semantics into an existing suite costs far more than designing for them. So this
is **P1's own invariants plus the minimum scaffolding the later gates require** —
which is the scope note the task asks for. Ordered by the cost of getting them
wrong later.

1. **Harness restart primitive.** GIVEN durable state and a running scenario;
   WHEN a restart is requested; THEN a new process lifetime observes the same
   durable state, the seeded streams advance by a stride rather than repeating,
   and the assertion can be repeated N times (100 in OBL-005 / DB-007 / SES-004).
   *Why first:* every durability invariant in P2–P4 is expressed across a restart.
   Without this primitive none of them can be written at all.
   §5; `crates/governor-testkit/src/restart.rs`, `src/harness.rs:45`.

2. **Fault-injection taxonomy with named points, plus the kill-window oracle.**
   GIVEN a scenario; THEN it can inject a failure at a *named, stable* point —
   before-send, after-arm, after-send, before-disposition, after-disposition,
   before-outcome-commit, each artifact-publication step, each migration
   failpoint — with the three design properties in §5 (fires once at an exact
   target; "never fired" is a real answer; unknown variants fail closed). And the
   five-step oracle: arm the crash before building the prefix, fingerprint,
   run, **fingerprint again before reopening**, then reopen and require replay
   verification.
   *Why first:* ADR 0008 Gate P4 requires exactly this (*"inject failures
   before/during/after send and before/after local disposition"*). Naming the
   points now is what lets P4's tests be written as data rather than bespoke code,
   and step 4 is what stops recovery masking a half-transition.
   §5; `crates/governor-testkit/src/failpoints.rs`, `src/restart.rs:93`.

3. **Injected clock, CSPRNG and ID source — as two domain-separated streams.**
   GIVEN a scenario; THEN time, entropy and generated identities come from
   injected sources; identity and randomness are **separate** streams, asserted
   independent; and production uses OS entropy.
   *Why first:* the duplicate-send and correlation tests cannot fail
   deterministically without controllable entropy, DEL-001's *">= 192 bits, and
   not a hash of deterministic metadata"* is unassertable without both streams,
   and a shared counter would make the two agree by construction — *"or every
   possession-fence assertion is vacuous."*
   §5; `crates/governor-testkit/src/rng.rs`, `src/clock.rs`;
   `crates/governor-testkit/tests/determinism.rs:147`.

4. **Verify the declared configuration by reading it back from the runtime
   (A1/A2/WRK-020).** GIVEN the pinned Pi release and the Command Governor
   distribution; WHEN loaded; THEN the **resolved** set of extensions, roles,
   skills and settings is reported and asserted against an expected manifest;
   project-versus-global precedence is proven by observation, not inferred from
   the flags passed; and a declared restriction is proven to actually block the
   thing it claims to block.
   *Why first:* this is Gate P1's core, and the failure mode has a documented
   precedent in WRK-020 — *no code path may promote a signal merely because a
   settings flag was assumed to isolate hooks.* Same failure, new substrate.
   §3.5; `crates/governor-store-sqlite/src/open.rs:1-9`, `:121-154`;
   `crates/governor-store-sqlite/tests/store_policy.rs:18-44`, `:46-66`.

5. **Version and epoch drift fails closed and mutates nothing (DB-003 + A5/A6).**
   GIVEN durable state whose compatibility epoch is above what the loaded
   distribution understands; WHEN it starts; THEN it refuses with both numbers
   reported, performs no downgrade and no mutation, and refuses on **every**
   subsequent open. GIVEN a content-hash of the loaded package set that differs
   from what the state records; THEN drift is a typed refusal, distinguishable
   from "you are behind" and from "history was rewritten".
   *Why first:* Gate P1's literal *"version drift detected"*, and the only P1
   assertion with a durable-state consequence.
   §3.5; `crates/governor-store-sqlite/src/migrate.rs:96-127`;
   `crates/governor-daemon/src/doctor.rs:348-359`, `:438-455`.

6. **Digest pre-images are frozen, with vectors.** GIVEN the four frozen vectors
   in §4.3; THEN the Pi-native implementation reproduces them byte-for-byte, and
   the length-prefix property tests pass
   (`["ab","c"] != ["a","bc"]`; `["","a"] != ["a"]`; `u64(1) != u32(1)`).
   *Why first:* cheap, pure, and the only invariant here whose violation is
   completely silent. Encoding it now means a later `delivery_key` port cannot
   drift.
   §4.2, §4.3.

7. **Typed refusals carry stable codes, and a refusal changes nothing —
   whole-state.** GIVEN any refused operation; THEN it returns a code from the
   stable vocabulary (§4.8), and a whole-store fingerprint taken before and after
   is identical: no version advanced, no retention instant stamped, no event
   written.
   *Why first:* "zero state mutation" appears in OBL-003, OBL-009, SEC-004,
   GPT-008, and every SES refusal. If the harness cannot make that assertion
   **broadly**, each of those tests silently narrows to whatever table its author
   happened to check — *"a refusal that advanced a version, stamped a retention
   instant or wrote an event would pass a narrower check."*
   §5; `crates/governor-testkit/src/dump.rs`;
   `@lineage-branch crates/governor-testkit/tests/ses_acceptance.rs:14-17`.

8. **Boundary fakes that read the committed state and panic rather than act.**
   GIVEN any adapter double — transport, external destination, spawner; THEN it
   holds its own **independent** reader of the durable store and refuses to
   perform its action unless the required durable state is already **committed**.
   *Why first:* this is the runtime replacement for the type-level capability
   tokens the port loses (§3.14). It converts "durable disposition before side
   effects", "claim before I/O" and "arm before send" from assertions a test
   remembered to write into properties of the boundary itself — and it is
   scaffolding, not a one-line assertion, so it must exist before the tests that
   depend on it.
   §5; `crates/governor-testkit/src/lib.rs:39-52`;
   `crates/governor-store-sqlite/tests/store_durability.rs:74-99`.

9. **Sentinel sweep infrastructure, with the representability self-check.** GIVEN
   the sentinel corpus; THEN a test asserts the corpus matches the charset claim
   made about it rather than trusting its label; token-shaped sentinels are
   actually injected through real public fields and asserted **confined to one
   location** (reaching nothing also fails); run-generated secrets are swept
   separately across **output surfaces only**, including refusal paths; and the
   surface list is built by walking the state root so a new file joins
   automatically.
   *Why first:* every later gate adds persistence surfaces — Pi session JSONL,
   compaction artifacts, memory stores, analytics, task spools, browser job
   records. A sweep that exists from the start grows with them; one added at P6
   has to be retro-validated against everything already written. And both
   candidate transports handle live cookies and bearer tokens (§3.12 SEC-009).
   §3.12, §5; `crates/governor-testkit/src/sentinels.rs`.

10. **The three ACK layers are distinguishable in the chosen composition.** GIVEN
    whatever Pi task or notification package the distribution adopts; WHEN every
    transport receipt and attention acknowledgement it offers is used, plus a
    landed foreman delivery and a completed external effect; THEN Governor-owed
    work remains open until a semantic foreman disposition is durably recorded.
    *Why first:* this is a **composition-review** test, and the foundation PR is
    where the package set is pinned. ADR 0008 lists `@geminixiang/pi-task-protocol`
    as a first candidate and its acknowledgements are layer 2. If the foreman
    disposition is mapped onto a package's built-in ACK, the product's defining
    invariant is lost **at the moment of composition**, and every later gate tests
    the wrong thing. ADR 0008 §6, by removing MCP, makes this confusion easier to
    fall into.
    §3.4; `docs/research/2026-08-31-durable-orchestration-pattern-review.md`
    §"The three ACK layers must remain separate";
    `crates/governor-testkit/tests/research_acceptance.rs:513`.

**Deliberately not in the first ten, with reasons.** DEL-004's retry
classification and DEL-015's promotion asymmetry are higher-value invariants than
several above, but they are Gate P4 semantics needing a transport to test against;
encoding them now would mean encoding them against a mock whose fidelity is
unproven. SES-002's byte-read-not-metadata rule is arguably the sharpest test in
the repository, but it needs a managed configuration artifact to exist first.
**Both should be the first additions after P1**, and item 8 above is precisely the
scaffolding that makes them cheap when they arrive.

---

## 8. Open questions the foundation PR should decide rather than inherit

1. **The prose field in `FOREMAN_ACTION`.** ADR 0008's envelope carries
   `instructions / delegation / question`. Phase 1 deliberately has **no**
   free-text answer variant and records both the reason and the open question
   (`docs/data-model.md` §"The structured answer set"). A ChatGPT Web foreman
   replies in prose by construction, so this field will exist. Decide its bounded
   size, classification, retention and redaction contract **before** implementing
   it, not after.
2. **Prompt injection into the reply parser.** Reading a prose foreman reply
   directly (§6 of ADR 0008) creates an injection surface the MCP topology did not
   have. A conformance test where a well-formed action envelope appears inside
   quoted untrusted content, and is *not* accepted as the disposition, belongs in
   Gate P4. This is the most significant new risk the pivot introduces.
3. **Whether `agent_settled` is genuinely non-vetoable.** ADR 0008 and the Pi
   review both assert it is. WRK-003/WRK-004 exist because an analogous claim about
   Claude's `Stop` hook was wrong, and architecture review R2 records a second
   provider-semantics assumption that had already gone stale once. **Verify against
   the pinned Pi source; do not inherit the claim.**
4. **Whether a transport can supply exact message identity.** DEL-014 makes
   acceptance require it. A transport that cannot forces every send into
   `ambiguous` — correct, but it may make the direct-API path strictly preferable
   to the browser path. That is an architectural finding, not just a test result,
   and it should be settled during the P4 transport spike.
5. **Whether the lineage/loadout branches are merged, ported, or archived.** They
   are not on `feat/pi-native-foundation`, and they carry the most implemented
   oracle material for ADR 0008 §4.6. Leaving them unmerged *and* unarchived is
   the state most likely to lose them.
6. **What replaces the type-level enforcement.** "No I/O inside a transaction", "no
   spawn without a durable intent and a re-proved snapshot", "no completion from a
   vetoable callback", "no acceptance from a weak signal" are currently
   *unrepresentable* rather than discouraged. Whatever replaces them (item 8 of
   §7) is weaker. **Record the weakening explicitly in the migration PR** rather
   than discovering it later.
7. **Whether "exactly one active binding" survives.** ADR 0004's singleton may not
   hold in a Pi harness addressing several conversations. The generation fence
   generalises to per-binding cleanly; the singleton should be re-decided, not
   inherited.
8. **Whether a Command Governor durable sidecar will exist.** ADR 0008 §5 permits
   one. If yes, the whole §3.6 family — single authority, reclaim-requires-proof,
   process incarnation, lease fencing, quarantine-before-new-I/O — becomes a
   requirement on it, and DB-005's real-process test needs a Pi-native equivalent.
