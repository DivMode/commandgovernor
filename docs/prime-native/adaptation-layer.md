# The Prime adaptation layer (Issue #17)

What `governor/` adds on top of the pinned Prime Agent substrate, why each
piece exists, and how the three Issue #15 blockers are closed. The layer is
small by design: Prime's supervisor, workers, leases, journals, schedules,
RLM children, sessions and ACP are used as they are (ADR 0009 §3). The
Governor adds durable records Prime lacks and the fences Prime's semantics
leave to the client.

```text
Command Governor
  governor/governor.ts            the one object that speaks to a supervisor
  governor/mutation/classify.ts   D2: structural outcome classification
  governor/mutation/ledger.ts     D2: durable intent, one id forever
  governor/session/registry.ts    D1: logical sessions, incarnations, lease
  governor/session/paths.ts       D8: canonical, fenced sessionPath
  governor/prime/env.ts           positive environment allowlist
  governor/prime/daemon-client.ts JSONL client with the pin guard
  governor/prime/protocol.ts      the narrow protocol slice, diffed vs the pin
  governor/prime/substrate.ts     pins.json reader, supervisor spawn/stop
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
produced only by the supervisor itself during session resolution, before a
worker could have received the command (`daemon-errors.ts`
`serializeDaemonError` is their sole producer). So:

| observation | verdict |
| --- | --- |
| `success: true` | COMPLETED |
| `errorInfo.code` in the pre-effect set | FAILED, with the code as proof |
| `errorInfo.code = command_result_uncertain` | UNCERTAIN (substrate says so) |
| any other `success: false` (untyped, or an unknown code) | UNCERTAIN |
| socket lost, write failed, timeout | UNCERTAIN |

No error text is read. A future Prime that rewords the message, or adds a
new typed code the Governor has not reviewed, lands in UNCERTAIN, not
FAILED: the guard fails closed by construction. `assertProductionPolicy`
refuses any policy that maps untyped failure or transport loss to FAILED,
and `conformance/tier1/prime-protocol.test.ts` diffs the vocabulary and the
read/mutating command split against the pinned build so a re-pin cannot
widen either silently.

**Why this is not brittle string matching.** The Issue #17 fallback trigger
("if the only possible implementation is matching `Daemon worker socket
closed`, re-open substrate selection") does not fire. The classifier's
inputs are the response's `success` flag and its typed `errorInfo` code,
both first-class wire fields; the message string is carried as evidence
and never consulted.

**The ledger.** `recordDispatch` writes `DISPATCHED` durably before the
envelope is written to the socket. A command id is dispatched once, ever;
`UNCERTAIN` leaves only through `resolveUncertain` with
`effect_observed` (→ COMPLETED) or `effect_absent_proven` (→ FAILED). A
human-issued replacement is a new command that must name the uncertain
record it supersedes, and is refused if that record is no longer
uncertain. `probeStoredResult` re-presents the same `clientId + commandId`
to fetch Prime's stored result; a stored untyped failure is still
UNCERTAIN, a stored success resolves the record from exact evidence.

`awaitingReconciliation()` is the attention surface: the UNCERTAIN records,
oldest first. There is no automatic consumer of it by design.

The classification policy is not injectable. `NAIVE_POLICY` exists for the
pure classifier tests and the D2 test's re-classification of a captured
response; a `Governor` always constructs with `DEFAULT_POLICY` and asserts
it is a production policy every time.

The Governor's `clientId` is stable per state directory (`<stateDir>/client-id`),
because Prime's idempotency key is `clientId + commandId`; a Governor that
minted a fresh client id per process would turn every probe into new work.

**Regression test.** `conformance/runtime/d2-worker-loss-uncertain.test.ts`
is the exact s1-07 (c) reproducer: effect on disk, SIGKILL the worker,
assert UNCERTAIN, same id, one record, no dispatch; reopen the root; probe
the stored failure (still UNCERTAIN, still one line); resolve from
evidence; then re-classify the captured response under `NAIVE_POLICY` and
assert it would have been FAILED. The runtime tier and the `harness` CI job
run it on every pull request.

**Upstream.** A focused fix proposal is drafted in
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
3. `failed` → take the recovery lease for the session (`O_EXCL` file; a
   dead holder is reclaimed with a report, a live or unknowable holder is
   honoured); re-check the summary under the lease;
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

## Known limits, recorded rather than discovered later

- **The fences are properties of the Governor's API, not of the process.**
  `DaemonClient` is exported and `Governor.client` is public, because the
  conformance suite needs the raw protocol for its negative controls. A
  caller that speaks to the socket directly bypasses D8 preflight, the
  ledger and the incarnation fence. Nothing automatic does so.
- **One state directory per fleet.** The recovery lease and the stable
  client id both live in the state directory. Two Governors with different
  state directories are two clients to Prime: Prime converges their
  reopens, but each would ledger its own `create`.
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
