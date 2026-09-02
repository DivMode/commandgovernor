/**
 * D2 — a typed error code after an external effect (PR #18 review MUST-FIX 1).
 *
 * The pinned Prime has this real path:
 *
 *   import_jsonl
 *     -> worker AgentSessionRuntime.importFromJsonl
 *     -> copyFileSync(resolvedPath, destinationPath)      <- the effect
 *     -> SessionManager.open(...)
 *     -> assertSessionCwdExists(...)
 *     -> MissingSessionCwdError
 *     -> typed errorInfo.code = missing_session_cwd
 *
 * so `missing_session_cwd` arrives AFTER an observable filesystem mutation.
 * A classifier that treats every serialised code as pre-effect proof calls
 * this FAILED, which is false: the transcript is in the session directory.
 *
 * Required: the Governor classifies the result UNCERTAIN with the reviewed
 * post-effect reason. Falsification: the same captured response under the
 * pre-review global-code classifier comes out FAILED. Positive control: the
 * same command with a nonexistent source is a reviewed PRE-effect pair
 * (`session_import_file_not_found`), classifies FAILED with its proof, and
 * leaves nothing in the session directory.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

import { classifyMutationOutcome, DEFAULT_POLICY, LEGACY_GLOBAL_CODE_POLICY } from "../../governor/mutation/classify.ts";
import type { DaemonFailureResponse } from "../../governor/prime/protocol.ts";
import { type PrimeFixture, startPrimeFixture } from "../lib/prime-fixture.ts";

let fixture: PrimeFixture;

before(async () => {
	fixture = await startPrimeFixture();
});

after(async () => {
	await fixture.stop();
});

/** A minimal Prime session transcript whose header records `cwd`. The header is the only line SessionManager.open reads. */
function writeSourceTranscript(path: string, cwd: string): string {
	const header = { type: "session", version: 3, id: `import-${basename(path, ".jsonl")}`, timestamp: new Date().toISOString(), cwd };
	const contents = `${JSON.stringify(header)}\n`;
	writeFileSync(path, contents);
	return contents;
}

describe("D2: import_jsonl + missing_session_cwd is a typed failure AFTER the effect", () => {
	it("Prime copies the transcript, then fails typed; the Governor says UNCERTAIN; the old classifier would have said FAILED", async () => {
		const governor = await fixture.governor("d2-import");
		const created = await governor.createSession({ sessionPath: join(fixture.sessionDir, "d2-import-root.jsonl"), name: "cg-d2-import" });
		const { sessionId } = created.record;
		const activeSessionId = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);

		// Step 1: a source JSONL, outside the session directory, whose embedded cwd does not exist.
		const sourceDir = join(fixture.root, "import-src");
		mkdirSync(sourceDir, { recursive: true });
		const missingCwd = join(fixture.root, "this-directory-does-not-exist");
		assert.equal(existsSync(missingCwd), false);
		const source = join(sourceDir, "orphan-cwd.jsonl");
		const sourceBytes = writeSourceTranscript(source, missingCwd);
		const destination = join(fixture.sessionDir, basename(source));
		assert.equal(existsSync(destination), false, "nothing under the destination name before the command");
		const sessionDirBefore = readdirSync(fixture.sessionDir).sort();

		// Step 2: invoke import_jsonl through the Governor.
		const result = await governor.dispatchMutation(sessionId, activeSessionId, { type: "import_jsonl", inputPath: source }, { timeoutMs: 60_000 });

		// Step 3: Prime copied the destination file before failing.
		assert.equal(existsSync(destination), true, "the transcript was copied into the session directory: an observable external effect");
		assert.equal(readFileSync(destination, "utf8"), sourceBytes, "byte-for-byte the source");
		assert.deepEqual(readdirSync(fixture.sessionDir).sort(), [...sessionDirBefore, basename(source)].sort(), "exactly one new entry in the session directory");

		// Step 4: Prime returned the typed code.
		assert.equal(result.verdict.verdict === "uncertain" ? result.verdict.response?.success : undefined, false);
		const response = (result.verdict.verdict === "uncertain" ? result.verdict.response : undefined) as DaemonFailureResponse | undefined;
		assert.ok(response, "the verdict carries the response");
		assert.equal(response.errorInfo?.code, "missing_session_cwd", `typed missing_session_cwd, got ${JSON.stringify(response)}`);
		assert.equal(response.command, "import_jsonl");

		// Step 5: the Governor classifies UNCERTAIN, for the reviewed post-effect reason, and the ledger agrees.
		assert.equal(result.verdict.verdict, "uncertain");
		assert.equal(result.verdict.verdict === "uncertain" ? result.verdict.reason : undefined, "typed_failure_post_effect");
		assert.equal(result.record.state, "UNCERTAIN");
		assert.equal(result.record.transitions.at(-1)?.uncertainReason, "typed_failure_post_effect");
		assert.deepEqual(governor.awaitingReconciliation().map((r) => r.commandId), [result.record.commandId], "it is on the attention surface");

		// Step 6: the falsifying control -- the pre-review classifier (any serialised code is proof, for any command) says FAILED.
		const legacy = classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response }, LEGACY_GLOBAL_CODE_POLICY);
		assert.equal(legacy.verdict, "failed", "the negative control: the global-code classifier would have called this FAILED with the copy on disk");
		assert.equal(classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response }, DEFAULT_POLICY).verdict, "uncertain");

		// The worker survived the failed import and still serves the original transcript: this was a rejection, not a crash.
		const summary = await governor.findSummary(sessionId);
		assert.equal(summary?.workerState, "ready");
		assert.equal(summary?.sessionFile, created.record.sessionPath, "the live session did not switch to the imported transcript");
		governor.close();
	});

	it("positive control: import_jsonl with a nonexistent source is the reviewed PRE-effect pair and is FAILED with its proof", async () => {
		const governor = await fixture.governor("d2-import-ctrl");
		const created = await governor.createSession({ sessionPath: join(fixture.sessionDir, "d2-import-ctrl-root.jsonl") });
		const { sessionId } = created.record;
		const activeSessionId = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);
		const source = join(fixture.root, "import-src", "never-written.jsonl");
		assert.equal(existsSync(source), false);
		const sessionDirBefore = readdirSync(fixture.sessionDir).sort();
		const result = await governor.dispatchMutation(sessionId, activeSessionId, { type: "import_jsonl", inputPath: source }, { timeoutMs: 60_000 });
		assert.equal(result.verdict.verdict, "failed", JSON.stringify(result.verdict));
		assert.ok(result.verdict.verdict === "failed");
		assert.equal(result.verdict.proof.code, "session_import_file_not_found");
		assert.equal(result.verdict.proof.commandType, "import_jsonl");
		assert.equal(result.verdict.proof.review?.timing, "pre_effect");
		assert.equal(result.record.state, "FAILED");
		assert.deepEqual(readdirSync(fixture.sessionDir).sort(), sessionDirBefore, "no entry appeared in the session directory");
		assert.equal(existsSync(join(fixture.sessionDir, basename(source))), false);
		governor.close();
	});
});
