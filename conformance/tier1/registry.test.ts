/**
 * D1 (pure) — the session registry: incarnations, the stale fence, and the
 * recovery lease with process identity, over a temp dir.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { currentProcessIdentity, type ProcessProbe } from "../../governor/process/identity.ts";
import { canonicalSessionPath } from "../../governor/session/paths.ts";
import {
	RecoveryLeaseHeld,
	type RecoveryLeaseRecord,
	RecoveryReclaimBlocked,
	SessionRegistry,
	type SessionRegistryOptions,
	StaleCursorError,
	StaleIncarnationError,
	UnknownSessionError,
} from "../../governor/session/registry.ts";

function fresh(options: SessionRegistryOptions = {}) {
	const stateDir = mkdtempSync(join(tmpdir(), "cg-registry-"));
	const sessionDir = join(stateDir, "sessions-dir");
	mkdirSync(sessionDir);
	return { registry: new SessionRegistry(stateDir, options), stateDir, sessionDir, path: (name: string) => canonicalSessionPath(join(sessionDir, name), sessionDir) };
}

const lease = (registry: SessionRegistry, sessionId: string, holder: Partial<RecoveryLeaseRecord> & { ownerToken: string; pid: number }) => {
	const path = join(registry.dir, `${sessionId}.json.recovery.lock`);
	writeFileSync(path, `${JSON.stringify({ sessionId, acquiredAt: "2026-01-01T00:00:00Z", ...holder })}\n`);
	return path;
};

describe("SessionRegistry (D1)", () => {
	it("keys by sessionId, binds a path once, and records incarnations in order", () => {
		const { registry, path } = fresh();
		const p = path("a.jsonl");
		const record = registry.create({ sessionId: "sid-a", sessionPath: p, lifecycle: "resident", activeSessionId: "act-1", workerPid: 1, openedBy: "o" });
		assert.equal(record.incarnations.length, 1);
		assert.throws(() => registry.create({ sessionId: "sid-a", sessionPath: path("b.jsonl"), lifecycle: "resident", activeSessionId: "x", openedBy: "o" }), /already has a registry record/);
		assert.throws(() => registry.create({ sessionId: "sid-b", sessionPath: p, lifecycle: "resident", activeSessionId: "x", openedBy: "o" }), /already bound to session sid-a/);
		const reopen = registry.recordIncarnation({ sessionId: "sid-a", activeSessionId: "act-2", workerPid: 2, cause: "reopen", openedBy: "o" });
		assert.equal(reopen.appended, true);
		assert.equal(reopen.incarnation.index, 1);
		const again = registry.recordIncarnation({ sessionId: "sid-a", activeSessionId: "act-2", cause: "converged", openedBy: "p" });
		assert.equal(again.appended, false, "a second observer of the same incarnation converges");
		assert.equal(registry.require("sid-a").incarnations.length, 2);
		assert.equal(registry.findByPath(p)?.sessionId, "sid-a");
	});

	it("the stale fence: only the current incarnation passes", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		registry.recordIncarnation({ sessionId: "s", activeSessionId: "act-2", cause: "reopen", openedBy: "o" });
		assert.equal(registry.assertCurrent("s", "act-2").index, 1);
		assert.throws(() => registry.assertCurrent("s", "act-1"), (e: unknown) => e instanceof StaleIncarnationError && e.presented === "act-1" && e.current === "act-2");
		assert.throws(() => registry.assertCurrent("s", "never"), StaleIncarnationError);
		assert.throws(() => registry.assertCurrent("unknown", "act-2"), UnknownSessionError);
	});

	it("the cursor fence: a generation binds to one incarnation once, and only the current one passes", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		assert.throws(() => registry.assertCurrentGeneration("s", "gen-1"), (e: unknown) => e instanceof StaleCursorError && e.current === undefined, "nothing observed yet: refuse");
		assert.equal(registry.recordGeneration("s", "act-1", "gen-1").generation, "gen-1");
		assert.equal(registry.recordGeneration("s", "act-1", "gen-1").generation, "gen-1", "idempotent");
		assert.throws(() => registry.recordGeneration("s", "act-1", "gen-other"), /refusing to rebind/);
		assert.throws(() => registry.recordGeneration("s", "act-never", "gen-1"), StaleIncarnationError);
		assert.equal(registry.assertCurrentGeneration("s", "gen-1").activeSessionId, "act-1");
		registry.recordIncarnation({ sessionId: "s", activeSessionId: "act-2", cause: "reopen", openedBy: "o" });
		registry.recordGeneration("s", "act-2", "gen-2");
		assert.throws(() => registry.assertCurrentGeneration("s", "gen-1"), (e: unknown) => e instanceof StaleCursorError && e.presented === "gen-1" && e.current === "gen-2");
		assert.equal(registry.assertCurrentGeneration("s", "gen-2").index, 1);
	});

	it("the recovery lease: exclusive while held, records the holder's process identity, released durably", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		const held = registry.acquireRecoveryLease("s", "owner-a");
		assert.equal(held.record.pid, process.pid);
		assert.equal(held.record.processStartId, currentProcessIdentity().processStartId, "the lease carries the holder's start identity");
		const onDisk = JSON.parse(readFileSync(join(registry.dir, "s.json.recovery.lock"), "utf8")) as RecoveryLeaseRecord;
		assert.equal(onDisk.processStartId, held.record.processStartId);
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-b"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holder.ownerToken === "owner-a" && e.holderIdentity === "current");
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-a"), RecoveryLeaseHeld, "not reentrant");
		held.release();
		assert.equal(existsSync(join(registry.dir, "s.json.recovery.lock")), false);
		const second = registry.acquireRecoveryLease("s", "owner-b");
		assert.equal(second.reclaimedFrom, undefined);
		second.release();
	});

	it("reclaims only from a holder whose process is proven over: gone, or replaced by pid reuse", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		// gone: the pid does not exist.
		lease(registry, "s", { ownerToken: "ghost", pid: 2147483646, processStartId: "ps:whenever" });
		const fromGhost = registry.acquireRecoveryLease("s", "owner-c");
		assert.equal(fromGhost.reclaimedFrom?.ownerToken, "ghost");
		assert.equal(fromGhost.reclaimedBecause, "gone");
		fromGhost.release();
		// replaced: our own live pid, but a start identity that is not ours -- exactly what pid reuse looks like.
		lease(registry, "s", { ownerToken: "previous-life", pid: process.pid, processStartId: "ps:Thu Jan  1 00:00:00 1970" });
		const fromReused = registry.acquireRecoveryLease("s", "owner-d");
		assert.equal(fromReused.reclaimedFrom?.ownerToken, "previous-life");
		assert.equal(fromReused.reclaimedBecause, "replaced");
		fromReused.release();
	});

	it("never reclaims on current or unknown: a live holder, a holder without a start id, an unreadable lease", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		// current: our own pid with our own start identity, under a foreign token.
		lease(registry, "s", { ownerToken: "other-process", pid: process.pid, processStartId: currentProcessIdentity().processStartId });
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-e"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holderIdentity === "current");
		// unknown: a live pid with no recorded start identity (a pre-identity or fabricated lease).
		lease(registry, "s", { ownerToken: "pid-only", pid: process.pid });
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-f"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holderIdentity === "unknown");
		// unknown: not a lease record at all.
		writeFileSync(join(registry.dir, "s.json.recovery.lock"), "garbage\n");
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-g"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holderIdentity === "unknown");
		writeFileSync(join(registry.dir, "s.json.recovery.lock"), JSON.stringify({ sessionId: "s", ownerToken: 7 }));
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-h"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holderIdentity === "unknown");
	});

	it("reclaim is a compare-and-swap: a reclaimer that inspected a dead lease cannot delete the lease another reclaimer published meanwhile", () => {
		// The interleaving the independent review of 50762f4 demonstrated: R1 and R2 both classify the dead lease;
		// R1 completes its reclaim inside R2's window; the old code then unlinked R1's live lease by name.
		const startIds = new Map<number, string | undefined>([[1001, "s1"], [1002, "s2"]]);
		let interleaved = false;
		const probe: ProcessProbe = {
			alive: (pid) => pid !== 2147483646,
			startId: (pid) => startIds.get(pid),
		};
		const { registry: r1, stateDir, path } = fresh({ processProbe: probe, self: { pid: 1001, processStartId: "s1" } });
		r1.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		const r2 = new SessionRegistry(stateDir, {
			self: { pid: 1002, processStartId: "s2" },
			processProbe: {
				alive: (pid) => {
					// R2 is classifying the dead holder (the liveness probe runs first); R1 reclaims and publishes right here.
					if (!interleaved && pid === 2147483646) {
						interleaved = true;
						const taken = r1.acquireRecoveryLease("s", "R1");
						assert.equal(taken.reclaimedFrom?.ownerToken, "ghost");
					}
					return probe.alive(pid);
				},
				startId: probe.startId,
			},
		});
		lease(r1, "s", { ownerToken: "ghost", pid: 2147483646, processStartId: "gone" });
		assert.throws(() => r2.acquireRecoveryLease("s", "R2"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holder.ownerToken === "R1" && e.holderIdentity === "current");
		assert.equal(interleaved, true, "the interleaving happened");
		const onDisk = JSON.parse(readFileSync(join(r1.dir, "s.json.recovery.lock"), "utf8")) as RecoveryLeaseRecord;
		assert.equal(onDisk.ownerToken, "R1", "R1 still holds; R2 deleted nothing");
		assert.equal(existsSync(join(r1.dir, "s.json.recovery.reclaim")), false, "the reclaim mutex was released");
	});

	it("the reclaim mutex is never taken over: contention, a stale mutex and an unreadable one all block, and nothing is dispatched", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		const mutex = join(registry.dir, "s.json.recovery.reclaim");
		lease(registry, "s", { ownerToken: "ghost", pid: 2147483646, processStartId: "gone" });
		// Stale: a Governor died inside the critical section.
		writeFileSync(mutex, `${JSON.stringify({ sessionId: "s", ownerToken: "crashed", pid: 2147483646, processStartId: "x", acquiredAt: "2026-01-01T00:00:00Z", stage: "reclaim" })}\n`);
		assert.throws(() => registry.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryReclaimBlocked && e.holder.ownerToken === "crashed" && e.holderIdentity === "gone");
		assert.equal(existsSync(mutex), true, "a stale mutex is reported, not removed");
		// Contention: a live reclaimer.
		writeFileSync(mutex, `${JSON.stringify({ sessionId: "s", ownerToken: "busy", pid: process.pid, processStartId: currentProcessIdentity().processStartId, acquiredAt: "2026-01-01T00:00:00Z" })}\n`);
		assert.throws(() => registry.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryReclaimBlocked && e.holderIdentity === "current");
		// Unreadable.
		writeFileSync(mutex, "garbage");
		assert.throws(() => registry.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryReclaimBlocked && e.holderIdentity === "unknown");
		const ghost = JSON.parse(readFileSync(join(registry.dir, "s.json.recovery.lock"), "utf8")) as RecoveryLeaseRecord;
		assert.equal(ghost.ownerToken, "ghost", "the dead lease was not touched while the mutex was blocked");
		// Operator clears the mutex: the reclaim proceeds.
		rmSync(mutex);
		const taken = registry.acquireRecoveryLease("s", "me");
		assert.equal(taken.reclaimedFrom?.ownerToken, "ghost");
		taken.release();
	});

	it("a fresh acquirer that takes the name while a reclaimer has it absent wins; the reclaimer re-inspects and yields", () => {
		const startIds = new Map<number, string | undefined>([[1001, "s1"]]);
		const probe: ProcessProbe = { alive: (pid) => pid !== 2147483646, startId: (pid) => startIds.get(pid) };
		const { registry, path } = fresh({ processProbe: probe, self: { pid: 1002, processStartId: "s2" } });
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		lease(registry, "s", { ownerToken: "ghost", pid: 2147483646, processStartId: "gone" });
		// Stage: the dead lease is replaced by a fresh live holder between inspection and the critical section.
		let staged = false;
		const staging = new SessionRegistry(registry.dir.replace(/\/sessions$/, ""), {
			self: { pid: 1002, processStartId: "s2" },
			processProbe: {
				alive: (pid) => {
					if (!staged && pid === 2147483646) {
						staged = true;
						rmSync(join(registry.dir, "s.json.recovery.lock"));
						lease(registry, "s", { ownerToken: "fresh", pid: 1001, processStartId: "s1" });
					}
					return probe.alive(pid);
				},
				startId: probe.startId,
			},
		});
		assert.throws(() => staging.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holder.ownerToken === "fresh" && e.holderIdentity === "current");
	});

	it("exhaustion under constant churn reports the real holder, or 'contended' when nobody holds -- never the caller's own record", () => {
		const probe: ProcessProbe = { alive: (pid) => pid !== 2147483646, startId: () => undefined };
		const { registry, stateDir, path } = fresh({ processProbe: probe, self: { pid: 1002, processStartId: "s2" } });
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		let looks = 0;
		const churning = new SessionRegistry(stateDir, {
			self: { pid: 1002, processStartId: "s2" },
			processProbe: {
				alive: (pid) => {
					if (pid === 2147483646) {
						looks += 1;
						// Every attempt: the dead lease is swapped for a DIFFERENT dead lease after inspection, so the CAS sees changed bytes.
						rmSync(join(registry.dir, "s.json.recovery.lock"));
						lease(registry, "s", { ownerToken: `ghost-${looks}`, pid: 2147483646, processStartId: "gone" });
					}
					return probe.alive(pid);
				},
				startId: probe.startId,
			},
		});
		lease(registry, "s", { ownerToken: "ghost-0", pid: 2147483646, processStartId: "gone" });
		// Final look finds the last planted lease: RecoveryLeaseHeld names it, not the caller.
		assert.throws(() => churning.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryLeaseHeld && /^ghost-\d+$/.test(e.holder.ownerToken) && e.holder.ownerToken !== "me");
		assert.ok(looks >= 4, `all attempts were spent (${looks})`);
		// Same churn, but the dead holder vanishes for good during the last attempt: the name is free, so the
		// acquirer simply takes it, and reports a plain acquisition rather than a reclaim of anybody.
		looks = 0;
		rmSync(join(registry.dir, "s.json.recovery.lock"));
		lease(registry, "s", { ownerToken: "ghost-0", pid: 2147483646, processStartId: "gone" });
		const freedAtEnd = new SessionRegistry(stateDir, {
			self: { pid: 1002, processStartId: "s2" },
			processProbe: {
				alive: (pid) => {
					if (pid === 2147483646) {
						looks += 1;
						rmSync(join(registry.dir, "s.json.recovery.lock"));
						if (looks < 4) lease(registry, "s", { ownerToken: `ghost-${looks}`, pid: 2147483646, processStartId: "gone" });
					}
					return probe.alive(pid);
				},
				startId: probe.startId,
			},
		});
		const taken = freedAtEnd.acquireRecoveryLease("s", "me");
		assert.equal(taken.reclaimedFrom, undefined, "nothing was reclaimed: the name was free when the critical section looked");
		assert.equal(taken.record.ownerToken, "me");
		taken.release();
		// RecoveryLeaseContended (every attempt changed under us AND the name is absent at the final look) needs a
		// contender that acquires and releases inside a single attempt's window; it is not reachable through the
		// probe seam and is covered by the reviewer's multi-process stress rather than staged here.
	});

	it("with a fabricated probe: a live pid whose start id cannot be read is unknown and honoured; a mismatch is replaced and reclaimed", () => {
		const startIds = new Map<number, string | undefined>();
		const probe: ProcessProbe = { alive: (pid) => pid !== 2147483646, startId: (pid) => startIds.get(pid) };
		const { registry, path } = fresh({ processProbe: probe, self: { pid: 4242, processStartId: "fake:self" } });
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		lease(registry, "s", { ownerToken: "holder", pid: 99, processStartId: "fake:99-first" });
		startIds.set(99, undefined);
		assert.throws(() => registry.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holderIdentity === "unknown");
		startIds.set(99, "fake:99-first");
		assert.throws(() => registry.acquireRecoveryLease("s", "me"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holderIdentity === "current");
		startIds.set(99, "fake:99-second");
		const taken = registry.acquireRecoveryLease("s", "me");
		assert.equal(taken.reclaimedBecause, "replaced");
		assert.equal(taken.record.pid, 4242);
		assert.equal(taken.record.processStartId, "fake:self", "the new lease carries the configured self identity");
		taken.release();
	});
});
