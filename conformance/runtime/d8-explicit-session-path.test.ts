/**
 * D8 — every stock client that persists a session gives it a durable path.
 *
 * The bake-off's D8 hazard was a raw `create` command with no `sessionPath`,
 * which yields a persistent session with no transcript on disk. This file asks
 * the product question instead: driving only the clients a user actually runs,
 * does any of them produce a persistent session without a durable JSONL, and
 * does that JSONL survive total process loss?
 *
 * Asserted (each phrased against stock client behaviour only):
 *
 *   1. For the TUI, `-p`, `--mode json` and `--mode rpc`: after one turn,
 *      exactly one new `*.jsonl` exists under the session directory and
 *      contains that turn.
 *   2. `--no-session` creates none, and is the only flag that does.
 *   3. `prime-agent list --json` reports a `sessionFile` that exists on disk.
 *   4. After SIGKILL of both worker and supervisor, `prime-agent -r <that
 *      file>` reopens a session whose `sessionId` and `sessionFile` are
 *      unchanged.
 *
 * This file also carries the one runtime check on the pin record itself: the
 * `daemon_hello` a LIVE pinned supervisor sends must match
 * `substrate.daemonProtocol` in `pins/pins.json`. A manifest that records a
 * protocol nothing speaks is a pin in name only, and no static check can catch
 * that.
 *
 * The whole experiment runs once in `before`; the `it` blocks read what it
 * recorded. Driving a real daemon inside each assertion would multiply the
 * suite's wall time by the number of assertions and make every one of them a
 * different run.
 *
 * Every stock client this file starts is KILLED before the total-loss phase.
 * A client left running ensures a new daemon the instant the supervisor dies,
 * which silently turns the "no supervisor is running" phase into a different
 * experiment (measured: `prime-agent list` succeeded because the still-live
 * rpc frontend had already brought a replacement supervisor up).
 */

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import {
	daemonRequest,
	jsonlFiles,
	listAgents,
	listWorkerDescriptors,
	reopenSaved,
	sleep,
	startResidentSession,
	startRoot,
	STOCK_CLIENT_FLAGS,
	waitUntil,
	type DaemonHello,
	type PrimeRoot,
	type SessionRow,
	type WorkerDescriptor,
} from "../lib/prime.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

interface ClientObservation {
	readonly surface: string;
	readonly status: number | null;
	readonly newTranscripts: string[];
	readonly containsTurn: boolean;
	readonly stdout: string;
}

let fixture: PrimeRoot;
let hello: DaemonHello;
let resident: SessionRow;
let descriptor: WorkerDescriptor | undefined;
let listedRow: SessionRow | undefined;
const clients: ClientObservation[] = [];
let noSessionTranscripts: string[] = [];
let noSessionStdout = "";
let deadListStatus: number | null = null;
let deadListOutput = "";
let revived: SessionRow;
let revivedRecords = 0;
let reopenCrashes: { attempt: number; tail: string }[] = [];

/** One stock non-interactive client run, recorded as an observation. */
function runClient(surface: string, args: readonly string[], marker: string): ClientObservation {
	const seen = new Set(jsonlFiles(fixture.sessionDir));
	const result = fixture.cli(args, { timeout: 240_000 });
	const newTranscripts = jsonlFiles(fixture.sessionDir).filter((path) => !seen.has(path));
	const containsTurn = newTranscripts.some((path) => readFileSync(path, "utf8").includes(marker));
	fixture.note("client", surface, "rc", String(result.status), JSON.stringify(newTranscripts.map((path) => path.replace(fixture.root, "<root>"))));
	return { surface, status: result.status, newTranscripts, containsTurn, stdout: result.stdout };
}

