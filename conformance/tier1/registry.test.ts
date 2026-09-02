/**
 * D1 (pure) — the session registry: incarnations, the stale fence, and the
 * recovery lease with process identity, over a temp dir.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { currentProcessIdentity, type ProcessProbe } from "../../governor/process/identity.ts";
import { canonicalSessionPath } from "../../governor/session/paths.ts";
import { RecoveryLeaseHeld, type RecoveryLeaseRecord, SessionRegistry, type SessionRegistryOptions, StaleCursorError, StaleIncarnationError, UnknownSessionError } from "../../governor/session/registry.ts";

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
