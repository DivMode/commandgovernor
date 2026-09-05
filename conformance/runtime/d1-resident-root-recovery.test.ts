/**
 * D1 — a resident root whose worker dies is recovered, once, as itself.
 *
 * The bake-off drove the RAW daemon protocol and found that a resident root
 * whose worker is SIGKILLed parks `failed` and is never relaunched, so a naive
 * client that re-`create`s could end up with a duplicate logical root. This
 * file asks the product question: using only `prime-agent` CLI clients, is a
 * session ever lost, duplicated, or silently re-run?
 *
 * Asserted:
 *
 *   1. `prime-agent -r <sessionFile>` reopens it with the SAME `sessionId` and
 *      `sessionFile` and a DIFFERENT `activeSessionId` — Prime's own
 *      durable-session vs process-incarnation distinction — and `list` then
 *      shows exactly one row for that session.
 *   2. Prime writes exactly one `prime-agent.worker_recovery` transcript entry
 *      per worker loss, and the transcript only grows.
 *   3. Two simultaneous `prime-agent -r <same file>` clients converge on ONE
 *      worker and one `list --all` record.
 *   4. Killing the supervisor under a live worker yields a replacement
 *      supervisor on the same socket that adopts the same worker pid and the
 *      same `activeSessionId`, and the session keeps serving work.
 *
 * What the killed root's `workerState` does in the meantime is RECORDED, not
 * asserted. On 0.9.1 the supervisor never returns it to `ready` on its own
 * (measured: `failed failed absent absent …`), but that is upstream behaviour,
 * not a product invariant — a future Prime that relaunched the root without
 * replaying the interrupted work would be fine, and is not a regression this
 * suite should turn red for. The invariants above hold either way, and the
 * timeline is written to the fixture's notes so a change in it is visible.
 *
 * Each experiment gets its OWN fixture root, and the reason is measured rather
 * than tidiness: when one experiment kills the last worker in a root, that
 * supervisor begins an idle shutdown, and the next experiment's TUI connects
 * during that window and gets a session the daemon archives one turn later
 * (observed: the client prints "The daemon stopped this agent session" and the
 * transcript ends with `session_state: archived`). Sharing a root makes the
 * experiments interact through the supervisor's lifecycle; a root each does
 * not, and costs about a second.
 *
 * Within each experiment the work runs once in `before`; the `it` blocks read
 * what it recorded.
 */

import assert from "node:assert/strict";
import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import {
	alive,
	daemonRequest,
	lineCount,
	listAgents,
	ptyCli,
	reopenSaved,
	sessionRow,
	sleep,
	startResidentSession,
	startRoot,
	STOCK_CLIENT_FLAGS,
	toolPrompt,
	waitUntil,
	type PrimeRoot,
	type SessionRow,
} from "../lib/prime.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

interface RecoveryMarker {
	readonly customType?: string;
	readonly content?: string;
	readonly details?: { readonly operations?: string[] };
}

function recoveryMarkers(sessionFile: string): RecoveryMarker[] {
	return readFileSync(sessionFile, "utf8")
		.split("\n")
		.filter(Boolean)
		.map((line) => {
			try {
				return JSON.parse(line) as RecoveryMarker;
			} catch {
				return {};
			}
		})
		.filter((entry) => entry.customType === "prime-agent.worker_recovery");
}

// ---------------------------------------------------------------------------

