/**
 * D2 — worker-loss mutation ambiguity (Issue #17 blocker; Issue #15 s1-07 (c)).
 *
 * The exact bake-off reproducer, run through the Governor instead of a naive
 * client:
 *
 *   1. a mutating command is admitted and journaled by the supervisor;
 *   2. the worker performs the external effect (a line lands on disk);
 *   3. the worker is SIGKILLed before it can return the result;
 *   4. the supervisor records "Daemon worker socket closed" as a definite
 *      failure and replays it on retry.
 *
 * Required: the Governor classifies the outcome UNCERTAIN, keeps the original
 * command identity, mints no replacement id, dispatches nothing, and later
 * resolves UNCERTAIN -> COMPLETED from exact evidence without executing again.
 * The file must still hold exactly one line at the end.
 *
 * Falsification: the captured response is re-classified under the naive
 * policy (any failure is a failure) and must come out FAILED, proving this
 * test can tell a fail-closed classifier from a naive one.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { join } from "node:path";

import { classifyMutationOutcome, DEFAULT_POLICY, NAIVE_POLICY } from "../../governor/mutation/classify.ts";
import type { DaemonFailureResponse } from "../../governor/prime/protocol.ts";
import { lineCount, type PrimeFixture, startPrimeFixture, waitUntil } from "../lib/prime-fixture.ts";

let fixture: PrimeFixture;

before(async () => {
	fixture = await startPrimeFixture();
});

after(async () => {
	await fixture.stop();
});

describe("D2: worker transport lost after the external effect", () => {
	it("becomes UNCERTAIN, keeps its identity, is never re-dispatched, and resolves from evidence without a duplicate effect", async () => {
		const governor = await fixture.governor("d2");
		const created = await governor.createSession({ sessionPath: join(fixture.sessionDir, "d2-root.jsonl"), name: "cg-d2" });
		const { sessionId } = created.record;
		const activeSessionId = created.record.incarnations[0]!.activeSessionId;
		const workerPid = created.summary.workerPid;
		assert.ok(workerPid, "the summary names the worker pid");
		await governor.attach(sessionId);

		const target = join(fixture.work, "d2-effect.txt");
		const command = { type: "execute_bash_and_wait", command: `echo effect >> ${JSON.stringify(target)}; sleep 4` };

		// Step 1-2: dispatch, and wait until the effect is observably on disk.
		const dispatched = governor.dispatchMutation(sessionId, activeSessionId, command, { timeoutMs: 30_000 });
		await waitUntil(() => lineCount(target) === 1, 10_000, 25);
		const ledgerBefore = governor.ledger.list();
		assert.equal(ledgerBefore.filter((r) => r.commandType === "execute_bash_and_wait").length, 1, "exactly one mutation record exists");
		const [record0] = ledgerBefore.filter((r) => r.commandType === "execute_bash_and_wait");
		assert.equal(record0!.state, "DISPATCHED", "intent was durable before the effect happened");

		// Step 3: kill the worker before it can report.
		process.kill(workerPid, "SIGKILL");
		const result = await dispatched;

		// The critical invariant.
		assert.equal(result.verdict.verdict, "uncertain", `worker loss must classify UNCERTAIN, got ${JSON.stringify(result.verdict)}`);
		assert.equal(result.record.state, "UNCERTAIN");
		assert.equal(result.record.commandId, record0!.commandId, "the original command identity is preserved");
		assert.equal(lineCount(target), 1, "the effect happened exactly once");

		// No replacement id, no automatic re-dispatch: still exactly one mutation record for this command type.
		const afterLoss = governor.ledger.list().filter((r) => r.commandType === "execute_bash_and_wait");
		assert.equal(afterLoss.length, 1, "no replacement command id was minted");

		// The substrate side: the supervisor journaled a definite failure (the D2 defect). Record what it says.
		const stored = result.verdict.verdict === "uncertain" ? result.verdict.response : undefined;
		if (stored && !stored.success) {
			assert.equal(stored.errorInfo, undefined, "the stored failure is untyped: it carries no pre-effect error code");
		}

		// Step 4: recover the root (D1), then probe the substrate's stored result under the SAME identity.
		await governor.waitFailed(sessionId);
		const recovery = await governor.recoverResidentRoot(sessionId);
		assert.equal(recovery.action, "reopened");
		await governor.waitReady(sessionId);
		const probe = await governor.probeStoredResult(result.record.commandId, { ...command, activeSessionId });
		assert.equal(probe.verdict.verdict, "uncertain", "the replayed stored failure is still not proof of anything");
		assert.equal(probe.record.state, "UNCERTAIN");
		assert.equal(lineCount(target), 1, "probing the stored result did not execute the command again");
		const probed = probe.record.probes?.[0]?.response as DaemonFailureResponse | undefined;
		assert.ok(probed && probed.success === false, "the probe fetched the stored failure");
		assert.equal(probed.errorInfo, undefined, "the stored failure is untyped");

		// Late exact evidence resolves UNCERTAIN -> COMPLETED with no dispatch.
		const resolved = governor.resolveUncertain(result.record.commandId, {
			kind: "effect_observed",
			by: "conformance probe of the target file",
			detail: `${target} holds 1 line`,
			observedAt: new Date().toISOString(),
		});
		assert.equal(resolved.state, "COMPLETED");
		assert.equal(lineCount(target), 1, "resolution dispatched nothing");
		assert.equal(governor.ledger.list().filter((r) => r.commandType === "execute_bash_and_wait").length, 1);

		// Falsification: the same captured response under a naive policy is a definite failure.
		assert.ok(probed);
		const naive = classifyMutationOutcome({ kind: "response", response: probed }, NAIVE_POLICY);
		assert.equal(naive.verdict, "failed", "the negative control: a naive classifier would call this FAILED");
		const strict = classifyMutationOutcome({ kind: "response", response: probed }, DEFAULT_POLICY);
		assert.equal(strict.verdict, "uncertain");

		// And the consequence the bake-off measured: a client that trusted FAILED retried under a new id and
		// duplicated the effect. The Governor offers no such path; a superseding command is a human decision
		// that must name the uncertain record, and it is refused once the record is resolved.
		await assert.rejects(
			governor.dispatchMutation(sessionId, governor.registry.current(sessionId).activeSessionId, { type: "execute_bash_and_wait", command: "true" }, { supersedes: result.record.commandId }),
			/not UNCERTAIN/,
		);
		assert.equal(lineCount(target), 1);
		governor.close();
	});
});
