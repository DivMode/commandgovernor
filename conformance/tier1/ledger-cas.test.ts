/**
 * D2 (pure) — every ledger write is a compare-and-swap on the record's
 * version, so a stale writer can neither regress the state nor overwrite
 * evidence written by another Governor sharing the state directory.
 *
 * The interleavings are staged deterministically with the `beforeCommit`
 * seam: a competing ledger writes between this ledger's read and its
 * publish, which is the window the foreman's finding names. The negative
 * control at the end performs the pre-review write (a rename over the
 * current file) from a stale snapshot and shows the evidence vanish, so the
 * assertions above are known to be able to fail. The real multi-process race
 * is `ledger-race.test.ts`.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { writeFileDurable } from "../../governor/fs/durable.ts";
import { MAX_CAS_ATTEMPTS, MutationLedger, MutationLedgerError, type MutationLedgerHooks } from "../../governor/mutation/ledger.ts";
import type { DaemonResponse } from "../../governor/prime/protocol.ts";

const command = { type: "execute_bash_and_wait", activeSessionId: "a", command: "true" };
const stored: DaemonResponse = { type: "response", command: "x", success: false, error: "Daemon worker socket closed" };
const observed = { kind: "effect_observed" as const, by: "operator A", detail: "the line is on disk", observedAt: "t1" };
const absent = { kind: "effect_absent_proven" as const, by: "operator B", detail: "no line on disk", observedAt: "t2" };

/** An UNCERTAIN record in a fresh dir, and a ledger over it. */
function uncertain(hooks?: MutationLedgerHooks): { dir: string; ledger: MutationLedger } {
	const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
	const ledger = new MutationLedger(dir, { hooks });
	ledger.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
	ledger.markUncertain("cg-1", "transport_lost");
	return { dir, ledger };
}

describe("D2: ledger writes are compare-and-swap", () => {
	it("the foreman's interleaving: B's stale probe lands ON A's resolution, not over it", () => {
		// B reads UNCERTAIN (v2) for recordProbe; A resolves with exact evidence (v3); B publishes.
		let competed = false;
		const B = { ledger: undefined as MutationLedger | undefined };
		const { dir, ledger: A } = uncertain();
		B.ledger = new MutationLedger(dir, {
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					A.resolveUncertain("cg-1", observed);
				},
			},
		});
		const result = B.ledger.recordProbe("cg-1", { response: stored });
		assert.equal(result.state, "COMPLETED", "B's write was re-applied on top of A's resolution");
		assert.equal(result.version, 4);
		assert.equal(result.probes?.length, 1, "the probe was kept");
		const final = A.require("cg-1");
		assert.equal(final.state, "COMPLETED", "the exact evidence survived");
		assert.equal(final.transitions[final.transitions.length - 1]!.evidence?.kind, "effect_observed");
		assert.deepEqual(A.history("cg-1").map((v) => v.state), ["DISPATCHED", "UNCERTAIN", "COMPLETED", "COMPLETED"]);
	});

	it("two conflicting resolutions: exactly one is legal, and the loser is refused rather than last-writer-wins", () => {
		let competed = false;
		const { dir, ledger: A } = uncertain();
		const B = new MutationLedger(dir, {
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					A.resolveUncertain("cg-1", observed); // A: COMPLETED
				},
			},
		});
		assert.throws(() => B.resolveUncertain("cg-1", absent), (e: unknown) => e instanceof MutationLedgerError && e.code === "illegal_transition" && /COMPLETED -> FAILED/.test(e.message));
		assert.equal(A.require("cg-1").state, "COMPLETED");
		assert.equal(A.require("cg-1").version, 3, "B wrote nothing");
	});

	it("two adopters of one abandoned record: one adoption, and the loser's retry is refused", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const dead = new MutationLedger(dir, { processProbe: probe, self: { pid: 1001, processStartId: "start:1001" } });
		dead.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		const s1 = new MutationLedger(dir, { processProbe: probe, self: { pid: 2002, processStartId: "start:2002" } });
		let competed = false;
		const s2 = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 3003, processStartId: "start:3003" },
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					s1.adoptAbandoned(); // s1 adopts inside s2's window
				},
			},
		});
		const report = s2.adoptAbandoned();
		assert.deepEqual(report.adopted, [], "s2 found the record already adopted on retry");
		const record = s1.require("cg-1");
		assert.equal(record.state, "UNCERTAIN");
		assert.equal(record.version, 2);
		assert.equal(record.transitions.filter((t) => t.adoption).length, 1);
		assert.equal(record.transitions[1]!.adoption?.adoptedBy.pid, 2002);
	});

	it("the dispatcher's own late completion beats an adopter that read DISPATCHED first", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		// The dispatcher is "gone" to the probe (its pid is not alive) but its process is in fact
		// still writing: the strongest form of the race, and the one adoption must lose.
		const probe = { alive: () => false, startId: () => undefined };
		const dispatcher = new MutationLedger(dir, { self: { pid: 1001, processStartId: "start:1001" } });
		dispatcher.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		let competed = false;
		const adopter = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 2002, processStartId: "start:2002" },
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					dispatcher.markCompleted("cg-1", { type: "response", command: "x", success: true });
				},
			},
		});
		const report = adopter.adoptAbandoned();
		assert.deepEqual(report.adopted, [], "the adoption was refused on retry: the record is COMPLETED now");
		assert.equal(dispatcher.require("cg-1").state, "COMPLETED");
		assert.equal(dispatcher.require("cg-1").version, 2);
	});

	it("a record's versions are contiguous, immutable and one per write; a writer that never wins reports contention", () => {
		const { dir, ledger: A } = uncertain();
		A.recordProbe("cg-1", { detail: "p1" });
		A.recordProbe("cg-1", { detail: "p2" });
		assert.deepEqual(A.history("cg-1").map((v) => v.version), [1, 2, 3, 4]);
		assert.deepEqual(A.history("cg-1")[1]!.probes, undefined, "an older version is not rewritten when a newer one is published");
		let attempts = 0;
		const starved = new MutationLedger(dir, {
			hooks: {
				beforeCommit: () => {
					attempts += 1;
					A.recordProbe("cg-1", { detail: `interloper ${attempts}` }); // always wins the next version
				},
			},
		});
		assert.throws(() => starved.recordProbe("cg-1", { detail: "never" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "contended");
		assert.equal(attempts, MAX_CAS_ATTEMPTS);
		assert.ok(!A.require("cg-1").probes?.some((p) => p.detail === "never"), "the starved write published nothing");
	});

	it("negative control: the pre-review rename-in-place write loses A's evidence from a stale snapshot", () => {
		const { dir, ledger: A } = uncertain();
		const B = new MutationLedger(dir);
		const stale = B.require("cg-1"); // B's snapshot: UNCERTAIN
		A.resolveUncertain("cg-1", observed); // A: COMPLETED
		assert.equal(A.require("cg-1").state, "COMPLETED");
		// What the old #transition did: build from the stale snapshot and rename over the record.
		const regressed = { ...stale, probes: [{ at: "t", response: stored }] };
		writeFileDurable(A.currentVersionPath("cg-1"), `${JSON.stringify(regressed, null, 2)}\n`);
		assert.equal(A.require("cg-1").state, "UNCERTAIN", "the control demonstrates the lost update the CAS prevents");
		assert.ok(!A.require("cg-1").transitions.some((t) => t.evidence), "and the evidence is gone");
	});
});
