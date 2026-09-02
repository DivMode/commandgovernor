# The Prime adaptation layer (Issue #17)

What `governor/` adds on top of the pinned Prime Agent substrate, why each
piece exists, and how the three Issue #15 blockers are closed. The layer is
small by design: Prime's supervisor, workers, leases, journals, schedules,
RLM children, sessions and ACP are used as they are (ADR 0009 §3). The
Governor adds durable records Prime lacks and the fences Prime's semantics
leave to the client.

```text
Command Governor
  governor/governor.ts              the one object that speaks to a supervisor
  governor/mutation/classify.ts     D2: structural outcome classification
  governor/mutation/proof.ts        D2: the reviewed (command, code) proof matrix
  governor/mutation/ledger.ts       D2: durable intent, one id forever
  governor/session/registry.ts      D1: logical sessions, incarnations, lease
  governor/session/paths.ts         D8: canonical, fenced sessionPath
  governor/fs/durable.ts            durable write / exclusive create / unlink
  governor/process/identity.ts      (pid, processStartId) verdicts for fences
  governor/prime/client-identity.ts one journal identity per state directory
  governor/prime/env.ts             positive environment allowlist
  governor/prime/daemon-client.ts   JSONL client with the pin guard
  governor/prime/protocol.ts        the narrow protocol slice, diffed vs the pin
  governor/prime/substrate.ts       pins.json reader, supervisor spawn/stop
                │
        public daemon protocol v7 over a Unix socket
                │
Prime Agent v0.8.1: detached supervisor, resident/client-owned workers,
session leases, command journal, schedules, RLM children, sessions, ACP
```

## Identity model

