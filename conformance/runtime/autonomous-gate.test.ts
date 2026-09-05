/**
 * GATE — `--autonomous-gate` is a host-owned acceptance gate that the model
 * cannot pass by claiming success.
 *
 * This is the one Prime-native primitive that enforces Command Governor's
 * product rule — an implementer finishing does not make its work accepted —
 * with no code at all: Prime runs a shell command itself and will not let an
 * autonomous run finish until that command exits 0.
 *
 * The experiment is two runs with an IDENTICAL model script and an identical
 * invocation. In both, the assistant says, in plain language, "I have finished
 * the work and verified it. Everything passes." The only difference is a file
 * the TEST creates between them, which the gate script looks for. If the model
 * could end the run by asserting completion, both runs would look the same.
 *
 * Asserted:
 *   1. the gate command really ran (the script records its own executions);
 *   2. under a failing gate the model's claim does not finish the run — more
 *      than one turn, and a non-zero exit at the turn limit;
 *   3. the failure is fed back to the model, carrying the gate's OWN stdout, so
 *      the next turn is told what was not accepted;
 *   4. once the artifact exists — written by the test, never by the model — the
 *      same command finishes in one turn with exit 0;
 *   5. nothing on the wire lets the model report that a gate passed: the tool
 *      list during the failing run has no gate-shaped tool in it.
 */