describe("D1: worker loss on a resident root", () => {
	let fixture: PrimeRoot;
	let session: SessionRow;
	let killedPid = 0;
	const timeline: string[] = [];
	let reopened: SessionRow;
	let liveRows = 0;
	let staleRows = 0;
	let bytesBefore = 0;
	let bytesAfter = 0;
	let transcript = "";
	let markers: RecoveryMarker[] = [];
	let effect = 0;
	let modelCallsBefore = 0;
	let modelCallsAfter = 0;

	before(async () => {
		fixture = await startRoot({ label: "d1-worker-loss" });
		const started = await startResidentSession(fixture, { name: "d1-e1" });
		session = started.row;
		killedPid = session.workerPid as number;

		// The model asks Prime's own tool to append one line and then sleep, so the
		// worker can be killed while the effect is on disk and the turn is
		// demonstrably still in flight.
		const target = join(fixture.work, "d1-effect.txt");
		await started.client.submit(toolPrompt(target, 25));
		await waitUntil(() => (lineCount(target) === 1 ? true : undefined), 600_000, 250, "the tool effect reaching disk");

		bytesBefore = statSync(session.sessionFile as string).size;
		modelCallsBefore = fixture.modelCalls().length;
		assert.ok(alive(killedPid), `the resident worker ${killedPid} died before it could be killed`);
		process.kill(killedPid, "SIGKILL");
		fixture.note("SIGKILL resident worker", String(killedPid));

		// Sampled, not inferred: what does the stock `list` say for seven seconds?
		for (let i = 0; i < 10; i += 1) {
			await sleep(700);
			const sample = sessionRow(fixture, session.sessionId as string);
			timeline.push(sample ? String(sample.workerState) : "absent");
		}
		fixture.note("workerState timeline:", timeline.join(" "));

		const back = await reopenSaved(fixture, session.sessionFile as string, session.sessionId as string, {
			name: "d1-e1-resume",
			excludePid: killedPid,
			timeoutMs: 120_000,
		});
		if (back.crashes.length > 0) fixture.note("the stock resume crashed before it worked:", JSON.stringify(back.crashes));
		reopened = back.row;
		await sleep(5000);

		liveRows = listAgents(fixture).sessions.filter((row) => row.sessionId === session.sessionId).length;
		staleRows = listAgents(fixture).sessions.filter((row) => row.activeSessionId === session.activeSessionId).length;
		bytesAfter = statSync(session.sessionFile as string).size;
		transcript = readFileSync(session.sessionFile as string, "utf8");
		markers = recoveryMarkers(session.sessionFile as string);
		effect = lineCount(target);
		modelCallsAfter = fixture.modelCalls().length;

		back.client.kill();
		started.client.kill();
		await sleep(1500);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("`prime-agent -r <sessionFile>` reopens the SAME durable session as a NEW incarnation", () => {
		assert.equal(reopened.sessionId, session.sessionId, "the durable sessionId changed");
		assert.equal(reopened.sessionFile, session.sessionFile, "the durable transcript path changed");
		assert.notEqual(reopened.activeSessionId, session.activeSessionId, "the reopened root reused the dead incarnation's active-session id");
		assert.notEqual(reopened.workerPid, killedPid, "the reopened root reports the killed worker's pid");
	});

	it("exactly one live root for the logical session, and the dead incarnation is gone", () => {
		assert.equal(liveRows, 1, "the recovery created a duplicate logical root");
		assert.equal(staleRows, 0, "the dead incarnation is still listed as live");
	});

	it("the transcript only grows, and keeps the pre-crash turn", () => {
		assert.ok(bytesAfter >= bytesBefore, `${bytesBefore} -> ${bytesAfter}`);
		assert.match(transcript, /TOOL:ipython/, "the pre-crash turn is missing from the recovered transcript");
	});

	it("Prime writes exactly one worker_recovery entry for one worker loss", () => {
		assert.equal(markers.length, 1, JSON.stringify(markers.map((marker) => marker.details)));
	});

	it("the interrupted tool effect is not replayed, and the model is not re-prompted", () => {
		assert.equal(effect, 1, "the external effect was duplicated by the recovery");
		assert.equal(modelCallsAfter, modelCallsBefore, "the model was asked again after the recovery");
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});

// ---------------------------------------------------------------------------

describe("D1: two stock clients resume the same saved session at once", () => {
	let fixture: PrimeRoot;
	let rows: SessionRow[] = [];
	let allRows = 0;
	let bytesBefore = 0;
	let bytesAfter = 0;
	let loserScreen = "";

	before(async () => {
		fixture = await startRoot({ label: "d1-resume-race" });
		const started = await startResidentSession(fixture, { name: "d1-e2" });
		const session = started.row;
		await started.client.submit("ECHO:d1-race-warm");
		await waitUntil(
			() => (readFileSync(session.sessionFile as string, "utf8").includes("d1-race-warm") ? true : undefined),
			180_000,
			250,
			"the warm turn reaching the transcript",
		);
		bytesBefore = statSync(session.sessionFile as string).size;
		process.kill(session.workerPid as number, "SIGKILL");
		await waitUntil(
			() => ((sessionRow(fixture, session.sessionId as string)?.workerState ?? "absent") !== "ready" ? true : undefined),
			90_000,
			400,
			"the killed worker leaving ready",
		);
		started.client.kill();
		await sleep(1500);

		fixture.note("racing two stock `prime-agent -r <sessionFile>` clients on the same saved session");
		const resumeArgs = [...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir, "-r", session.sessionFile as string];
		const a = ptyCli(fixture, resumeArgs, { name: "d1-e2-a" });
		const b = ptyCli(fixture, resumeArgs, { name: "d1-e2-b" });
		await waitUntil(
			() => (sessionRow(fixture, session.sessionId as string)?.workerState === "ready" ? true : undefined),
			180_000,
			500,
			"one of the racing resumes reopening the session",
		);
		await sleep(12_000);
		rows = listAgents(fixture).sessions.filter((row) => row.sessionId === session.sessionId);
		allRows = listAgents(fixture, { all: true }).sessions.filter((row) => row.sessionId === session.sessionId).length;
		bytesAfter = statSync(session.sessionFile as string).size;
		loserScreen = b.screen().slice(-200).replace(/\s+/g, " ");
		fixture.note(
			"rows after the race:",
			JSON.stringify(rows.map((row) => ({ id: row.activeSessionId, pid: row.workerPid, state: row.workerState, clients: row.attachedClients }))),
		);
		a.kill();
		b.kill();
		await sleep(1500);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("both resumes converge on ONE worker", () => {
		assert.equal(rows.length, 1, JSON.stringify(rows.map((row) => ({ id: row.activeSessionId, pid: row.workerPid, state: row.workerState }))));
		assert.ok((rows[0]?.attachedClients ?? 0) >= 1, `attachedClients=${rows[0]?.attachedClients}; the second client showed: ${loserScreen}`);
	});

	it("`prime-agent list --all` holds exactly one record for that session", () => {
		assert.equal(allRows, 1, "the race forked the logical session");
	});

	it("the transcript was not forked or truncated by the race", () => {
		assert.ok(bytesAfter >= bytesBefore, `${bytesBefore} -> ${bytesAfter}`);
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});

// ---------------------------------------------------------------------------

describe("D1: supervisor loss under a live worker", () => {
	let fixture: PrimeRoot;
	let supervisorBefore = 0;
	let supervisorAfter = 0;
	let workerBefore = 0;
	let activeBefore = "";
	let adopted: SessionRow;
	let servedAfterAdoption = false;

	before(async () => {
		fixture = await startRoot({ label: "d1-supervisor-loss" });
		const started = await startResidentSession(fixture, { name: "d1-e3" });
		const session = started.row;
		await started.client.submit("ECHO:d1-supervisor-warm");
		await waitUntil(
			() => (readFileSync(session.sessionFile as string, "utf8").includes("d1-supervisor-warm") ? true : undefined),
			180_000,
			250,
			"the warm turn reaching the transcript",
		);

		supervisorBefore = (await daemonRequest(fixture.socketPath, { type: "list" })).hello.supervisorPid as number;
		workerBefore = session.workerPid as number;
		activeBefore = session.activeSessionId as string;
		assert.ok(alive(workerBefore), `the resident worker ${workerBefore} died before the supervisor was killed`);
		fixture.note("SIGKILL supervisor", String(supervisorBefore), "under live worker", String(workerBefore));
		process.kill(supervisorBefore, "SIGKILL");

		const replacement = await waitUntil(
			async () => {
				try {
					const { hello } = await daemonRequest(fixture.socketPath, { type: "list" }, { timeoutMs: 5000 });
					return hello && hello.supervisorPid !== supervisorBefore ? hello : undefined;
				} catch {
					return undefined;
				}
			},
			120_000,
			1000,
			"a replacement supervisor on the same socket",
		);
		supervisorAfter = replacement.supervisorPid as number;
		adopted = await waitUntil(
			() => {
				const candidate = sessionRow(fixture, session.sessionId as string);
				return candidate?.workerState === "ready" ? candidate : undefined;
			},
			120_000,
			1000,
			"the adopted session becoming ready again",
		);
		await started.client.submit("ECHO:d1-after-supervisor");
		servedAfterAdoption = await waitUntil(
			() => (readFileSync(session.sessionFile as string, "utf8").includes("d1-after-supervisor") ? true : undefined),
			180_000,
			250,
			"the adopted session serving another turn",
		);
		started.client.kill();
		await sleep(1500);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("a replacement supervisor answers on the same socket", () => {
		assert.notEqual(supervisorAfter, supervisorBefore, "no replacement supervisor answered");
	});

	it("it adopts the live worker and keeps the same incarnation", () => {
		assert.equal(adopted.workerPid, workerBefore, "the replacement supervisor did not adopt the live worker");
		assert.equal(adopted.activeSessionId, activeBefore, "the session lost its incarnation across the supervisor replacement");
	});

	it("the adopted session keeps serving work through the stock TUI", () => {
		assert.ok(servedAfterAdoption);
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});
