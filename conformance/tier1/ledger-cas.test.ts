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

import { chmodSync, mkdtempSync } from "node:fs";
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
			maxAttempts: 8,
			hooks: {
				beforeCommit: () => {
					attempts += 1;
					A.recordProbe("cg-1", { detail: `interloper ${attempts}` }); // always wins the next version
				},
			},
		});
		assert.throws(() => starved.recordProbe("cg-1", { detail: "never" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "contended");
		assert.equal(attempts, 8);
		assert.ok(MAX_CAS_ATTEMPTS > 8, "the production bound is far higher (and each lost attempt backs off)");
		assert.ok(!A.require("cg-1").probes?.some((p) => p.detail === "never"), "the starved write published nothing");
	});

	it("the dispatcher's own outcome is kept as evidence when an adopter marked the record uncertain first", () => {
		// The reverse ordering of the race above: adoption commits before the dispatcher's result lands.
		const setup = () => {
			const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
			const dispatcher = new MutationLedger(dir, { self: { pid: 1001, processStartId: "start:1001" } });
			dispatcher.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
			const adopter = new MutationLedger(dir, { processProbe: { alive: () => false, startId: () => undefined }, self: { pid: 2002, processStartId: "start:2002" } });
			assert.equal(adopter.adoptAbandoned().adopted.length, 1);
			assert.equal(dispatcher.require("cg-1").state, "UNCERTAIN");
			return dispatcher;
		};
		// A success response is exact evidence: UNCERTAIN -> COMPLETED, with the response kept as a probe.
		const ok = setup();
		const completed = ok.recordOutcome("cg-1", { verdict: "completed", response: { type: "response", command: "x", success: true } });
		assert.equal(completed.state, "COMPLETED");
		assert.equal(completed.transitions[completed.transitions.length - 1]!.evidence?.by, "dispatcher's own response");
		assert.equal(completed.probes?.length, 1);
		assert.deepEqual(ok.history("cg-1").map((v) => v.state), ["DISPATCHED", "UNCERTAIN", "UNCERTAIN", "COMPLETED"]);
		// A typed pre-effect rejection is exact evidence the other way: UNCERTAIN -> FAILED.
		const failed = setup().recordOutcome("cg-1", {
			verdict: "failed",
			proof: { kind: "typed_pre_effect_rejection", commandType: "import_jsonl", code: "session_import_file_not_found" },
			response: { type: "response", command: "x", success: false, error: "nope", errorInfo: { code: "session_import_file_not_found", filePath: "/x" } },
		});
		assert.equal(failed.state, "FAILED");
		assert.equal(failed.transitions[failed.transitions.length - 1]!.evidence?.kind, "effect_absent_proven");
		// An uncertain outcome adds nothing the adoption did not: it is appended as a probe, state unchanged.
		const still = setup().recordOutcome("cg-1", { verdict: "uncertain", reason: "transport_lost", detail: "socket closed" });
		assert.equal(still.state, "UNCERTAIN");
		assert.equal(still.probes?.length, 1);
		assert.match(still.probes![0]!.detail!, /transport_lost/);
		// And with no adoption in the way, recordOutcome is the plain DISPATCHED transition.
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const plain = new MutationLedger(dir);
		plain.recordDispatch({ commandId: "cg-2", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		assert.equal(plain.recordOutcome("cg-2", { verdict: "completed", response: { type: "response", command: "x", success: true } }).version, 2);
		// A resolved record is not something a late outcome may touch: that is the caller's error.
		assert.throws(() => plain.recordOutcome("cg-2", { verdict: "uncertain", reason: "timeout" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "illegal_transition");
	});

	it("supersede vs resolution: a resolution that lands first makes the claim fail, and no replacement record is created", () => {
		let competed = false;
		const { dir, ledger: A } = uncertain();
		const B = new MutationLedger(dir, {
			hooks: {
				beforeCommit: (commandId) => {
					if (competed || commandId !== "cg-1") return;
					competed = true;
					A.resolveUncertain("cg-1", observed); // O is COMPLETED before B's claim lands
				},
			},
		});
		assert.throws(
			() => B.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }),
			(e: unknown) => e instanceof MutationLedgerError && e.code === "supersedes_not_uncertain",
		);
		assert.equal(B.get("cg-R"), undefined, "the replacement was never created, so it can never be sent");
		assert.equal(A.require("cg-1").supersededBy, undefined, "and no claim was written on the resolved record");
	});

	it("supersede vs supersede: exactly one claim wins, exactly one replacement exists, the loser is refused before creating anything", () => {
		let competed = false;
		const { dir, ledger: A } = uncertain();
		const B = new MutationLedger(dir, {
			hooks: {
				beforeCommit: (commandId) => {
					if (competed || commandId !== "cg-1") return;
					competed = true;
					A.recordDispatch({ commandId: "cg-RA", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" });
				},
			},
		});
		assert.throws(
			() => B.recordDispatch({ commandId: "cg-RB", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }),
			(e: unknown) => e instanceof MutationLedgerError && e.code === "already_superseded" && /cg-RA/.test(e.message),
		);
		assert.equal(B.get("cg-RB"), undefined, "the losing replacement was never created");
		assert.equal(A.require("cg-RA")?.supersedes, "cg-1");
		assert.equal(A.require("cg-1").supersededBy?.commandId, "cg-RA");
		assert.equal(A.require("cg-1").state, "UNCERTAIN", "the claim does not resolve the record");
		// Exact evidence about O still resolves it afterwards; the claim stays on the record for the reader.
		const resolved = A.resolveUncertain("cg-1", observed);
		assert.equal(resolved.state, "COMPLETED");
		assert.equal(resolved.supersededBy?.commandId, "cg-RA");
		// And nobody can supersede a resolved record, claimed or not.
		assert.throws(() => A.recordDispatch({ commandId: "cg-RC", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), /not UNCERTAIN/);
	});

	it("a claimant that dies between the claim and the create: the claim is released by a successor under the identity fence, and a new supersede succeeds", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const setup = new MutationLedger(dir, { processProbe: probe, self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		const dying = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 1001, processStartId: "start:1001" },
			hooks: {
				afterClaim: () => {
					throw new Error("SIGKILL between the claim and the create");
				},
			},
		});
		assert.throws(() => dying.recordDispatch({ commandId: "cg-R1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), /SIGKILL/);
		assert.equal(setup.require("cg-1").supersededBy?.commandId, "cg-R1", "the claim is on disk");
		assert.equal(setup.get("cg-R1"), undefined, "the replacement is not");
		// A live claimant (pid 9's world says 1001 is alive) is honoured: the claim is pending, not released.
		const cautious = new MutationLedger(dir, { processProbe: { alive: () => true, startId: (pid) => `start:${pid}` }, self: { pid: 2002, processStartId: "start:2002" } });
		const pending = cautious.adoptAbandoned();
		assert.deepEqual(pending.releasedClaims, []);
		assert.deepEqual(pending.pendingClaims.map((c) => [c.record.commandId, c.verdict]), [["cg-1", "current"]]);
		assert.throws(() => cautious.recordDispatch({ commandId: "cg-R2", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "already_superseded");
		// Once the claimant is proven gone, a successor releases the claim and a new supersede goes through.
		const successor = new MutationLedger(dir, { processProbe: probe, self: { pid: 3003, processStartId: "start:3003" } });
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.releasedClaims.map((r) => r.commandId), ["cg-1"]);
		const released = successor.require("cg-1");
		assert.equal(released.supersededBy, undefined);
		assert.equal(released.transitions[released.transitions.length - 1]!.claim?.action, "released");
		assert.equal(released.state, "UNCERTAIN");
		const replacement = successor.recordDispatch({ commandId: "cg-R3", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" });
		assert.equal(replacement.supersedes, "cg-1");
		assert.equal(successor.require("cg-1").supersededBy?.commandId, "cg-R3");
		assert.deepEqual(successor.adoptAbandoned().pendingClaims, [], "a claim whose replacement exists is not reported");
	});

	it("the claim is two-phase: a confirmed claim is never released, even when the claimant is proven gone", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const setup = new MutationLedger(dir, { processProbe: probe, self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		const claimant = new MutationLedger(dir, { processProbe: probe, self: { pid: 1001, processStartId: "start:1001" } });
		// The claimant creates R, confirms, sends R, and dies (pid 1001 is dead to the probe).
		const R = claimant.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" });
		assert.equal(R.state, "DISPATCHED");
		const claim = setup.require("cg-1").supersededBy!;
		assert.equal(claim.commandId, "cg-R");
		assert.ok(claim.confirmedAt, "confirmed before recordDispatch returned, i.e. before R could be sent");
		assert.deepEqual(setup.history("cg-1").map((v) => v.transitions[v.transitions.length - 1]!.claim?.action), [undefined, undefined, "taken", "confirmed"]);
		const successor = new MutationLedger(dir, { processProbe: probe, self: { pid: 3003, processStartId: "start:3003" } });
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.releasedClaims, [], "a confirmed claim is never released");
		assert.deepEqual(report.pendingClaims, []);
		assert.equal(successor.require("cg-1").supersededBy?.commandId, "cg-R");
		assert.throws(() => successor.recordDispatch({ commandId: "cg-R2", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "already_superseded");
		assert.deepEqual(report.adopted.map((r) => r.commandId), ["cg-R"], "R itself is adopted as an abandoned dispatch, as any record of a dead dispatcher is");
	});

	it("the reviewer's race: the adopter derives a release with R absent, the claimant creates and confirms R, the adopter's publish is refused", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const setup = new MutationLedger(dir, { processProbe: probe, self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		// The claimant (dead to the probe) takes the claim and stalls before creating R.
		let stall = true;
		const claimant = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 1001, processStartId: "start:1001" },
			hooks: {
				afterClaim: () => {
					if (stall) throw new Error("stall");
				},
			},
		});
		const R = { commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" };
		assert.throws(() => claimant.recordDispatch(R), /stall/);
		assert.equal(setup.get("cg-R"), undefined);
		// The adopter reads the unconfirmed claim with R absent and derives a release; in its window the claimant resumes
		// (re-driving its own claim: create R, confirm) and publishes first.
		let competed = false;
		const adopter = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 2002, processStartId: "start:2002" },
			hooks: {
				beforeCommit: (commandId) => {
					if (competed || commandId !== "cg-1") return;
					competed = true;
					stall = false;
					assert.equal(claimant.recordDispatch(R).state, "DISPATCHED");
				},
			},
		});
		const report = adopter.adoptAbandoned();
		assert.deepEqual(report.releasedClaims, [], "the release was refused on retry: the claim is confirmed now");
		const O = setup.require("cg-1");
		assert.equal(O.supersededBy?.commandId, "cg-R");
		assert.ok(O.supersededBy?.confirmedAt);
		assert.equal(setup.require("cg-R").state, "DISPATCHED");
		assert.equal(O.transitions.filter((t) => t.claim?.action === "released").length, 0);
		// And no second replacement can be claimed.
		assert.throws(() => adopter.recordDispatch({ ...R, commandId: "cg-R2" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "already_superseded");
	});

	it("the confirm loses to a release: the replacement is marked never sent and the caller gets claim_lost, so nothing goes out", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const setup = new MutationLedger(dir, { processProbe: probe, self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		// While the claimant is between its claim and its create, a successor (to whom the claimant is gone) releases the claim.
		let stall = true;
		const dying = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 1001, processStartId: "start:1001" },
			hooks: {
				afterClaim: () => {
					if (!stall) return;
					stall = false;
					const successor = new MutationLedger(dir, { processProbe: probe, self: { pid: 3003, processStartId: "start:3003" } });
					assert.equal(successor.adoptAbandoned().releasedClaims.length, 1);
				},
			},
		});
		assert.throws(
			() => dying.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }),
			(e: unknown) => e instanceof MutationLedgerError && e.code === "claim_lost",
		);
		const R = setup.require("cg-R");
		assert.equal(R.state, "FAILED", "the replacement exists but is marked never sent");
		assert.ok(R.transitions[R.transitions.length - 1]!.neverSent, "with the dispatcher's own proof");
		assert.equal(setup.require("cg-1").supersededBy, undefined, "O is unclaimed");
		assert.equal(setup.require("cg-1").state, "UNCERTAIN");
		// A later supersede proceeds normally: exactly one replacement will ever be SENT for O.
		const again = new MutationLedger(dir, { processProbe: probe, self: { pid: 4004, processStartId: "start:4004" } });
		assert.equal(again.recordDispatch({ commandId: "cg-R2", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }).state, "DISPATCHED");
		assert.equal(setup.require("cg-1").supersededBy?.commandId, "cg-R2");
	});

	it("a created-but-unconfirmed replacement of a dead claimant: the claim is released and the replacement resolved never sent, so the record is supersedable again", () => {
		// The claimant died between its create of R and its confirm. R exists (DISPATCHED, never sent); the claim is unconfirmed.
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const setup = new MutationLedger(dir, { processProbe: probe, self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		const dying = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 1001, processStartId: "start:1001" },
			hooks: {
				beforeCommit: (commandId, fromVersion) => {
					if (commandId === "cg-1" && fromVersion === 3) throw new Error("SIGKILL before confirm"); // v3 = the taken claim; this is the confirm write
				},
			},
		});
		assert.throws(() => dying.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), /SIGKILL/);
		assert.equal(setup.require("cg-R").state, "DISPATCHED");
		assert.equal(setup.require("cg-1").supersededBy?.confirmedAt, undefined);
		// While the claimant is alive (to this probe), nothing is touched: it may be about to confirm.
		const cautious = new MutationLedger(dir, { processProbe: { alive: () => true, startId: (pid) => `start:${pid}` }, self: { pid: 2002, processStartId: "start:2002" } });
		const pending = cautious.adoptAbandoned();
		assert.deepEqual(pending.releasedClaims, []);
		assert.deepEqual(pending.pendingClaims.map((c) => [c.record.commandId, c.verdict]), [["cg-1", "current"]]);
		assert.equal(setup.require("cg-R").state, "DISPATCHED");
		// Once the claimant is proven over: it never confirmed, so it never sent. The claim is released and R resolved never sent.
		const successor = new MutationLedger(dir, { processProbe: probe, self: { pid: 3003, processStartId: "start:3003" } });
		const report = successor.adoptAbandoned();
		assert.deepEqual(report.releasedClaims.map((r) => r.commandId), ["cg-1"]);
		const R = setup.require("cg-R");
		assert.equal(R.state, "FAILED", "the never-sent replacement can never be probed or sent");
		assert.equal(R.transitions[R.transitions.length - 1]!.evidence?.kind, "effect_absent_proven");
		assert.ok(R.transitions[R.transitions.length - 1]!.neverSent);
		assert.deepEqual(report.adopted, [], "R was resolved by proof, not adopted as uncertain");
		assert.equal(setup.require("cg-1").supersededBy, undefined);
		assert.equal(successor.recordDispatch({ commandId: "cg-R2", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }).state, "DISPATCHED");
	});

	it("a lost never-sent mark still reports claim_lost, and the replacement is resolved from the dispatcher's own proof even after adoption", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: () => false, startId: () => undefined };
		const setup = new MutationLedger(dir, { self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		let stall = true;
		let rogueAdopted = false;
		const dying = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 1001, processStartId: "start:1001" },
			hooks: {
				afterClaim: () => {
					if (!stall) return;
					stall = false;
					new MutationLedger(dir, { processProbe: probe, self: { pid: 3003, processStartId: "start:3003" } }).adoptAbandoned(); // releases the claim
				},
				beforeCommit: (commandId) => {
					// Between the never-sent derive and its publish, a rogue adopter (to whom the claimant is gone) adopts R as UNCERTAIN.
					if (commandId !== "cg-R" || rogueAdopted) return;
					rogueAdopted = true;
					assert.equal(new MutationLedger(dir, { processProbe: probe, self: { pid: 4004, processStartId: "start:4004" } }).adoptAbandoned().adopted.length, 1);
				},
			},
		});
		assert.throws(
			() => dying.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }),
			(e: unknown) => e instanceof MutationLedgerError && e.code === "claim_lost",
		);
		const R = setup.require("cg-R");
		assert.equal(R.state, "FAILED", "resolved from the dispatcher's proof on top of the adoption");
		assert.deepEqual(setup.history("cg-R").map((v) => v.state), ["DISPATCHED", "UNCERTAIN", "FAILED"]);
		assert.equal(R.transitions[R.transitions.length - 1]!.evidence?.kind, "effect_absent_proven");
	});

	it("only a record's own dispatcher may mark it never sent", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const A = new MutationLedger(dir, { ownerToken: "gov-A" });
		A.recordDispatch({ commandId: "cg-x", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		const B = new MutationLedger(dir, { ownerToken: "gov-B" });
		assert.throws(() => B.markNeverSent("cg-x", "not mine"), (e: unknown) => e instanceof MutationLedgerError && e.code === "illegal_transition" && /only its dispatcher/.test(e.message));
		assert.equal(A.require("cg-x").state, "DISPATCHED");
		assert.equal(A.markCompleted("cg-x", { type: "response", command: "x", success: true }).state, "COMPLETED", "A's real result still lands");
		assert.equal(A.markNeverSent("cg-x", "too late").state, "COMPLETED", "a resolved record is left as it is, without throwing");
	});

	it("a create that fails after the claim releases the claim it took, so the record is supersedable again at once", { skip: process.getuid?.() === 0 ? "root ignores directory modes" : false }, () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const A = new MutationLedger(dir);
		A.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		A.markUncertain("cg-1", "transport_lost");
		// Between C's claim and C's create, the ledger directory becomes unwritable: the create of R fails with EACCES.
		const C = new MutationLedger(dir, { hooks: { afterClaim: () => chmodSync(A.dir, 0o500) } });
		try {
			assert.throws(() => C.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), (e: unknown) => (e as NodeJS.ErrnoException).code === "EACCES");
		} finally {
			chmodSync(A.dir, 0o700);
		}
		assert.equal(A.get("cg-R"), undefined);
		assert.equal(A.require("cg-1").supersededBy, undefined, "C released the claim it had taken");
		assert.equal(A.require("cg-1").transitions[A.require("cg-1").transitions.length - 1]!.claim?.action, "released");
		assert.equal(A.recordDispatch({ commandId: "cg-R2", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }).supersedes, "cg-1");
	});

	it("releasedClaims names only the release this ledger performed", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-cas-"));
		const probe = { alive: (pid: number) => pid !== 1001, startId: (pid: number) => `start:${pid}` };
		const setup = new MutationLedger(dir, { processProbe: probe, self: { pid: 9, processStartId: "start:9" } });
		setup.recordDispatch({ commandId: "cg-1", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		setup.markUncertain("cg-1", "transport_lost");
		const dying = new MutationLedger(dir, { processProbe: probe, self: { pid: 1001, processStartId: "start:1001" }, hooks: { afterClaim: () => { throw new Error("SIGKILL"); } } });
		assert.throws(() => dying.recordDispatch({ commandId: "cg-R", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0, supersedes: "cg-1" }), /SIGKILL/);
		const s1 = new MutationLedger(dir, { processProbe: probe, self: { pid: 2002, processStartId: "start:2002" } });
		let competed = false;
		const s2 = new MutationLedger(dir, {
			processProbe: probe,
			self: { pid: 3003, processStartId: "start:3003" },
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					s1.adoptAbandoned(); // s1 releases inside s2's window
				},
			},
		});
		assert.deepEqual(s2.adoptAbandoned().releasedClaims, [], "s2 did not release anything; s1 did");
		assert.equal(setup.require("cg-1").transitions.filter((t) => t.claim?.action === "released").length, 1);
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
