/**
 * D2 (pure) — a DISPATCHED record whose Governor process is proven over is
 * adopted as UNCERTAIN; one whose process is live, or cannot be told, is
 * left alone. Process identity is fabricated through the injectable probe,
 * exactly as the registry's lease tests do, so pid reuse can be staged. The
 * real crash is `governor-crash-recovery.test.ts`.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { MutationLedger, type MutationLedgerOptions } from "../../governor/mutation/ledger.ts";
import type { ProcessProbe } from "../../governor/process/identity.ts";

const command = { type: "execute_bash_and_wait", activeSessionId: "a", command: "true" };
const dispatch = (ledger: MutationLedger, commandId: string) => ledger.recordDispatch({ commandId, clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });

/** A world in which only the listed pids are alive, each with the given start id. */
function world(alive: Record<number, string | undefined>): ProcessProbe {
	return {
		alive: (pid) => pid in alive,
		startId: (pid) => alive[pid],
	};
}

function ledgerAs(dir: string, pid: number, startId: string | undefined, probe: ProcessProbe, ownerToken = `owner#${pid}`): MutationLedger {
	const options: MutationLedgerOptions = { processProbe: probe, self: startId === undefined ? { pid } : { pid, processStartId: startId }, ownerToken };
	return new MutationLedger(dir, options);
}

