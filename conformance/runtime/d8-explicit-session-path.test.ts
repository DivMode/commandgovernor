/**
 * D8 — mandatory explicit sessionPath (Issue #17 blocker; Issue #15 s1-05 (3)).
 *
 * Every Governor-created session names its transcript. The runtime half of
 * the proof: create -> work -> worker loss -> reopen/relaunch preserves the
 * transcript when the path was explicit. The negative control reproduces the
 * bake-off observation through the raw daemon protocol, bypassing the
 * Governor: a client-owned worker created WITHOUT a path relaunches with an
 * empty live transcript although its JSONL on disk has the turns.
 *
 * The pure half (preflight refusals, canonicalisation, the fence to the
 * session directory) is in conformance/tier1/session-paths.test.ts.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { DaemonClient } from "../../governor/prime/daemon-client.ts";
import { buildLaunchEnv } from "../../governor/prime/env.ts";
import { activeSessionIdOf, isSessionSummary } from "../../governor/prime/protocol.ts";
import { expectedSubstrate } from "../../governor/prime/substrate.ts";
import { SessionPathError } from "../../governor/session/paths.ts";
import { type PrimeFixture, startPrimeFixture, waitUntil } from "../lib/prime-fixture.ts";

let fixture: PrimeFixture;

before(async () => {
	fixture = await startPrimeFixture();
});

after(async () => {
	await fixture.stop();
});

describe("D8: explicit sessionPath on every created session", () => {
	it("refuses a create without a path before any I/O", async () => {
		const governor = await fixture.governor("d8-preflight");
		const before = governor.ledger.list().length;
		await assert.rejects(governor.createSession({ sessionPath: undefined }), (error: unknown) => error instanceof SessionPathError && error.issue === "missing");
		await assert.rejects(governor.createSession({ sessionPath: "relative/path.jsonl" }), (error: unknown) => error instanceof SessionPathError && error.issue === "relative");
		await assert.rejects(governor.createSession({ sessionPath: join(fixture.root, "elsewhere.jsonl") }), (error: unknown) => error instanceof SessionPathError && error.issue === "outside_session_dir");
		assert.equal(governor.ledger.list().length, before, "no dispatch record was written for a refused create");
		assert.equal((await governor.list()).length, 0, "the supervisor never saw a create");
		governor.close();
	});

	it("resident: create -> work -> worker loss -> reopen keeps the transcript", async () => {
		const governor = await fixture.governor("d8-resident");
		const sessionPath = join(fixture.sessionDir, "sub", "..", "d8-resident.jsonl"); // non-canonical spelling on purpose
		const created = await governor.createSession({ sessionPath });
		assert.equal(created.record.sessionPath, join(fixture.sessionDir, "d8-resident.jsonl").replace(/^\/tmp\//, "/private/tmp/"), "the recorded path is canonical");
		assert.equal(created.summary.sessionFile, created.record.sessionPath, "Prime's sessionFile is the canonical path we asked for");
		const { sessionId } = created.record;
		const active = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);
		await governor.dispatchMutation(sessionId, active, { type: "prompt", message: "ECHO:d8-resident-turn" });
		await governor.read({ type: "wait_for_idle", activeSessionId: active }, 60_000);
		assert.ok(existsSync(created.record.sessionPath), "the transcript exists at the explicit path");
		assert.match(readFileSync(created.record.sessionPath, "utf8"), /d8-resident-turn/);

		assert.ok(created.summary.workerPid);
		process.kill(created.summary.workerPid, "SIGKILL");
		await governor.waitFailed(sessionId);
		const outcome = await governor.recoverResidentRoot(sessionId);
		assert.equal(outcome.action, "reopened");
		await governor.waitReady(sessionId);
		await governor.attach(sessionId);
		const messages = await governor.read({ type: "get_messages", activeSessionId: outcome.incarnation.activeSessionId });
		assert.ok(messages.success);
		assert.match(JSON.stringify(messages.data), /d8-resident-turn/, "the live transcript after reopen holds the pre-crash turn");
		governor.close();
	});

	it("client-owned: with an explicit path the automatic relaunch keeps the transcript; without one it does not (negative control)", async () => {
		const results: Record<string, { messagesAfter: number; onDiskHasTurn: boolean; liveHasTurn: boolean }> = {};
		for (const explicit of [true, false]) {
			const label = explicit ? "with-path" : "without-path";
			const clientId = `cg-d8-owned-${label}`;
			const client = new DaemonClient(fixture.socketPath, { clientId, expected: expectedSubstrate(), wireLog: join(fixture.root, "wire.jsonl") });
			await client.connect();
			const launchEnv = buildLaunchEnv(process.env, { overrides: { HOME: fixture.home, TMPDIR: fixture.tmpDir, PRIME_AGENT_CODING_AGENT_DIR: fixture.agentDir, PRIME_AGENT_TELEMETRY: "0", PRIME_AGENT_INSTALL_UV: "0" } }).env;
			const config = { cwd: fixture.work, agentDir: fixture.agentDir, sessionDir: fixture.sessionDir, provider: "mock", model: "mock-1", noExtensions: true, noSkills: true, noContextFiles: true, noPromptTemplates: true, noThemes: true, telemetryDisabled: true };
			const sessionPath = join(fixture.sessionDir, `d8-owned-${label}.jsonl`);
			const created = await client.request({ type: "create", lifecycle: "client_owned", ...(explicit ? { sessionPath } : {}), config, launchEnv }, `${clientId}-create`, 120_000);
			assert.ok(created.success, `owned create: ${created.success ? "" : created.error}`);
			assert.ok(isSessionSummary(created.data));
			const id = activeSessionIdOf(created.data);
			const pid = created.data.workerPid;
			assert.ok(pid);
			await client.request({ type: "attach", activeSessionId: id, clientId, telemetryDisabled: true, launchEnv }, `${clientId}-attach`);
			await client.request({ type: "prompt", activeSessionId: id, message: `ECHO:d8-owned-${label}` }, `${clientId}-prompt`);
			await client.request({ type: "wait_for_idle", activeSessionId: id }, `${clientId}-idle`, 60_000);
			const file = created.data.sessionFile;
			assert.ok(file, "Prime reports a session file either way");

			process.kill(pid, "SIGKILL");
			// Client-owned workers relaunch automatically under the same active id (Issue #15 D8/s1-05).
			const relaunched = await waitUntil(async () => {
				const list = await client.request({ type: "list", includeClientOwned: true }, `${clientId}-list-${Date.now()}`);
				if (!list.success) return undefined;
				const rows = ((list.data as { sessions?: unknown[] }).sessions ?? (list.data as unknown[])) as unknown[];
				const row = rows.filter(isSessionSummary).find((s) => activeSessionIdOf(s) === id);
				return row && row.workerState === "ready" && row.workerPid !== pid ? row : undefined;
			}, 40_000);
			assert.equal(activeSessionIdOf(relaunched), id, "same active-session id after automatic relaunch");
			const client2 = new DaemonClient(fixture.socketPath, { clientId, expected: expectedSubstrate() });
			await client2.connect();
			await client2.request({ type: "attach", activeSessionId: id, clientId, telemetryDisabled: true, launchEnv }, `${clientId}-attach2`);
			const messages = await client2.request({ type: "get_messages", activeSessionId: id }, `${clientId}-messages`);
			assert.ok(messages.success);
			const live = JSON.stringify(messages.data);
			results[label] = {
				messagesAfter: ((messages.data as { messages?: unknown[] }).messages ?? []).length,
				onDiskHasTurn: readFileSync(file, "utf8").includes(`d8-owned-${label}`),
				liveHasTurn: live.includes(`d8-owned-${label}`),
			};
			await client2.request({ type: "kill", activeSessionId: id }, `${clientId}-kill`);
			client.close();
			client2.close();
		}
		assert.equal(results["with-path"]!.liveHasTurn, true, "explicit path: the relaunched worker resumes the transcript");
		assert.equal(results["without-path"]!.onDiskHasTurn, true, "omitted path: the turn IS on disk ...");
		assert.equal(results["without-path"]!.liveHasTurn, false, "... but the relaunched worker does not see it. This is why the Governor forbids omitting the path.");
	});
});
