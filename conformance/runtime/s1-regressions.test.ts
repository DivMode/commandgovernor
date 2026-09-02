/**
 * Issue #15 S1 behaviours that already passed and must keep passing under the
 * Governor (Issue #17 required scenarios 10-12):
 *
 *  10. supervisor loss mid-command -> the substrate reports
 *      `command_result_uncertain` on same-identity retry; the Governor
 *      classifies UNCERTAIN; the effect happens at most once;
 *  11. completed-command idempotence: the same `clientId + commandId` returns
 *      the stored result and does not repeat the effect;
 *  12. process-safe session lease: a second open of an owned path converges
 *      or is refused with the typed `session_already_active`; no second
 *      writer; transcript bytes unchanged.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { DaemonClient } from "../../governor/prime/daemon-client.ts";
import { buildLaunchEnv } from "../../governor/prime/env.ts";
import { expectedSubstrate, isProcessAlive } from "../../governor/prime/substrate.ts";
import { lineCount, type PrimeFixture, sleep, startPrimeFixture, waitUntil } from "../lib/prime-fixture.ts";

let fixture: PrimeFixture;

before(async () => {
	fixture = await startPrimeFixture();
});

after(async () => {
	await fixture.stop();
});

describe("S1 regressions under the Governor", () => {
	it("(11) completed-command idempotence: the same identity replays the stored result without a second effect", async () => {
		const governor = await fixture.governor("idem");
		const created = await governor.createSession({ sessionPath: join(fixture.sessionDir, "idem.jsonl") });
		const { sessionId } = created.record;
		const active = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);
		const target = join(fixture.work, "idem.txt");
		const command = { type: "execute_bash_and_wait", activeSessionId: active, command: `echo run >> ${JSON.stringify(target)}` };
		const first = await governor.dispatchMutation(sessionId, active, command);
		assert.equal(first.verdict.verdict, "completed");
		assert.equal(lineCount(target), 1);
		// The Governor never re-dispatches; the raw replay is what the substrate guarantees, shown here directly.
		const replay = await governor.client.request(command, first.record.commandId);
		assert.ok(replay.success);
		assert.deepEqual(replay, (first.verdict.verdict === "completed" ? first.verdict.response : undefined), "byte-for-byte the stored response");
		assert.equal(lineCount(target), 1, "no duplicate effect");
		// A second Governor command id is new work, by design: identity is the idempotency key.
		const second = await governor.dispatchMutation(sessionId, active, command);
		assert.equal(second.verdict.verdict, "completed");
		assert.notEqual(second.record.commandId, first.record.commandId);
		assert.equal(lineCount(target), 2);
		governor.close();
	});

	it("(12) session lease: a second open of an owned path never creates a second writer", async () => {
		const governor = await fixture.governor("lease");
		const sessionPath = join(fixture.sessionDir, "lease.jsonl");
		const created = await governor.createSession({ sessionPath });
		const { sessionId } = created.record;
		const active = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);
		await governor.dispatchMutation(sessionId, active, { type: "prompt", message: "ECHO:lease-owner" });
		await governor.read({ type: "wait_for_idle", activeSessionId: active }, 60_000);
		const bytesBefore = readFileSync(created.record.sessionPath).length;

		// The Governor refuses at preflight: the path is already bound.
		await assert.rejects(governor.createSession({ sessionPath }), /already belongs to session/);

		// A foreign daemon client on the same path converges onto the owner (Issue #15 D4) -- no new worker.
		const other = new DaemonClient(fixture.socketPath, { clientId: "cg-lease-contender", expected: expectedSubstrate() });
		await other.connect();
		const launchEnv = buildLaunchEnv(process.env, { overrides: { HOME: fixture.home, TMPDIR: fixture.tmpDir, PRIME_AGENT_CODING_AGENT_DIR: fixture.agentDir, PRIME_AGENT_TELEMETRY: "0", PRIME_AGENT_INSTALL_UV: "0" } }).env;
		const config = { cwd: fixture.work, agentDir: fixture.agentDir, sessionDir: fixture.sessionDir, provider: "mock", model: "mock-1", noExtensions: true, noSkills: true, noContextFiles: true, noPromptTemplates: true, noThemes: true, telemetryDisabled: true };
		const contend = await other.request({ type: "create", sessionPath: created.record.sessionPath, config, launchEnv }, "cg-lease-contender-create", 120_000);
		assert.ok(contend.success, contend.success ? "" : contend.error);
		const convergedId = (contend.data as { activeSessionId?: string; id: string }).activeSessionId ?? (contend.data as { id: string }).id;
		assert.equal(convergedId, active, "converged onto the owner's active session, not a second worker");
		// A different session trying to switch onto the owned path is refused with the typed lease error.
		const own = await other.request({ type: "create", sessionPath: join(fixture.sessionDir, "lease-contender-own.jsonl"), config, launchEnv }, "cg-lease-contender-own", 120_000);
		assert.ok(own.success, own.success ? "" : own.error);
		const ownId = (own.data as { activeSessionId?: string; id: string }).activeSessionId ?? (own.data as { id: string }).id;
		assert.notEqual(ownId, active);
		const switched = await other.request({ type: "switch_session", activeSessionId: ownId, sessionPath: created.record.sessionPath }, "cg-lease-contender-switch");
		assert.ok(!switched.success);
		assert.equal(switched.success ? undefined : switched.errorInfo?.code, "session_already_active", "the typed lease error names the owner");
		other.close();

		const listed = (await governor.list()).filter((s) => s.sessionId === sessionId);
		assert.equal(listed.length, 1);
		assert.equal(readFileSync(created.record.sessionPath).length, bytesBefore, "transcript bytes unchanged by the contenders");
		governor.close();
	});

	it("(10) supervisor loss mid-command: the substrate says uncertain, the Governor records UNCERTAIN, the effect happens at most once", async () => {
		const governor = await fixture.governor("sup");
		const created = await governor.createSession({ sessionPath: join(fixture.sessionDir, "sup.jsonl") });
		const { sessionId } = created.record;
		const active = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);
		const target = join(fixture.work, "sup.txt");
		const command = { type: "execute_bash_and_wait", activeSessionId: active, command: `sleep 3; echo unc >> ${JSON.stringify(target)}` };
		const supervisorPid = governor.client.hello?.supervisorPid;
		assert.ok(supervisorPid);
		const pending = governor.dispatchMutation(sessionId, active, command, { timeoutMs: 30_000 });
		await sleep(400);
		process.kill(supervisorPid, "SIGKILL");
		const lost = await pending;
		assert.equal(lost.verdict.verdict, "uncertain", "the socket died with the supervisor: UNCERTAIN");
		assert.equal(lost.verdict.verdict === "uncertain" ? lost.verdict.reason : undefined, "transport_lost");
		governor.close();

		// A replacement supervisor comes up on its own (launched by a worker). Reconnect and probe the same identity.
		await waitUntil(() => !isProcessAlive(supervisorPid), 10_000);
		const governor2 = await fixture.governor("sup"); // same state dir => same clientId
		assert.equal(governor2.clientId, governor.clientId);
		assert.notEqual(governor2.client.hello?.supervisorPid, supervisorPid, "a replacement supervisor answered");
		await sleep(5000); // let the in-flight bash (if the worker survived) finish, so the count below is final
		const probe = await governor2.probeStoredResult(lost.record.commandId, command);
		assert.equal(probe.verdict.verdict, "uncertain");
		assert.equal(probe.verdict.verdict === "uncertain" ? probe.verdict.reason : undefined, "substrate_reported_uncertain", "Prime replied command_result_uncertain for the journaled receipt");
		assert.ok(lineCount(target) <= 1, `the effect happened at most once (${lineCount(target)})`);
		await sleep(500);
		assert.ok(lineCount(target) <= 1, "and the probe did not execute it again");
		governor2.close();
	});
});