describe("D2: adoption of abandoned DISPATCHED records", () => {
	it("a record whose dispatcher is gone becomes UNCERTAIN (dispatcher_lost) and joins the attention surface", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		const dead = ledgerAs(dir, 1001, "start:1001", world({ 1001: "start:1001" }));
		dispatch(dead, "cg-a");
		// Governor 1001 dies. Governor 2002 starts over the same state dir.
		const successor = ledgerAs(dir, 2002, "start:2002", world({ 2002: "start:2002" }));
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.adopted.map((r) => r.commandId), ["cg-a"]);
		assert.deepEqual(report.inFlight, []);
		assert.deepEqual(report.undecidable, []);
		const record = successor.require("cg-a");
		assert.equal(record.state, "UNCERTAIN");
		const last = record.transitions[record.transitions.length - 1]!;
		assert.equal(last.uncertainReason, "dispatcher_lost");
		assert.equal(last.adoption?.verdict, "gone");
		assert.equal(last.adoption?.dispatcher.pid, 1001);
		assert.equal(last.adoption?.adoptedBy.pid, 2002);
		assert.deepEqual(successor.awaitingReconciliation().map((r) => r.commandId), ["cg-a"]);
		// Adoption is once: a second pass finds nothing DISPATCHED.
		assert.deepEqual(successor.adoptAbandoned().adopted, []);
	});

	it("a recycled pid does not keep a dead dispatcher's record in flight", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		const dead = ledgerAs(dir, 1001, "start:old", world({ 1001: "start:old" }));
		dispatch(dead, "cg-b");
		// pid 1001 is alive again, but it is a different process.
		const successor = ledgerAs(dir, 2002, "start:2002", world({ 1001: "start:new", 2002: "start:2002" }));
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.adopted.map((r) => r.commandId), ["cg-b"]);
		const record = successor.require("cg-b");
		assert.equal(record.transitions[record.transitions.length - 1]!.adoption?.verdict, "replaced");
	});

	it("a recycled pid with DEFAULT owner tokens is still adopted: the token shortcut cannot mask the identity verdict", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		const dead = new MutationLedger(dir, { processProbe: world({ 5000: "start:old" }), self: { pid: 5000, processStartId: "start:old" } });
		dispatch(dead, "cg-reuse");
		const successor = new MutationLedger(dir, { processProbe: world({ 5000: "start:new" }), self: { pid: 5000, processStartId: "start:new" } });
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.adopted.map((r) => r.commandId), ["cg-reuse"]);
		assert.equal(successor.require("cg-reuse").state, "UNCERTAIN");
	});

	it("a live dispatcher's record is fenced: not adopted, not on the attention surface, and its own completion still lands", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		const probe = world({ 1001: "start:1001", 2002: "start:2002" });
		const live = ledgerAs(dir, 1001, "start:1001", probe);
		dispatch(live, "cg-c");
		const other = ledgerAs(dir, 2002, "start:2002", probe);
		const report = other.adoptAbandoned();
		assert.deepEqual(report.adopted, []);
		assert.deepEqual(report.inFlight.map((r) => r.commandId), ["cg-c"]);
		assert.deepEqual(other.awaitingReconciliation(), [], "an in-flight record is not an obligation yet");
		assert.equal(other.require("cg-c").state, "DISPATCHED");
		// The live Governor's response arrives; the transition is still legal.
		assert.equal(live.markCompleted("cg-c", { type: "response", command: "x", success: true }).state, "COMPLETED");
	});

	it("a dispatcher whose identity cannot be told is reported, never adopted", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		// Recorded without a start id (it could not be read at dispatch), pid alive: unknown.
		const noStart = ledgerAs(dir, 1001, undefined, world({ 1001: undefined }));
		dispatch(noStart, "cg-d");
		// Recorded with a start id, but the probe cannot read one now: unknown.
		const unreadable = ledgerAs(dir, 1002, "start:1002", world({ 1002: "start:1002" }));
		dispatch(unreadable, "cg-e");
		const successor = ledgerAs(dir, 2002, "start:2002", world({ 1001: undefined, 1002: undefined, 2002: "start:2002" }));
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.adopted, []);
		assert.deepEqual(report.undecidable.map((u) => [u.record.commandId, u.verdict]), [["cg-d", "unknown"], ["cg-e", "unknown"]]);
		assert.equal(successor.require("cg-d").state, "DISPATCHED");
		assert.equal(successor.require("cg-e").state, "DISPATCHED");
		assert.deepEqual(successor.awaitingReconciliation(), []);
	});

	it("this process's own in-flight record is never inspected, even under a probe that would call it dead", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		const ledger = ledgerAs(dir, 1001, "start:1001", world({}), "me");
		dispatch(ledger, "cg-f");
		const report = ledger.adoptAbandoned();
		assert.deepEqual(report.inFlight.map((r) => r.commandId), ["cg-f"]);
		assert.deepEqual(report.adopted, []);
		assert.equal(ledger.require("cg-f").state, "DISPATCHED");
	});

	it("two successors adopting the same record: exactly one adoption transition is written", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		dispatch(ledgerAs(dir, 1001, "start:1001", world({ 1001: "start:1001" })), "cg-g");
		const probe = world({ 2002: "start:2002", 3003: "start:3003" });
		const s1 = ledgerAs(dir, 2002, "start:2002", probe);
		const s2 = ledgerAs(dir, 3003, "start:3003", probe);
		const r1 = s1.adoptAbandoned();
		const r2 = s2.adoptAbandoned();
		assert.equal(r1.adopted.length + r2.adopted.length, 1);
		const record = s1.require("cg-g");
		assert.equal(record.state, "UNCERTAIN");
		assert.equal(record.transitions.filter((t) => t.adoption).length, 1);
	});

	it("a record with no dispatcher identity at all (an older schema) is undecidable, not adopted", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-adopt-"));
		const ledger = ledgerAs(dir, 1001, "start:1001", world({ 1001: "start:1001" }));
		const record = dispatch(ledger, "cg-h");
		const { dispatchedBy: _dropped, ...legacy } = record;
		writeFileSync(ledger.currentVersionPath("cg-h"), `${JSON.stringify(legacy, null, 2)}\n`);
		const successor = ledgerAs(dir, 2002, "start:2002", world({ 2002: "start:2002" }));
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.adopted, []);
		assert.deepEqual(report.undecidable.map((u) => u.record.commandId), ["cg-h"]);
	});
});