import assert from "node:assert/strict";
import { chmodSync, existsSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import { startRoot, STOCK_CLIENT_FLAGS, type MockEntry, type PrimeRoot } from "../lib/prime.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

/**
 * Every discovery mechanism off. Nothing about extensions, skills, prompts or
 * AGENTS.md is this file's subject, and context-file discovery in particular
 * walks UP from the working directory, i.e. out of the fixture.
 */
const FLAGS = [...STOCK_CLIENT_FLAGS, "--no-session"];

/**
 * The model's entire script, used verbatim for BOTH runs. `ECHO:` makes the
 * mock stream this text back as the assistant's answer, so the claim under test
 * is a real assistant message rather than a description of one.
 */
const CLAIM = "ECHO:I have finished the work and verified it. Everything passes.";

let fixture: PrimeRoot;
let gateRunsAfterFailing = 0;
let gateRunsDeltaOnPass = 0;
let failingStatus: number | null = null;
let failingTurns = 0;
let failingFeedback = "";
let failingToolNames: string[] = [];
let failingStderr = "";
let passingStatus: number | null = null;
let passingTurns = 0;
let passingStdout = "";

describe("GATE: an autonomous run is ended by the host's gate, not by the model's claim", () => {
	before(async () => {
		fixture = await startRoot({ label: "autonomous-gate" });
		const project = realpathSync(fixture.work);
		const gateLog = join(fixture.root, "gate-runs.log");
		const artifact = join(project, "REVIEW-ACCEPTED");
		const gate = join(fixture.root, "gate.sh");

		// The host runs this. It is the only thing that can end an autonomous run,
		// and it records its own executions so "did it actually run?" is read from
		// a file rather than inferred from the run's behaviour.
		writeFileSync(
			gate,
			`#!/bin/sh\n` +
				`echo "gate-ran $(date +%s)" >> "${gateLog}"\n` +
				`if [ -f "${artifact}" ]; then\n` +
				`  echo "REVIEW ACCEPTED: reviewer artifact present"\n` +
				`  exit 0\n` +
				`fi\n` +
				`echo "REVIEW NOT ACCEPTED: no reviewer artifact at ${artifact}"\n` +
				`exit 1\n`,
		);
		chmodSync(gate, 0o755);
		writeFileSync(gateLog, "");
		const gateRuns = (): number => (existsSync(gateLog) ? readFileSync(gateLog, "utf8").split("\n").filter(Boolean).length : 0);

		const invocation = (extra: readonly string[]): string[] => [
			"-p",
			...FLAGS,
			"--autonomous",
			"--autonomous-gate",
			gate,
			"--autonomous-max-continuations",
			"3",
			"--autonomous-max-turns",
			"2",
			...extra,
			CLAIM,
		];

		const turnsSince = (mark: number): MockEntry[] => fixture.mockRequests().slice(mark).filter((entry) => entry.kind === "request");

		// ---- run 1: the artifact does not exist, so the gate refuses -----------
		let mark = fixture.mockRequests().length;
		const failing = fixture.cli(invocation(["--autonomous-gate-retries", "2"]), { timeout: 900_000, cwd: project });
		const failingRequests = turnsSince(mark);
		failingStatus = failing.status;
		failingStderr = failing.stderr;
		failingTurns = failingRequests.length;
		gateRunsAfterFailing = gateRuns();
		failingFeedback = failingRequests
			.map((entry) => `${String(entry.system ?? "")}\n${((entry.userMessages as string[] | undefined) ?? []).join("\n")}`)
			.join("\n=====\n");
		failingToolNames = [...new Set(failingRequests.flatMap((entry) => (entry.toolNames as string[] | undefined) ?? []))];
		fixture.note("failing run: exit", String(failing.status), "turns", String(failingTurns), "gate runs", String(gateRunsAfterFailing));

		// ---- run 2: the TEST writes the artifact; the model script is unchanged -
		writeFileSync(artifact, "an independent reviewer produced this artifact\n");
		const runsBeforePass = gateRuns();
		mark = fixture.mockRequests().length;
		const passing = fixture.cli(invocation([]), { timeout: 900_000, cwd: project });
		passingStatus = passing.status;
		passingStdout = passing.stdout;
		passingTurns = turnsSince(mark).length;
		gateRunsDeltaOnPass = gateRuns() - runsBeforePass;
		fixture.note("passing run: exit", String(passing.status), "turns", String(passingTurns), "gate runs delta", String(gateRunsDeltaOnPass));
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("the gate command actually ran", () => {
		assert.ok(gateRunsAfterFailing >= 1, `the gate script recorded ${gateRunsAfterFailing} executions`);
	});

	it("a model claiming the work is finished does not finish the run", () => {
		assert.ok(
			failingTurns > 1,
			`the assistant said "I have finished the work and verified it" and the host issued only ${failingTurns} turn(s); one turn would mean the claim ended the run`,
		);
		assert.notEqual(failingStatus, 0, `the run exited 0 under a failing gate: ${failingStderr.slice(0, 300)}`);
	});

	it("the gate's refusal, and its own stdout, are fed back to the model", () => {
		assert.match(failingFeedback, /Autonomous quality gate failed/i, "the continuation did not tell the model the gate failed");
		assert.match(failingFeedback, /REVIEW NOT ACCEPTED/, "the continuation did not carry the gate's own output");
	});

	it("no tool on the wire lets the model report that a gate passed", () => {
		const suspicious = failingToolNames.filter((name) => /gate|autonomous|accept|approve/i.test(name));
		assert.deepEqual(suspicious, [], `the model was offered ${JSON.stringify(suspicious)}`);
		assert.ok(failingToolNames.length > 0, "the run advertised no tools at all, so this check saw nothing");
	});

	it("the same run finishes once the artifact the gate looks for exists", () => {
		assert.equal(passingStatus, 0, passingStdout.slice(0, 300));
		assert.equal(passingTurns, 1, `${passingTurns} turn(s) once the gate exits 0`);
		assert.ok(gateRunsDeltaOnPass >= 1, "the passing run did not run the gate at all");
	});

	it("the gate is two-sided: an identical model script, opposite outcomes", () => {
		assert.ok(
			passingTurns < failingTurns && passingStatus === 0 && failingStatus !== 0,
			`failing: ${failingTurns} turn(s) exit ${failingStatus}; passing: ${passingTurns} turn(s) exit ${passingStatus}. ` +
				"The only thing that changed between them is a file the test wrote, so the gate decided, not the model.",
		);
		assert.match(passingStdout, /I have finished the work and verified it/, "the passing run did not use the same model script");
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});
