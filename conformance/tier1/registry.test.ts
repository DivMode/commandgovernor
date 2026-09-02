/**
 * D1 (pure) — the session registry: incarnations, the stale fence, and the
 * recovery lease, over a temp dir.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { canonicalSessionPath } from "../../governor/session/paths.ts";
import { RecoveryLeaseHeld, SessionRegistry, StaleIncarnationError, UnknownSessionError } from "../../governor/session/registry.ts";

function fresh() {
	const stateDir = mkdtempSync(join(tmpdir(), "cg-registry-"));
	const sessionDir = join(stateDir, "sessions-dir");
	mkdirSync(sessionDir);
	return { registry: new SessionRegistry(stateDir), stateDir, sessionDir, path: (name: string) => canonicalSessionPath(join(sessionDir, name), sessionDir) };
}

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

	it("the recovery lease: exclusive while held, released on release, reclaimed only from a dead holder", () => {
		const { registry, path } = fresh();
		registry.create({ sessionId: "s", sessionPath: path("s.jsonl"), lifecycle: "resident", activeSessionId: "act-1", openedBy: "o" });
		const lease = registry.acquireRecoveryLease("s", "owner-a");
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-b"), (e: unknown) => e instanceof RecoveryLeaseHeld && e.holder.ownerToken === "owner-a");
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-a"), RecoveryLeaseHeld, "not reentrant");
		lease.release();
		const second = registry.acquireRecoveryLease("s", "owner-b");
		assert.equal(second.reclaimedFrom, undefined);
		second.release();
		// A holder whose pid is gone is reclaimed, with the dead holder reported.
		writeFileSync(join(registry.dir, "s.json.recovery.lock"), `${JSON.stringify({ sessionId: "s", ownerToken: "ghost", pid: 2147483646, acquiredAt: "2026-01-01T00:00:00Z" })}\n`);
		const reclaimed = registry.acquireRecoveryLease("s", "owner-c");
		assert.equal(reclaimed.reclaimedFrom?.ownerToken, "ghost");
		reclaimed.release();
		// A holder whose pid is alive (ours) is honoured even under a foreign token.
		writeFileSync(join(registry.dir, "s.json.recovery.lock"), `${JSON.stringify({ sessionId: "s", ownerToken: "other-process", pid: process.pid, acquiredAt: "2026-01-01T00:00:00Z" })}\n`);
		assert.throws(() => registry.acquireRecoveryLease("s", "owner-d"), RecoveryLeaseHeld);
	});
});
