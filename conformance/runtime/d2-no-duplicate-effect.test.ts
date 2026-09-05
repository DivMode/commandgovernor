/**
 * D2 — a worker that dies after an external effect and before it reports never
 * causes that effect to happen twice.
 *
 * The external effect is a real one, produced by the product path rather than
 * by the test: the mock model emits a `tool_calls` chunk for Prime's own
 * `ipython` tool, whose code appends ONE line to a file and then sleeps. The
 * worker is SIGKILLed only after that line is observably on disk. Every model
 * request and every scripted tool call is logged to JSONL, so "was the prompt
 * re-delivered?" and "was the tool re-issued?" are answered from files.
 *
 * Asserted:
 *
 *   1. Effect-once under worker loss on the model-tool path: after
 *      `prime-agent -r <sessionFile>` the file still holds exactly one line,
 *      the model was asked exactly once, and exactly one tool call was issued.
 *   2. A negative control: the same prompt sent twice through the same stock
 *      client produces TWO lines. Without it, assertion 1 would pass on a
 *      counter that cannot see a duplicate.
 *   3. Prime's own recovery marker is present, typed, and model-visible: one
 *      `prime-agent.worker_recovery` transcript entry whose
 *      `details.operations` names the interrupted operation class and whose
 *      text says the work was not replayed.
 *   4. The client-owned RPC path: a stock `--mode rpc` client issuing
 *      `{"type":"bash"}` — the daemon's `execute_bash_and_wait` mutation on the
 *      wire — whose worker dies after the effect gets exactly ONE response, is
 *      told the command did not succeed, and the command is never re-issued.
 *
 * What this file deliberately does NOT assert is the shape of that failure
 * message. The known upstream defect is that the response is an untyped
 * `Daemon worker socket closed` with no `errorInfo`; pinning that string here
 * would make the suite go red the day upstream fixes it. The product invariant
 * is "once, and reported as a failure", and that is what is checked.
 *
 * Each of the three parts gets its own fixture root. A root whose last worker
 * has just been killed starts an idle shutdown, and a client that connects
 * during that window gets a session the daemon archives one turn later — so
 * running the negative control in the root the measurement just finished with
 * makes the control depend on the measurement's supervisor lifecycle.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import {
	alive,
	lineCount,
	listAgents,
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

interface RpcLine {
	readonly id?: string;
	readonly type?: string;
	readonly command?: string;
	readonly success?: boolean;
	readonly error?: string;
}

function parseJsonLines(text: string): RpcLine[] {
	return text
		.split("\n")
		.filter(Boolean)
		.map((line) => {
			try {
				return JSON.parse(line) as RpcLine;
			} catch {
				return undefined;
			}
		})
		.filter((line): line is RpcLine => line !== undefined);
}

// ---------------------------------------------------------------------------

describe("D2: an interrupted tool effect happens exactly once on the resident path", () => {
	let fixture: PrimeRoot;
	let session: SessionRow;
	const timeline: string[] = [];
	let effectAtKill = 0;
	let effectAfterReopen = 0;
	let modelCallsBefore = 0;
	let modelCallsAfter = 0;
	let toolCallCount = 0;
	let markers: RecoveryMarker[] = [];
	let liveRows = 0;
	let reopened: SessionRow;

	before(async () => {
		fixture = await startRoot({ label: "d2-resident" });
		const started = await startResidentSession(fixture, { name: "d2-tui" });
		session = started.row;
		const killed = session.workerPid as number;

		const target = join(fixture.work, "d2-effect.txt");
		await started.client.submit(toolPrompt(target, 25));
		await waitUntil(() => (lineCount(target) === 1 ? true : undefined), 600_000, 250, "the tool effect reaching disk");
		effectAtKill = lineCount(target);

		const live = sessionRow(fixture, session.sessionId as string);
		fixture.note(
			"effect on disk while the tool still runs; isRunningTools =",
			String(live?.isRunningTools),
			"unfinishedActionCount =",
			String(live?.unfinishedActionCount),
		);
		modelCallsBefore = fixture.modelCalls().length;
		assert.ok(alive(killed), `the resident worker ${killed} died before it could be killed`);
		process.kill(killed, "SIGKILL");
		fixture.note("SIGKILL worker", String(killed), "mid tool execution");

		for (let i = 0; i < 8; i += 1) {
			await sleep(700);
			const sample = sessionRow(fixture, session.sessionId as string);
			timeline.push(sample ? String(sample.workerState) : "absent");
		}
		fixture.note("workerState timeline:", timeline.join(" "));

		const back = await reopenSaved(fixture, session.sessionFile as string, session.sessionId as string, {
			name: "d2-reopen",
			excludePid: killed,
			timeoutMs: 120_000,
		});
		reopened = back.row;
		if (back.crashes.length > 0) fixture.note("the stock resume crashed before it worked:", JSON.stringify(back.crashes));
		await sleep(6000);

		effectAfterReopen = lineCount(target);
		modelCallsAfter = fixture.modelCalls().length;
		toolCallCount = fixture.toolCalls().length;
		markers = readFileSync(session.sessionFile as string, "utf8")
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
		liveRows = listAgents(fixture).sessions.filter((row) => row.sessionId === session.sessionId).length;
		fixture.note("supervisor mutation journal entries:", JSON.stringify(fixture.commandJournal()));

		back.client.kill();
		started.client.kill();
		await sleep(1500);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("the killed root is not relaunched behind the user's back", () => {
		assert.ok(!timeline.includes("ready"), timeline.join(","));
	});

	it("the external effect happened exactly once while the worker was dead", () => {
		assert.equal(effectAtKill, 1);
	});

	it("and is still exactly once after the stock reopen", () => {
		assert.equal(effectAfterReopen, 1, "the recovery duplicated the external effect");
		assert.equal(modelCallsAfter, modelCallsBefore, "the model was asked again, so the turn was replayed");
		assert.equal(toolCallCount, 1, "more than one tool call was ever issued");
	});

	it("Prime writes one typed, model-visible recovery marker", () => {
		assert.equal(markers.length, 1, JSON.stringify(markers.map((marker) => marker.details)));
		assert.ok(
			(markers[0]?.details?.operations ?? []).includes("tool_execution_start"),
			`operations = ${JSON.stringify(markers[0]?.details?.operations)}`,
		);
		assert.match(markers[0]?.content ?? "", /was not replayed/, "the marker does not tell the model the work was not replayed");
	});

	it("the reopened session is the same one, and there is exactly one of it", () => {
		assert.equal(reopened.sessionId, session.sessionId);
		assert.equal(reopened.sessionFile, session.sessionFile);
		assert.equal(liveRows, 1);
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});

// ---------------------------------------------------------------------------

describe("D2: negative control — the effect counter can see a duplicate", () => {
	let fixture: PrimeRoot;
	let lines = 0;
	let modelCalls = 0;
	let toolCalls = 0;

	before(async () => {
		fixture = await startRoot({ label: "d2-control" });
		const started = await startResidentSession(fixture, { name: "d2-control" });
		const target = join(fixture.work, "d2-control-effect.txt");

		await started.client.submit(toolPrompt(target, 0));
		await waitUntil(() => (lineCount(target) === 1 ? true : undefined), 600_000, 250, "the first control effect");
		// The turn is closed once the model has answered the tool result; waiting
		// on the mock's own log rather than on a `list` row keeps this independent
		// of whether the daemon still reports the session.
		await waitUntil(
			() => (fixture.mockRequests().filter((entry) => entry.kind === "response" && entry.mode === "after-tool").length >= 1 ? true : undefined),
			120_000,
			300,
			"the first control turn closing",
		);
		await sleep(1500);

		await started.client.submit(toolPrompt(target, 0));
		await waitUntil(() => (lineCount(target) === 2 ? true : undefined), 300_000, 250, "the second control effect");
		lines = lineCount(target);
		modelCalls = fixture.modelCalls().length;
		toolCalls = fixture.toolCalls().length;
		started.client.kill();
		await sleep(1500);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("two deliberate sends of the same prompt produce two lines", () => {
		assert.equal(lines, 2, "the effect counter cannot detect a duplicate, so the effect-once assertions prove nothing");
	});

	it("and the model/tool log shows both", () => {
		assert.ok(modelCalls >= 2, `the control asked the model ${modelCalls} times`);
		assert.equal(toolCalls, 2, `the control issued ${toolCalls} tool calls`);
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});

// ---------------------------------------------------------------------------

describe("D2: a lost RPC bash mutation is reported once and never re-issued", () => {
	let fixture: PrimeRoot;
	let responses: RpcLine[] = [];
	let effectAfterKill = 0;
	let effectLater = 0;
	let rpcOutput = "";

	before(async () => {
		fixture = await startRoot({ label: "d2-rpc" });
		const target = join(fixture.work, "d2-rpc-effect.txt");
		const rpc = fixture.cliSpawn(["--mode", "rpc", ...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir]);
		rpc.stdout?.on("data", (data: Buffer) => {
			rpcOutput += data.toString("utf8");
		});
		rpc.stderr?.on("data", (data: Buffer) => {
			rpcOutput += data.toString("utf8");
		});

		try {
			// Warm up first: the session must exist before the mutation is issued.
			rpc.stdin?.write(`${JSON.stringify({ id: "warm", type: "prompt", message: "ECHO:d2-rpc-warm" })}\n`);
			await waitUntil(() => (rpcOutput.includes('"id":"warm"') ? true : undefined), 240_000, 300, "the rpc warm-up response");
			await sleep(1500);

			rpc.stdin?.write(`${JSON.stringify({ id: "mutate", type: "bash", command: `echo effect >> ${JSON.stringify(target)}; sleep 20` })}\n`);
			await waitUntil(() => (lineCount(target) === 1 ? true : undefined), 180_000, 100, "the bash effect reaching disk");

			// Prime retitles its processes to the bare string "prime-agent", so the
			// session worker is found by walking this root's own process tree rather
			// than by matching a command line.
			const tree = fixture.relatedProcesses();
			fixture.note("process tree while the bash command runs:", JSON.stringify(tree.map((row) => `${row.pid} ${row.command.slice(0, 60)}`)));
			const candidates = tree
				.filter((row) => row.command.trim() === "prime-agent" && row.pid !== fixture.supervisorPid && row.pid !== rpc.pid)
				.map((row) => row.pid)
				.sort((a, b) => b - a);
			assert.ok(candidates.length > 0, `no session worker found in the tree: ${JSON.stringify(tree)}`);
			fixture.note("SIGKILL the session worker", String(candidates[0]), "(rpc frontend is", String(rpc.pid), ")");
			process.kill(candidates[0], "SIGKILL");

			await waitUntil(
				() => (parseJsonLines(rpcOutput).some((line) => line.type === "response" && line.command === "bash") ? true : undefined),
				120_000,
				300,
				"a response for the lost bash command",
			);
			effectAfterKill = lineCount(target);

			// Long enough for any re-issue after a reconnect to have happened, and
			// for the original `sleep 20` to have finished had it survived.
			await sleep(25_000);
			effectLater = lineCount(target);
			responses = parseJsonLines(rpcOutput).filter((line) => line.type === "response" && line.command === "bash");
			fixture.note("bash responses:", JSON.stringify(responses));
		} finally {
			// A stock client left running will ENSURE a new daemon the moment the
			// fixture's supervisor stops, which both defeats teardown and leaks a
			// process into the next test. Kill it, do not merely close its stdin.
			try {
				rpc.kill("SIGKILL");
			} catch {
				/* already gone */
			}
			await sleep(1500);
		}
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("the mutating command's effect happened exactly once", () => {
		assert.equal(effectAfterKill, 1, rpcOutput.slice(0, 600));
	});

	it("exactly one response is delivered for the lost command, and it is not a success", () => {
		assert.equal(responses.length, 1, JSON.stringify(responses));
		assert.equal(responses[0]?.success, false, JSON.stringify(responses[0]));
		assert.ok((responses[0]?.error ?? "").length > 0, "a failed mutation must carry a reason");
	});

	it("Prime never re-issues the command under a new identity", () => {
		assert.equal(effectLater, 1, "the effect was repeated after the worker was lost");
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});
