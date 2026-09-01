# Migrating off the Rust Phase-1 crates

Which gate obsoletes which crate, what has to be preserved before anything is
archived, and the three design decisions the foundation must settle rather than
inherit.

Source: [`../research/2026-09-01-rust-invariant-catalog.md`](../research/2026-09-01-rust-invariant-catalog.md).
ADR 0008 §10 freezes the Rust scaffold for feature expansion and keeps it as
migration oracle material; this is the reading of what that material actually
contains.

---

## Read this first: most of the oracle material is not on this branch

The ADR 0007 lineage and loadout implementation — the single item with the most
implemented oracle material for ADR 0008 invariant 6, *"resumed loadouts are
explicit and least-authority; resume cannot silently broaden an old worker under
new defaults"* — **is not on `feat/pi-native-foundation`.** `grep -ri
'loadout\|lineage'` over `crates/` on this branch returns nothing.

It lives on two unmerged sibling branches:

- **`feat/session-lineage-loadout-core`** — `crates/governor-core/src/session.rs`
  (1547 lines), `src/digest.rs`, `tests/persisted_digest_vectors.rs`;
- **`feat/session-lineage-loadout-store`** @ `8cfbcd0` — schema **epoch 2**,
  `crates/governor-store-sqlite/src/ops/session.rs`,
  `migrations/0002_session_lineage_and_loadouts.sql`,
  `crates/governor-daemon/src/worker.rs`,
  `crates/governor-testkit/tests/ses_acceptance.rs` (779 lines), and
  **SES-001..006** plus required invariants 18–22 in the docs.

`SUPPORTED_SCHEMA_EPOCH` is `1` on this branch and `2` on the sibling — itself a
live instance of exactly the drift the epoch gate exists to catch.

**These branches must be read before any Rust crate is archived.**
`git show feat/session-lineage-loadout-store:<path>` suffices; a merge is not
required. Leaving them unmerged *and* unarchived is the state most likely to
lose them.

---

## Per-crate verdicts

| Crate | Obsoleted by | Archive verdict |
| --- | --- | --- |
| `command-governor` | **P1** delivering an equivalent Pi-native entry point with classified refusals and stable exit codes | **archive first**, provided the process-level tests are re-homed |
| `governor-daemon` | **P1** for layout, precedence and version drift; **P2** for spawn/resume authorisation. The IPC surface is obsoleted by the pivot itself, not by a gate | archive after P1 and P2 |
| `governor-store-sqlite` | **P1** and **P2**. "Obsolete" means this SQLite implementation, not durable state — ADR 0008 §5 permits a Pi-native sidecar, which inherits the same family | archive after P1 and P2, and only once the Pi-native store has its own DB-001/002/003/004/006/008 equivalents |
| `governor-artifacts` | **P2** with a durable result store, plus **P5**. Pi's session persistence alone does **not** obsolete it | archive after P2. Highest ratio of portable specification to Rust-specific code: if the Pi harness keeps a filesystem artifact store, these *tests* are directly reusable |
| `governor-core` | **P2 and P4 together, and not before.** P1 touches almost none of it | **archive last.** Roughly 80% product semantics, 20% language |
| `governor-testkit` | **none of P1–P6 individually.** Superseded only by a Pi-native conformance harness existing | **never archive without a successor** |

### What must be preserved first

**`governor-core`** — the digest absorption rules and their frozen vectors,
inputs as well as outputs; `DeliveryKey::derive` including the wake-key domain
and the revision-numbering rule; the `Conflict` code vocabulary; the two
transition-legality test files (2,598 lines combined, and **the cheapest tests
in the repository to re-express in any language, because they need no substrate
at all**); the retry-classification precedence and the at-most-one-live-revision
rule, both implementation discoveries absent from the ADRs; the whitelist-only
default where a loadout constructed with no grants grants nothing, and the
two-constructor split that stops a loadout assembled at run time standing in for
the one a session was launched under; and the two ready-made tables mapping the
17 state-machine invariants and the 8 durable-execution rules to their enforcing
type.

**`governor-store-sqlite`** — A1/A2, *verify declared configuration by reading it
back from the runtime*, which is the most directly transferable invariant in the
workspace and the literal content of Gate P1; the epoch gate and the
checksum/unknown-version drift taxonomy; the replay-equivalence method **and its
stated residue**, the enumeration of what cannot be compared and why; the rule
that the *presented* fence is recorded rather than the derived one, or the
compare-and-swap is trivially true on every replay; the three-phase write op and
the claim-expiry wake re-point; retention derived rather than set, with no
instant meaning keep forever; the source-identity rule of stable non-secret
facts only, never content; and the privacy structure — four value shapes, no
free-form accessor, a flat-object-only parser, per-kind allowlists, and a pinned
column inventory that is *a lock, not a description*.

**`governor-artifacts`** — the durability-proof token, a private-field value
with one construction site on the far side of the barrier; the publication
ordering and `link`-not-`rename` with the `EEXIST`-versus-silent-replace
reasoning; the read-back post-condition and the bounded verified read; the GC
decision procedure verbatim; the crash matrix, the four-attempts-at-GC test with
grace set to zero, the ten path-safety cases, and the hostile-umask
child-process technique; and the trust-model test that asserts what the system
does **not** protect against. **Highest transfer risk in the workspace:**
`link`-not-`rename` and hard-link detection — a reimplementation will reach for
`rename` and skip the link count.

**`governor-testkit`** — effectively all of it, in priority order: the
fault-injection taxonomy with named points; the kill-window oracle; whole-store
fingerprinting; the two domain-separated seeded streams and the seed stride; the
restart primitive; the boundary fakes that panic rather than act; the sentinel
corpus with its representability self-check; and the coverage-table discipline
that states which half of an ID is unproven.

**`governor-daemon`** — the eleven-step spawn ordering on the lineage branch and
its "why the permits are not fields" argument, the clearest statement anywhere
of *read outside, re-check inside, permit only after commit*; the four-way
reclaim decision including the ambiguous "lock holder still alive" case, and the
never-unlink rule; re-derivable process incarnation and the deliberate refusal
to distinguish "gone" from "cannot tell"; the scoped-versus-fatal taxonomy; the
read-only-diagnosis discipline that creates nothing, repairs nothing, and
reports a watermark as a *note* rather than as a check it cannot perform; and
resource precedence — an empty environment variable does not count, the fallback
chain is explicit and ordered, and exhaustion is a usage error rather than a
guess at `/`.

**`command-governor`** — the daemon acceptance suite almost entirely, the only
place two *real OS processes* contend; the falsifying single-authority test,
which distinguishes two mutually exclusive mechanisms rather than confirming
one, has no substitute; the CLI-output sentinel sweep **including the refusal
path**, because error paths are where redaction usually fails; and the
exit-code contract.

### One observation that spans all six

Several of these are valuable specifically because they are **falsifiable**: the
trust-model test asserts what the system does *not* protect against; the doctor
refuses to claim a check it cannot perform; the sentinel injection fails if a
sentinel reaches *nothing*. **A Pi-native conformance harness that only asserts
happy paths will lose exactly the properties that were hardest to get right.**

### The largest fidelity loss, recorded deliberately

Rust encoded several ordering contracts as types rather than as review rules:
durable intent before any external I/O, durable bytes before durable
disposition, claim before transport, arm before send. TypeScript has no affine
types, so every one becomes **checked** rather than **proven**. The replacement
is boundary fakes that hold their own independent reader of the committed store
and **panic rather than act** when it does not already show the required state —
which converts the property from an assertion a test remembered to write into a
property of the boundary itself, but is still strictly weaker. **This weakening
is recorded here rather than discovered later.**

---

## Three flagged design decisions

These are decisions to make, not findings to file.

### 1. The prose field in `FOREMAN_ACTION` needs a retention contract

ADR 0008's envelope carries `instructions / delegation / question`. Phase 1
deliberately had **no** free-text answer variant. A ChatGPT Web foreman replies
in prose by construction, so the field will exist.

Decide its bounded size, its classification, its retention and its redaction
contract **before** implementing it. It is typed as optional in
`harness/extensions/cg-foreman/transport.ts` with that note attached, so nobody
implements it by accident.

### 2. Prose-reply parsing is a prompt-injection surface and needs its own P4 test

Reading a prose foreman reply directly creates an injection surface the MCP
topology did not have. **This is the most significant new risk the pivot
introduces.**

The required Gate P4 test: a well-formed action envelope appearing inside quoted
untrusted content is **not** accepted as the disposition. Untrusted content
reaches a foreman thread routinely — a pasted issue body, a diff, a log — so
this is a normal path, not an exotic one.

### 3. `agent_settled` non-vetoability is an inherited claim

ADR 0008 and the Pi review both assert that `agent_settled` is the correct
completion signal and cannot be vetoed. That is inherited from documentation,
not measured.

WRK-003 and WRK-004 exist in the Rust workspace **because an analogous claim
about a different harness's `Stop` hook was wrong**, and the 2026-08-31
architecture review recorded a second provider-semantics assumption that had
already gone stale once. Verify it empirically against the pinned Pi.

`conformance/tier2/credentialed.test.ts` records this as a skipped test with
that reason attached rather than assuming it.

---

## The rest of the open questions

Recorded so they are decided rather than inherited:

- **Whether a transport can supply exact message identity.** A transport that
  cannot forces every send into `ambiguous` — correct, but it may make a direct
  API path strictly preferable to a browser path. That is an architectural
  finding, not a test result, and belongs in the P4 spike.
- **Whether "exactly one active binding" survives.** ADR 0004's singleton may not
  hold in a harness addressing several conversations. The generation fence
  generalises to per-binding cleanly; the singleton should be re-decided, not
  inherited. `Binding` in the transport interface is already per-binding fenced.
- **Whether a Command Governor durable sidecar will exist.** ADR 0008 §5 permits
  one. If yes, the whole single-authority family — reclaim requires proof,
  process incarnation, lease fencing, quarantine before new I/O — becomes a
  requirement on it, and the two-real-processes test needs a Pi-native
  equivalent.
- **Whether the lineage/loadout branches are merged, ported, or archived.** See
  the top of this document.

---

## Keep the acceptance IDs

`docs/testing.md` is already a complete, implementation-independent
specification of 99 numbered acceptance tests in eight families, plus 12
pattern-review tests and the six SES tests on the sibling branch. 64 of those
IDs are implemented in `governor-testkit`.

**Adopt the IDs verbatim.** An ID that survives the pivot is how a reviewer
proves the pivot did not quietly drop a requirement. Port the harness contract
first and the suites incrementally, family by family, keeping the IDs.

The first two additions after Gate P1 should be the retry-classification
precedence and the byte-read-not-metadata rule from the session suite. Both were
left out of the foundation only because they need a transport and a managed
configuration artifact respectively; the boundary-fake scaffolding is what makes
them cheap when they arrive.