describe("D8: every stock persistent session has an explicit durable path", () => {
	before(async () => {
		fixture = await startRoot({ label: "d8" });

		// The live protocol, read from the supervisor's own greeting.
		hello = (await daemonRequest(fixture.socketPath, { type: "list" })).hello;

		// 1a. The interactive TUI: the only stock client that creates a RESIDENT
		// session, and the one whose transcript must survive the kills below.
		const started = await startResidentSession(fixture, { name: "d8-tui" });
		resident = started.row;
		await started.client.submit("ECHO:d8-tui");
		await waitUntil(
			() => (existsSync(resident.sessionFile as string) && readFileSync(resident.sessionFile as string, "utf8").includes("d8-tui") ? true : undefined),
			180_000,
			250,
			"the TUI turn reaching the transcript",
		);
		// Read while the worker is alive: Prime reclaims the descriptor when it dies.
		descriptor = listWorkerDescriptors(fixture.agentDir).find((entry) => entry.rootSessionId === resident.sessionId);
		listedRow = listAgents(fixture).sessions.find((row) => row.sessionId === resident.sessionId);

		// 1b. print mode, 1c. json mode. Both exit on their own.
		clients.push(runClient("prime-agent -p <prompt>", ["-p", ...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir, "ECHO:d8-print"], "d8-print"));
		clients.push(runClient("prime-agent --mode json <prompt>", ["--mode", "json", ...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir, "ECHO:d8-json"], "d8-json"));

		// 1d. rpc mode, which does not exit on an answer: drive it, then kill it.
		{
			const seen = new Set(jsonlFiles(fixture.sessionDir));
			const rpc = fixture.cliSpawn(["--mode", "rpc", ...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir]);
			let out = "";
			rpc.stdout?.on("data", (data: Buffer) => {
				out += data.toString("utf8");
			});
			rpc.stderr?.on("data", (data: Buffer) => {
				out += data.toString("utf8");
			});
			try {
				await sleep(3000);
				rpc.stdin?.write(`${JSON.stringify({ id: "d8", type: "prompt", message: "ECHO:d8-rpc" })}\n`);
				const created = await waitUntil(
					() => {
						const fresh = jsonlFiles(fixture.sessionDir).filter((path) => !seen.has(path));
						return fresh.length > 0 && fresh.some((path) => readFileSync(path, "utf8").includes("d8-rpc")) ? fresh : undefined;
					},
					180_000,
					400,
					"the rpc transcript",
				);
				clients.push({ surface: "prime-agent --mode rpc", status: 0, newTranscripts: created, containsTurn: true, stdout: out });
			} finally {
				try {
					rpc.kill("SIGKILL");
				} catch {
					/* already gone */
				}
				await sleep(1500);
			}
		}

		// 2. --no-session, the documented ephemeral path.
		{
			const seen = new Set(jsonlFiles(fixture.sessionDir));
			const result = fixture.cli(["-p", "--no-session", ...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir, "ECHO:d8-nosession"], { timeout: 240_000 });
			noSessionTranscripts = jsonlFiles(fixture.sessionDir).filter((path) => !seen.has(path));
			noSessionStdout = result.stdout;
		}

		// 4. Total process loss, then the stock way back in. The TUI goes FIRST:
		// a live client would bring a replacement supervisor up under us.
		const live = listAgents(fixture).sessions.find((row) => row.sessionId === resident.sessionId);
		assert.ok(live?.workerPid, "the resident worker must be identifiable before it is killed");
		started.client.kill();
		await sleep(2000);
		const supervisorPid = (await daemonRequest(fixture.socketPath, { type: "list" })).hello.supervisorPid;
		assert.ok(supervisorPid, "the supervisor must be identifiable before it is killed");
		process.kill(live.workerPid, "SIGKILL");
		process.kill(supervisorPid, "SIGKILL");
		fixture.note("SIGKILLed worker", String(live.workerPid), "and supervisor", String(supervisorPid));
		await sleep(4000);

		// With no supervisor listening, `list` connects but does not ensure a
		// daemon. The interactive path does, which is why `-r` works from here.
		const deadList = fixture.cli(["list", "--all", "--json"], { timeout: 60_000 });
		deadListStatus = deadList.status;
		deadListOutput = `${deadList.stdout}${deadList.stderr}`.trim().slice(0, 300);

		const reopened = await reopenSaved(fixture, resident.sessionFile as string, resident.sessionId as string, { name: "d8-resume", timeoutMs: 120_000 });
		revived = reopened.row;
		reopenCrashes = reopened.crashes;
		revivedRecords = listAgents(fixture, { all: true }).sessions.filter((row) => row.sessionId === resident.sessionId).length;
		reopened.client.kill();
		await sleep(1500);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("the live supervisor speaks exactly the daemon protocol pins.json records", () => {
		const pinned = fixture.pins.substrate.daemonProtocol;
		assert.equal(hello.protocol.name, pinned.name);
		assert.equal(hello.protocol.version, pinned.version);
		assert.equal(hello.schemaRevision, pinned.schemaRevision, "pins.json records a schema revision the installed daemon does not speak");
		assert.equal(hello.appVersion, fixture.pins.substrate.version, "the running daemon is not the pinned version");
	});

	it("a stock TUI session is given <session-dir>/<sessionId>.jsonl before its first turn", () => {
		assert.ok(resident.sessionId, "the TUI session has no sessionId");
		assert.equal(resident.sessionFile, join(fixture.sessionDir, `${resident.sessionId}.jsonl`));
	});

	it("Prime's own client sends an explicit sessionPath on create", () => {
		assert.ok(descriptor, "no worker descriptor for the resident session");
		assert.equal(descriptor.createCommand?.type, "create");
		assert.equal(descriptor.createCommand?.sessionPath, resident.sessionFile, "the path Prime asked for is the path it reported");
	});

	it("prime-agent list --json reports a sessionFile that exists on disk", () => {
		assert.ok(listedRow, "the resident session is not in `prime-agent list --json`");
		assert.equal(listedRow.sessionFile, resident.sessionFile);
		assert.ok(existsSync(listedRow.sessionFile as string), `${listedRow.sessionFile} does not exist`);
	});

	for (const surface of ["prime-agent -p <prompt>", "prime-agent --mode json <prompt>", "prime-agent --mode rpc"]) {
		it(`${surface} persists exactly one durable transcript containing the turn`, () => {
			const observation = clients.find((entry) => entry.surface === surface);
			assert.ok(observation, `${surface} was never run`);
			assert.equal(observation.newTranscripts.length, 1, JSON.stringify(observation.newTranscripts));
			assert.ok(observation.containsTurn, `${surface}: the new transcript does not contain the turn`);
		});
	}

	it("--no-session is the only stock path that writes no transcript, and it still answers", () => {
		assert.deepEqual(noSessionTranscripts, []);
		assert.match(noSessionStdout, /d8-nosession/);
	});

	it("with no supervisor running and no client alive, `prime-agent list` fails to connect rather than inventing state", () => {
		assert.notEqual(deadListStatus, 0, deadListOutput);
		assert.match(deadListOutput, /daemon|connect|ECONNREFUSED|ENOENT/i, deadListOutput);
	});

	it("the durable transcript survives worker AND supervisor loss", () => {
		assert.ok(existsSync(resident.sessionFile as string));
		assert.match(readFileSync(resident.sessionFile as string, "utf8"), /d8-tui/);
	});

	it("`prime-agent -r <sessionFile>` reopens the same sessionId at the same path", () => {
		assert.equal(revived.sessionId, resident.sessionId);
		assert.equal(revived.sessionFile, resident.sessionFile);
		assert.equal(revivedRecords, 1, "the revival must not fork the logical session");
		// Recorded, not asserted away: the stock client crashes intermittently
		// when the idle supervisor exits between connect and attach (upstream U3).
		if (reopenCrashes.length > 0) fixture.note("the stock resume crashed before it worked:", JSON.stringify(reopenCrashes));
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});