| Prime concept | Governor meaning |
| --- | --- |
| `sessionId` | the stable logical session identity; the registry key |
| active-session id (`activeSessionId` / `id`) | an incarnation: one attachment of a worker process to the logical session; changes on every reopen |
| `sessionFile` / `sessionPath` | the durable handle; canonical and fenced by the Governor |
| worker pid, event-cursor generation | diagnostic attributes of an incarnation, never identity |
| `workerState` | lifecycle authority (`failed` is the recoverable state) |
| `activity` | not consulted (Issue #15 D10) |

Every mutation names `(sessionId, activeSessionId)`. The registry refuses a
stale `activeSessionId` before any I/O with a typed `stale_incarnation`
error, which is the port of the Rust oracle's WRK-018
`stale_session_incarnation` fence. `attach` records the event-cursor
generation the supervisor reports for the incarnation; a generation binds
to one incarnation once, and `assertCurrentCursor` refuses a cursor from
an earlier one with `stale_cursor`. Prime itself never honours a dead
generation as a position (Issue #15 D3, re-proved in the D1 test), so
this fence is belt to Prime's braces rather than the only thing holding.

## D2 — worker-loss mutation ambiguity

**Invariant.** For a mutating command, *worker transport lost + outcome not
proven* is `UNCERTAIN`, never `FAILED`. `FAILED` requires positive proof
that the external effect did not happen. An uncertain mutation never
receives a replacement command id and is never dispatched again
automatically.

**What the pinned supervisor does.** The command journal writes a receipt
before dispatch. If the worker dies mid-command, the supervisor's worker
client rejects with a bare `Error("Daemon worker socket closed")`; the
supervisor's catch path turns that into an untyped failure response and
records it in the journal as the durable result (`daemon-supervisor.ts`,
the `journalIdentity && !isSupervisorGenerationStale(error)` branch).
Retries under the same identity replay that failure. Only supervisor death
yields the typed `command_result_uncertain`.

**The structural discriminator.** Prime's typed error vocabulary is closed
and tiny: `missing_session_cwd`, `session_import_file_not_found`,
`session_already_active`, `command_result_uncertain`. The first three are
what `serializeDaemonError` (`daemon-errors.ts`) can produce. That is a
*vocabulary*, not a proof. The serializer runs in the supervisor's catch
path **and in the worker's** (`daemon-mode.ts`), so a typed code can be
relayed from a worker that has already acted, and whether a given code was
thrown before or after a command's external effect depends on the command.
The pinned build has a real case:

```text
import_jsonl
  -> worker AgentSessionRuntime.importFromJsonl
  -> copyFileSync(resolvedPath, destinationPath)      <- the effect
  -> SessionManager.open(...)
  -> assertSessionCwdExists(...)
  -> MissingSessionCwdError
  -> typed errorInfo.code = missing_session_cwd
```

`missing_session_cwd` arrives with the transcript already copied into the
session directory. A classifier that took any serialised code as pre-effect
proof (the one this PR shipped before review) called that FAILED, which is
false. Proof is therefore a property of the **pair** `(commandType, code)`,
and only of pairs a human has read the pinned source for
(`governor/mutation/proof.ts`, `REVIEWED_PROOFS`). The classifier keys on
the command type the Governor *sent*, never on the `command` field the
response claims.

| command | code | reviewed timing | thrown at (pin 514633727) |
| --- | --- | --- | --- |
| `import_jsonl` | `session_import_file_not_found` | pre-effect | `importFromJsonl`, first statement, before `mkdirSync`/lease/`copyFileSync` |
| `import_jsonl` | `missing_session_cwd` | **post-effect** | `importFromJsonl` → `assertSessionCwdExists`, after `copyFileSync` |
| `create` | `session_already_active` | **ambiguous** | supervisor `reuseWorkerForCreate` before `launchWorker`, **and** the worker's `createRuntime` → `acquireSessionLease` after it (which may first rename/remove a stale lease directory); the supervisor re-serialises both identically |

The `create` row was first recorded as pre-effect by enumerating one throw
site; the independent review of 50762f4 produced the worker-side response
through a spawned worker against the pinned daemon. It is the failure mode
this matrix exists to prevent, so the row is kept as a reviewed
*ambiguous* pair rather than deleted: only `pre_effect` is proof.

Everything not in the table is unreviewed. So:

| observation | verdict |
| --- | --- |
| `success: true` | COMPLETED |
| `errorInfo.code` reviewed **pre-effect** for this command | FAILED, with the review as proof |
| `errorInfo.code` reviewed **post-effect** for this command | UNCERTAIN (`typed_failure_post_effect`) |
| `errorInfo.code` reviewed **ambiguous** for this command | UNCERTAIN (`typed_failure_ambiguous`) |
| `errorInfo.code` known but the pair unreviewed, or the command unknown | UNCERTAIN (`typed_failure_unreviewed`) |
| `errorInfo.code` unknown to the pin | UNCERTAIN (`unknown_error_code`) |
| `errorInfo.code = command_result_uncertain` | UNCERTAIN (substrate says so) |
| any other `success: false` (untyped) | UNCERTAIN |
| socket lost, write failed, timeout | UNCERTAIN |

No error text is read. A future Prime that rewords a message, adds a code,
or moves a throw past an effect lands in UNCERTAIN, not FAILED: the guard
fails closed by construction. `assertProductionPolicy` refuses any policy
that maps untyped failure or transport loss to FAILED, or that treats a code
as proof regardless of command. `conformance/tier1/prime-protocol.test.ts`
diffs the vocabulary and the read/mutating split against the pinned build,
asserts that the worker's catch path does serialise typed codes, and pins
the source facts behind each row of the table (the copy precedes the cwd
check in `importFromJsonl`; the existence check is its first statement;
`reuseWorkerForCreate` throws before `launchWorker` *and*
`acquireSessionLease` throws the same class from the worker's
`createRuntime`), so a re-pin cannot move a throw silently. Adding a reviewed pair requires adding its source
fact there; the test counts the rows.

**Why this is not brittle string matching.** The Issue #17 fallback trigger
("if the only possible implementation is matching `Daemon worker socket
closed`, re-open substrate selection") does not fire. The classifier's
inputs are the response's `success` flag and its typed `errorInfo` code,
both first-class wire fields; the message string is carried as evidence
and never consulted.

**The ledger.** `recordDispatch` writes `DISPATCHED` durably before the
envelope is written to the socket. Storage is a compare-and-swap: a
record is a directory `mutations/<commandId>/` of immutable versions
`v1.json, v2.json, ...`, and every write reads the highest version N,
derives the next state, and publishes `v(N+1).json` with an exclusive
`link(2)` create. A version that appears in between means another
Governor sharing the state directory won; the writer re-reads and
re-applies against the new state, where a transition that is no longer
legal is refused. Nothing is renamed over, unlinked or locked, so a stale
writer cannot put an old snapshot over a newer one (a resolved record
cannot become uncertain, and supersedable, again), a probe recorded while
another Governor resolved the record is appended to the resolved record,
of two conflicting resolutions exactly one is legal, and there is no stale
lock to reclaim. `ledger-cas.test.ts` stages each of those interleavings
deterministically through the `beforeCommit` seam and ends with the
negative control (the pre-review rename-in-place from a stale snapshot,
which loses the evidence); `ledger-race.test.ts` releases eight real child
processes on one record by a filesystem barrier and asserts one winner,
every probe kept, contiguous versions and no regression, and six real
adopters producing one adoption. The dispatcher's own outcome is never
discarded either: if an adopter marked its record uncertain first, a
success response resolves it (`effect_observed`, the response kept as a
probe), a typed pre-effect rejection resolves it the other way, and an
uncertain outcome is appended as a probe (`recordOutcome`). A command id
is dispatched once, ever;
`UNCERTAIN` leaves only through `resolveUncertain` with
`effect_observed` (→ COMPLETED) or `effect_absent_proven` (→ FAILED). A
human-issued replacement is a new command that must name the uncertain
record it supersedes, and is refused if that record is no longer
uncertain. `probeStoredResult` re-presents the same `clientId + commandId`
to fetch Prime's stored result; a stored untyped failure is still
UNCERTAIN, a stored success resolves the record from exact evidence.

`awaitingReconciliation()` is the attention surface: the UNCERTAIN records,
oldest first. There is no automatic consumer of it by design.

**The crash window.** A Governor that dies after writing DISPATCHED and
before recording a result leaves the record DISPATCHED, which is the
truth, but a surface that listed only UNCERTAIN would never show it. Every
record therefore carries `dispatchedBy`: the dispatching Governor's
`(ownerToken, pid, processStartId)`, the same identity a recovery lease
holder carries. `MutationLedger.adoptAbandoned()` classifies each
DISPATCHED record's dispatcher with the same verdicts the lease fence
uses: `gone` or `replaced` (pid reuse) adopts the record as UNCERTAIN with
reason `dispatcher_lost` and the verdict on the transition; `current` is a
live Governor sharing the state directory, whose in-flight record is left
alone so that its own completion remains a legal transition; `unknown` is
reported and left, never adopted; entries under `mutations/` that are
not record directories are listed as `strays` and never read. The
Governor runs adoption at
construction (`startupAdoption`) and `awaitingReconciliation()` runs it
before listing, so a record whose dispatcher is proven over is on the
surface whichever way a successor looks; an `unknown` dispatcher's record
is only in the adoption report's `undecidable` list (see *Known limits*). `conformance/tier1/governor-crash-recovery.test.ts` is a
real child-process Governor SIGKILLed after its DISPATCHED write: a
second Governor is fenced while the child lives, adopts once it is dead,
and probes under the original id, identity and command;
`ledger-adoption.test.ts` stages `gone`, `replaced`, `current`, `unknown`,
this process's own record, two concurrent adopters, and a record with no
dispatcher identity through the injectable process probe.

**The exact command.** Prime answers a repeated `clientId + commandId`
from its stored result only if the supervisor received the original. If
the Governor died before the envelope reached the socket there is no
receipt, and whatever the probe carries under the old id is admitted as
new work -- correctly, from Prime's side. The record therefore stores
`commandDigest`, the SHA-256 of the canonical JSON (keys sorted
recursively) of the COMPLETE wire command, and `command`, the command
itself less any field named `launchEnv` or `env` at any depth
(`withheld` lists the dotted paths removed, and no value under such a
field is ever written to the ledger). `probeStoredResult` refuses with
`command_mismatch` before any I/O unless the offered command's digest
equals the record's: a different body of the same type, a different
incarnation id, an added or removed field, or a different environment.
When the record holds the complete command the probe may omit it and the
stored command is re-presented verbatim, so nobody has to remember a
command across a restart; a record that withheld `launchEnv` must be
given the command again, and it must digest the same.

The classification policy is not injectable. `NAIVE_POLICY` exists for the
pure classifier tests and the D2 test's re-classification of a captured
response; a `Governor` always constructs with `DEFAULT_POLICY` and asserts
it is a production policy every time.

**Journal identity.** Prime's idempotency key is `clientId + commandId`.
The Governor's `clientId` is one per state directory
(`<stateDir>/client-identity.json`, `governor/prime/client-identity.ts`):
created atomically with an fsynced temp file published by `link(2)` (which
cannot replace an existing name) and a directory fsync, so two Governors
performing first initialisation at once converge on one id and no reader
ever sees a partial file; read back byte-for-byte on every restart; and
never overwritten -- a missing, unreadable or malformed file is a typed
error, not a reason to mint again.

The `MutationRecord` stores the `clientId` a command was dispatched under,
and that record is the authority when the command is probed.
`probeStoredResult` re-reads the identity file, and refuses with
`client_identity_mismatch` **before any socket I/O** unless the file, this
Governor's id and the live connection's id all equal the record's. A
Governor that restarted over a re-initialised state directory therefore
cannot re-present an old `commandId` under a new `clientId` (which Prime
would run as new work); the record stays UNCERTAIN for a human. A probe
must also carry the record's exact command (see *The exact command*
above). `conformance/tier1/governor-probe-identity.test.ts`
proves each refusal, identity and command, against a fake daemon socket
that records every byte it receives, and `client-identity.test.ts` races
eight processes through first initialisation.

**Regression tests.** `conformance/runtime/d2-worker-loss-uncertain.test.ts`
is the exact s1-07 (c) reproducer: effect on disk, SIGKILL the worker,
assert UNCERTAIN, same id, one record, no dispatch; reopen the root; probe
the stored failure (still UNCERTAIN, still one line); resolve from
evidence; then re-classify the captured response under `NAIVE_POLICY` and
assert it would have been FAILED.
`conformance/runtime/d2-import-jsonl-post-effect.test.ts` is the typed
post-effect reproducer against the pinned daemon: write a source JSONL
whose header `cwd` does not exist, dispatch `import_jsonl`, prove the copy
is in the session directory byte-for-byte, prove the response is typed
`missing_session_cwd`, prove the Governor recorded UNCERTAIN with
`typed_failure_post_effect`, and prove the same response under
`LEGACY_GLOBAL_CODE_POLICY` (the pre-review classifier) would have been
FAILED; its positive control imports a nonexistent source and gets the
reviewed pre-effect FAILED with nothing written. The runtime tier and the
`harness` CI job run both on every pull request.

**Upstream.** The worker-loss journal defect is filed as
[PrimeIntellect-ai/prime-agent discussion #1978](https://github.com/PrimeIntellect-ai/prime-agent/discussions/1978)
(issue #1974 was auto-closed by their contribution gate; Discussions are
the intake);
the proposal text is in
[`../upstream/2026-09-01-prime-worker-loss-journal.md`](../upstream/2026-09-01-prime-worker-loss-journal.md).
The Governor-side guard is sufficient on its own and does not depend on
it landing.

## D1 — resident-root recovery

**What the pinned supervisor does.** A resident root whose worker process
dies is marked `workerState: failed` with `lastError: "Waiting for a client
with fresh runtime context"` and is never relaunched, because
`recoverWorker` only has a recovery command for client-owned workers
(`ownerClientId`) and runtime config is deliberately not persisted. A
`create` on the same session path from any client reclaims the dead
registration (`reclaimStaleWorkerRegistration`, only for a confirmed-dead
process) and reopens the transcript under the same `sessionId` with a new
active-session id and a visible `prime-agent.worker_recovery` marker.

**What the Governor does.** `recoverResidentRoot(sessionId)`:

1. reads the summary; `workerState` decides, `activity` is ignored;
2. `ready` with the registry's current id → `healthy`; `ready` with a
   different id → someone else reopened, record it as `converged`;
3. `failed` → take the recovery lease for the session (an exclusive,
   durable create of `<sessionId>.json.recovery.lock` recording the
   holder's `(pid, processStartId)`; see below); re-check the summary under
   the lease;
4. dispatch exactly one `create` on the registry's canonical `sessionPath`
   through the same ledger as any mutation;
5. refuse to bind if the reopened `sessionId` differs from the logical one
   (`session_identity_mismatch`);
6. append the new incarnation and release the lease.

The fence is the Governor's because Prime's convergence hides the
duplicate: two naive recoverers both send `create`, Prime converges them,
and nothing records that two reopens were attempted. The runtime test
races two Governors over one state directory and asserts one `create` with
the fence and two without it.

## D8 — explicit session paths

Every Governor-created session, resident or client-owned, carries a
canonical persistent `sessionPath` from creation. `canonicalSessionPath`
applies Prime's own rule (`realpath` of the file, or of its directory plus
the basename) so the Governor and Prime's lease agree on identity, then
adds a fence Prime does not have: the path must lie inside the configured
session directory, must be absolute, must end in `.jsonl`, and its parent
must exist. There is no default and no fallback; omission is a typed
preflight error before any I/O and writes no ledger record.

The negative control reproduces Issue #15 D8 through the raw protocol,
bypassing the Governor: a client-owned worker created without a path
relaunches with an empty live transcript although its JSONL on disk holds
the turns, while the same worker created with a path resumes them.

## Environment boundary

Two edges, one allowlist (`DEFAULT_LAUNCH_ENV_ALLOWLIST`): the environment
the Governor gives the supervisor it spawns, and the `launchEnv` it puts on
the wire for `create`/`attach`. A variable crosses only by being on the
list or by an explicit per-name grant. A name-based denylist is rejected as
the mechanism, and the tests plant a sentinel whose name contains none of
TOKEN/SECRET/PASSWORD/KEY and prove from a worker's `env` that it did not
arrive while an explicitly granted control variable did. The wire evidence
log records `launchEnv` as key names only.

**Lease holder identity.** A pid is recycled, so a lease that recorded only
a pid could look live because an unrelated process inherited the number.
The lease records the holder's `(pid, processStartId)` -- the kernel start
time from `/proc/<pid>/stat` on Linux, `ps -o lstart=` on macOS, the same
pair Prime's own session lease and worker registry use -- and
`governor/process/identity.ts` classifies a holder as `current`,
`replaced` (alive, different start identity: pid reuse), `gone` (not
alive) or `unknown` (no recorded or no observable start identity;
"cannot signal" counts as alive). The lease is reclaimed, with the old
holder reported, **only** on `gone` or `replaced`; `current` and `unknown`
are honoured, and so is a lease file that is not a readable record.

**Reclaim is a compare-and-swap, not an unlink by name.** Inspecting a
holder may spawn `ps`, so two recoverers can both classify the same dead
lease, and a reclaimer that then deleted "the lease file" by name could
delete the live lease the other one had just published (demonstrated by the
independent review of 50762f4). The reclaim therefore happens under a
per-session reclaim mutex (`<sessionId>.json.recovery.reclaim`, exclusive
create) whose critical section spawns nothing: re-read the lease bytes,
proceed only if they are exactly the bytes that were classified dead,
replace them, release the mutex. A changed file means someone else acted
first; the reclaimer re-inspects instead of deleting. A fresh acquirer
needs no mutex, and if one takes the name while the reclaimer has it
absent, the reclaimer's exclusive create fails and it yields. The mutex
itself is never taken over: a live holder is contention, a dead one means a
Governor died inside a microsecond critical section, and both surface as
`recovery_reclaim_blocked` (the Governor returns `reclaim_blocked`, dispatching
nothing) until an operator who has confirmed the holder is gone removes the
file. A mutex that could be stolen would reintroduce the race it closes.

`conformance/tier1/registry.test.ts` stages pid reuse with this process's
own pid under a foreign start identity, each conservative branch with a
fabricated probe, the reclaim interleaving above (R1 reclaims inside R2's
classification window; R2 must not delete R1's lease), a fresh acquirer
winning the absent name, and a stale, a contended and an unreadable mutex.

## Durability contract

The Governor's invariant is *durable intent before external I/O*: the
ledger's DISPATCHED record exists before the envelope is written to the
socket, the registry's incarnation before the next fenced dispatch, the
recovery lease before the reopen is sent, and the client identity before
the first command. "Exists" means survives power loss, not only the death
of the Governor process, because the next Governor reads these files to
decide what may already have happened in the world.

A temp file + `fsync` + `rename` gives crash-atomic *contents* but not a
durable *name*: on ext4/xfs/btrfs and APFS the rename is a directory
operation, and an unsynced directory entry can be lost on power failure
while the file's bytes survive. Every authority-bearing write therefore
goes through `governor/fs/durable.ts`:

| helper | sequence | used for |
| --- | --- | --- |
| `writeFileDurable` | write temp → `fsync` temp → close → `rename` → `fsync` parent | session records |
| `createFileExclusiveDurable` | write temp → `fsync` temp → close → `link` (fails `EEXIST`, never replaces) → `fsync` parent → unlink temp; the `EEXIST` loser also `fsync`s the parent before it reads the winner (the winner may have died between its link and its own fsync); a name that vanishes between the `EEXIST` and the read is reported `vanished` for the caller to retry, never thrown | client identity, recovery lease, every ledger version |
| `unlinkDurable` | `unlink` → `fsync` parent | lease release; the dead lease inside a reclaim's critical section |
| `mkdirDurable` | for each missing component, top-down: `mkdir` → `fsync` ITS parent (the `EEXIST` loser fsyncs too) | the state directory, `mutations/`, each record directory, `sessions/` |

The exclusive create publishes an already-complete, already-fsynced file,
so no concurrent reader can observe an empty or partial identity or lease,
which an `O_EXCL` open followed by a write would allow. A failed directory
`fsync` is an error: a record whose name is not known to be durable is not
reported as written.

Two details the contract depends on and a naive helper gets wrong. A
`write(2)` may accept fewer bytes than asked; `writeAllSync` writes the
byte buffer in a loop until every byte is accepted and throws
`ShortWrite` (nothing published) if the kernel makes no progress, so a
truncated record can never be fsynced and renamed into place. And a
`mkdir(2)` creates an entry in the PARENT that is no more durable than a
`rename`'s: fsyncing `mutations/` after creating a record inside it makes
the record's name durable in `mutations/` and says nothing about whether
`mutations/` itself survived in the state directory, so every directory
the Governor creates is followed by an `fsync` of its parent.

What is relied on, per platform: on Linux, `fsync(2)` on the parent
directory fd makes the `rename`/`link`/`unlink` entry durable (the
documented ext4/xfs requirement, and the sequence SQLite and PostgreSQL
use). On macOS, Node's `fs.fsyncSync` reaches libuv `uv__fs_fsync`, which
issues `fcntl(F_FULLFSYNC)` (flush through the drive cache; plain
`fsync(2)` on macOS does not) and falls back to `fsync(2)` where that is
refused, e.g. on a directory fd. Nothing here is claimed for NFS.
`conformance/tier1/durable.test.ts` records the exact call sequence
through an instrumented `fs` and asserts the order and the failure
behaviour, since power loss itself cannot be staged in a test; it also
drives the writers through a kernel that accepts seven bytes per write,
one that stalls, and shows that the single-write helper this replaced
would have truncated the record.

## Known limits, recorded rather than discovered later

- **The fences are properties of the Governor's API, not of the process.**
  `DaemonClient` is exported and `Governor.client` is public, because the
  conformance suite needs the raw protocol for its negative controls. A
  caller that speaks to the socket directly bypasses D8 preflight, the
  ledger and the incarnation fence. Nothing automatic does so.
- **A dispatcher whose identity is `unknown` is never adopted.** A
  DISPATCHED record whose Governor's start identity was not readable at
  dispatch, or is not readable now, stays DISPATCHED and is listed in the
  adoption report's `undecidable`, not on the attention surface. Adopting
  it could make a live Governor's own completion an illegal transition;
  an operator who has confirmed the process is over resolves it. On macOS
  the start identity has one-second resolution, the same limit Prime's
  own lease accepts.
- **A write can report `contended`.** Each lost compare-and-swap attempt
  means another writer landed a version; after 1024 losses with jittered
  backoff the write throws `contended` and nothing is written. Reaching
  it needs that many other writes on ONE record while this one keeps
  losing; the independent review saw a worst case of 54 attempts with 32
  processes hammering one record before the backoff existed. Every
  version carries the whole record, so a record with thousands of
  versions costs quadratic bytes; writes per record are bounded by its
  lifecycle, not by anything here.
- **Readers trust names that writers made durable.** The create paths
  (`createFileExclusiveDurable`, `mkdirDurable`, including their losers
  and the already-exists case) fsync the parent before relying on a name.
  Plain reads (`#current`, `readClientIdentity`, `inspectRecoveryLease`)
  rely on that: the state directory and `mutations/` are fsynced at
  Governor construction and each record directory at every dispatch, so
  by the time a read happens the names it depends on have been confirmed
  by a writer in this or an earlier process.
- **Abandonment inside a live process is not adopted.** If the Governor
  process survives but its own dispatch path fails after DISPATCHED
  (the send throws something other than transport loss or timeout, or
  the result write itself fails, e.g. `ENOSPC`), the record stays
  DISPATCHED with a `current` dispatcher for the life of that process.
  The fence is process-level by design; the record is adopted by the
  next Governor once this one is over.
- **A `create` record cannot be re-presented from the ledger alone.**
  `launchEnv` is withheld from the stored command, so probing an uncertain
  `create` needs the command supplied again, and it must digest the same
  as the original -- including the environment. A changed `PATH` is a
  refusal, by design.
- **One state directory per fleet.** The recovery lease and the client
  identity (`client-identity.json`) both live in the state directory. Two
  Governors with different state directories are two clients to Prime:
  Prime converges their reopens, but each would ledger its own `create`,
  and neither can probe the other's UNCERTAIN records (the identity fence
  refuses, by design).
- **Only one `(command, code)` pair is proof** (`import_jsonl` +
  `session_import_file_not_found`). Every other typed
  failure is UNCERTAIN: the reviewed post-effect and ambiguous rows, and
  everything unreviewed, including `create` + `missing_session_cwd`, which
  is thrown in the worker after a session lease was taken. Widening the
  matrix is a source review of *every* throw site plus a pinned source-fact
  assertion, never a runtime observation.
- **A Governor that dies inside a reclaim's critical section blocks that
  session's recovery** until an operator removes
  `<sessionId>.json.recovery.reclaim`. The window is microseconds and
  spawns nothing; the trade is a blocked reopen for an impossible double one.
- **The substrate pin guard is proven by a fake daemon**
  (`conformance/tier1/substrate-pin.test.ts`): each wrong hello is refused
  with `SubstrateMismatch`, the socket is closed, and nothing is sent.
- **The pinned Prime is not itself durability-hardened.** The Governor's
  contract covers the Governor's records. Prime's own journal and lease
  files are written as Prime writes them.
- **An UNCERTAIN `create` leaves no registry record.** If the create
  succeeded and only the response was lost, Prime holds a session the
  registry does not know; a repeat create on the same path converges
  rather than duplicates, so recovery is a manual reopen, not a hazard.
- **`assertCurrent` and the send are two steps.** A reopen by another
  process between them is caught by Prime's own active-session addressing
  rather than by the Governor.
- **UNCERTAIN means unknown.** A worker killed before the effect and one
  killed after it produce the same record, on purpose. Nothing may infer
  which from the record.

## What this layer deliberately does not do

- It is not a resident Governor daemon. The reopen loop is a library call
  and a test; who calls it in production (a Prime schedule, a supervisor
  extension, an operator command) is a later decision that needs no new
  authority.
- It does not send `ack_result`. Acknowledgement lets Prime compact and
  re-admit the id; that is a receipt acknowledgement, not a foreman
  disposition, and the foreman ledger is Phase B.
- It does not choose a sandbox, a memory system, an ACP harness, or a
  ChatGPT transport (ADR 0009 gates S2-S6; `harness/authorities.json`
  records each as unassigned with its planned owner).

## Fidelity note

The Rust oracle encoded "durable intent before external I/O" and "one
loadout per incarnation" as types. Here they are checked: the ledger writes
before the socket, the registry refuses before the socket, and the
conformance suite falsifies each with a negative control. This weakening
was recorded in [`../pi-native/migration-notes.md`](../pi-native/migration-notes.md)
before the pivot and holds unchanged.
