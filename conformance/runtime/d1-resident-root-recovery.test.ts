/**
 * D1 — resident roots do not self-heal (Issue #17 blocker; Issue #15 s1-03/s1-05).
 *
 * A resident root whose worker dies goes `workerState: failed` and is never
 * relaunched by the pinned supervisor. The Governor owns the reopen:
 *
 *   detect the failed state (from `workerState`, never `activity`)
 *   -> take the recovery lease
 *   -> `create` on the SAME canonical sessionPath, exactly once
 *   -> same Prime `sessionId`, new active-session incarnation recorded
 *   -> stale incarnation refused before dispatch
 *   -> no duplicate logical root
 *
 * Falsification: two Governors race to recover; with the fence exactly one
 * `create` is dispatched. With the fence disabled both dispatch a `create`
 * (Prime converges them, so the duplicate is visible only in the ledgers --
 * which is why the fence is the Governor's, not the substrate's).
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { join } from "node:path";

import { StaleCursorError, StaleIncarnationError } from "../../governor/session/registry.ts";
import { type PrimeFixture, startPrimeFixture } from "../lib/prime-fixture.ts";

let fixture: PrimeFixture;

before(async () => {
	fixture = await startPrimeFixture();
});

after(async () => {
	await fixture.stop();
});

async function messagesText(governor: Awaited<ReturnType<PrimeFixture["governor"]>>, sessionId: string): Promise<string> {
	const current = governor.registry.current(sessionId);
	const response = await governor.read({ type: "get_messages", activeSessionId: current.activeSessionId });
	assert.ok(response.success, `get_messages: ${response.success ? "" : response.error}`);
	return JSON.stringify(response.data);
}

describe("D1: a dead resident root is reopened exactly once under the same logical session", () => {
	it("reopens on the same path, keeps sessionId, records a new incarnation, and refuses the stale one", async () => {
		const governor = await fixture.governor("d1");
		const sessionPath = join(fixture.sessionDir, "d1-root.jsonl");
		const created = await governor.createSession({ sessionPath, name: "cg-d1" });
		const { sessionId } = created.record;
		const first = created.record.incarnations[0]!;
		await governor.attach(sessionId);

		const firstGeneration = governor.registry.current(sessionId).generation;
		assert.ok(firstGeneration, "attach recorded the first incarnation's event-cursor generation");

		// Observable work before the crash.
		const prompt = await governor.dispatchMutation(sessionId, first.activeSessionId, { type: "prompt", message: "ECHO:d1-before" });
		assert.equal(prompt.verdict.verdict, "completed");
		const idle = await governor.read({ type: "wait_for_idle", activeSessionId: first.activeSessionId }, 60_000);
		assert.ok(idle.success);
		assert.match(await messagesText(governor, sessionId), /d1-before/);
		const staleCursor = governor.client.lastCursor;
		assert.ok(staleCursor && staleCursor.generation === firstGeneration, "a cursor observed before the crash belongs to the first incarnation's generation");

		// Healthy root: recovery is a no-op and says so.
		const healthy = await governor.recoverResidentRoot(sessionId);
		assert.equal(healthy.action, "healthy");

		// Kill the worker. The supervisor reports failed; `activity` is not consulted.
		assert.ok(created.summary.workerPid);
		process.kill(created.summary.workerPid, "SIGKILL");
		const failed = await governor.waitFailed(sessionId);
		assert.equal(failed.workerState, "failed");
		assert.equal(failed.sessionId, sessionId, "the failed summary still names the logical session");

		const outcome = await governor.recoverResidentRoot(sessionId);
		assert.equal(outcome.action, "reopened", JSON.stringify(outcome));
		assert.equal(outcome.previous.activeSessionId, first.activeSessionId);
		assert.notEqual(outcome.incarnation.activeSessionId, first.activeSessionId, "reopen yields a new active-session id");
		assert.equal(outcome.incarnation.cause, "reopen");
		assert.equal(outcome.incarnation.index, 1);

		const record = governor.registry.require(sessionId);
		assert.equal(record.sessionPath, created.record.sessionPath, "the same canonical path");
		assert.equal(record.incarnations.length, 2);

		await governor.waitReady(sessionId);
		await governor.attach(sessionId);
		const secondGeneration = governor.registry.current(sessionId).generation;
		assert.ok(secondGeneration && secondGeneration !== firstGeneration, "the reopened incarnation has a new generation");
		const text = await messagesText(governor, sessionId);
		assert.match(text, /d1-before/, "history survived the reopen");
		assert.match(text, /worker_recovery|recovery/i, "the transcript carries Prime's visible recovery marker");

		// No duplicate logical root.
		const listed = (await governor.list()).filter((summary) => summary.sessionId === sessionId);
		assert.equal(listed.length, 1, "exactly one registration for the logical session");
		assert.equal(listed[0]!.workerState, "ready");

		// Stale incarnation is refused before any I/O.
		const ledgerBefore = governor.ledger.list().length;
		await assert.rejects(
			governor.dispatchMutation(sessionId, first.activeSessionId, { type: "prompt", message: "ECHO:stale" }),
			(error: unknown) => error instanceof StaleIncarnationError && error.presented === first.activeSessionId,
		);
		assert.equal(governor.ledger.list().length, ledgerBefore, "a refused stale dispatch writes no ledger record");

		// Scenario 6, cursor half: a cursor from the dead generation is refused by the Governor ...
		assert.throws(() => governor.assertCurrentCursor(sessionId, staleCursor), (error: unknown) => error instanceof StaleCursorError && error.presented === firstGeneration && error.current === secondGeneration);
		assert.equal(governor.assertCurrentCursor(sessionId, { generation: secondGeneration, sequence: 0 }).activeSessionId, outcome.incarnation.activeSessionId);
		// ... and Prime itself does not honour it as a position (Issue #15 D3): replay restarts at the new generation.
		const replayed = await governor.read({ type: "attach", activeSessionId: outcome.incarnation.activeSessionId, clientId: governor.clientId, telemetryDisabled: true, resumeCursor: staleCursor }, 60_000);
		assert.ok(replayed.success);
		const replay = (replayed.data as { replay?: { toCursor?: { generation: string; sequence: number }; toSequence?: number } }).replay;
		assert.equal(replay?.toCursor?.generation, secondGeneration, "the substrate answers in the new generation");
		assert.equal(replay?.toSequence, 0, "and restarts replay at its baseline rather than honouring the dead sequence");
		assert.ok(!(await messagesText(governor, sessionId)).includes("ECHO:stale"), "the stale prompt never reached the new incarnation");

		// The new incarnation serves work.
		const after = await governor.dispatchMutation(sessionId, outcome.incarnation.activeSessionId, { type: "prompt", message: "ECHO:d1-after" });
		assert.equal(after.verdict.verdict, "completed");
		await governor.read({ type: "wait_for_idle", activeSessionId: outcome.incarnation.activeSessionId }, 60_000);
		assert.match(await messagesText(governor, sessionId), /d1-after/);

		// Second recovery call after a successful reopen: nothing to do.
		const again = await governor.recoverResidentRoot(sessionId);
		assert.equal(again.action, "healthy");
		governor.close();
	});

	it("fences concurrent recoverers: exactly one create is dispatched (and the negative control shows two without the fence)", async () => {
		for (const fenced of [true, false]) {
			const stateDir = join(fixture.root, "governor", `d1-race-${fenced ? "fenced" : "unfenced"}`);
			const a = await fixture.governor(`d1-race-${fenced}-a`, { stateDir, unsafeDisableRecoveryFence: !fenced });
			const b = await fixture.governor(`d1-race-${fenced}-b`, { stateDir, unsafeDisableRecoveryFence: !fenced });
			assert.equal(a.clientId, b.clientId, "two Governor processes over one state dir share the client identity");
			const created = await a.createSession({ sessionPath: join(fixture.sessionDir, `d1-race-${fenced}.jsonl`) });
			const { sessionId } = created.record;
			assert.ok(created.summary.workerPid);
			process.kill(created.summary.workerPid, "SIGKILL");
			await a.waitFailed(sessionId);

			const [ra, rb] = await Promise.all([a.recoverResidentRoot(sessionId), b.recoverResidentRoot(sessionId)]);
			const actions = [ra.action, rb.action].sort();
			const creates = a.ledger.list().filter((r) => r.commandType === "create" && r.sessionId === sessionId);
			if (fenced) {
				assert.deepEqual(actions.filter((x) => x === "reopened"), ["reopened"], `exactly one reopen: ${JSON.stringify(actions)}`);
				assert.ok(actions.includes("lease_held") || actions.includes("converged"), `the other observed the lease or converged: ${JSON.stringify(actions)}`);
				assert.equal(creates.length, 1, "with the fence, one reopen create was dispatched");
			} else {
				assert.equal(creates.length, 2, "negative control: without the fence both Governors dispatched a create");
			}
			// Either way Prime converged: one registration, one logical root.
			await a.waitReady(sessionId);
			const listed = (await a.list()).filter((summary) => summary.sessionId === sessionId);
			assert.equal(listed.length, 1);
			a.close();
			b.close();
		}
	});
});
