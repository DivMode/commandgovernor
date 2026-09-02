/**
 * PROTO — the Governor's copy of Prime's protocol facts is exactly what the
 * pinned build ships, and the facts the D2 proof matrix rests on are still
 * true of the pinned source.
 *
 * `governor/prime/protocol.ts` restates two things from Prime source: the set
 * of read-only (unjournaled) commands, and the closed error-code vocabulary.
 * `governor/mutation/proof.ts` restates a third: for each reviewed
 * `(commandType, code)` pair, WHERE the throw is relative to the command's
 * external effect. All three are loaded here from the pinned package's own
 * modules and compared. A re-pin that widens the vocabulary, moves a command
 * across the read/mutating line, or moves a throw past an effect fails this
 * test before it can weaken the guard.
 *
 * Deliberately NOT asserted: that the serializer's vocabulary equals
 * pre-effect proof. Those are different concepts (PR #18 review MUST-FIX 1).
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { PRE_EFFECT_PROOF_MATRIX, REVIEWED_PROOFS } from "../../governor/mutation/proof.ts";
import { DAEMON_ERROR_CODES, READ_ONLY_DAEMON_COMMANDS, SERIALIZED_ERROR_CODES, commandKind } from "../../governor/prime/protocol.ts";
import { readPins, REPO_ROOT } from "../lib/repo.ts";

const installRoot = join(REPO_ROOT, readPins().substrate.installRoot, "node_modules", "prime-agent");
const dist = (...parts: string[]) => readFileSync(join(installRoot, "dist", ...parts), "utf8");

/** The body of the first method/function named `name` in `source`, by brace matching from its opening brace. */
function bodyOf(source: string, name: string): string {
	const declaration = new RegExp(`(?:async\\s+)?(?:function\\s+)?${name}\\s*\\([^)]*\\)\\s*\\{`);
	const match = declaration.exec(source);
	assert.ok(match, `pinned source has no ${name}(...) {`);
	let depth = 0;
	for (let i = match.index + match[0].length - 1; i < source.length; i += 1) {
		const char = source[i];
		if (char === "{") depth += 1;
		if (char === "}") {
			depth -= 1;
			if (depth === 0) return source.slice(match.index, i + 1);
		}
	}
	throw new Error(`unbalanced braces after ${name}`);
}

function indexOfOrFail(haystack: string, needle: string | RegExp, what: string): number {
	const index = typeof needle === "string" ? haystack.indexOf(needle) : haystack.search(needle);
	assert.ok(index >= 0, `${what}: ${String(needle)} not found`);
	return index;
}

describe("PROTO: the protocol facts the Governor relies on match the pinned build", () => {
	it("read-only command set equals the pinned isDaemonMutatingCommand", async () => {
		const mod = (await import(pathToFileURL(join(installRoot, "dist", "modes", "daemon", "daemon-protocol.js")).href)) as {
			isDaemonMutatingCommand(command: { type: string }): boolean;
			DAEMON_COMMAND_COMPATIBILITY?: Record<string, unknown>;
		};
		assert.equal(typeof mod.isDaemonMutatingCommand, "function");
		const allCommands = Object.keys(mod.DAEMON_COMMAND_COMPATIBILITY ?? {});
		assert.ok(allCommands.length > 40, `expected the pinned command table, got ${allCommands.length} entries`);
		const pinnedReadOnly = allCommands.filter((type) => !mod.isDaemonMutatingCommand({ type })).sort();
		const ours = [...READ_ONLY_DAEMON_COMMANDS].sort();
		assert.deepEqual(ours, pinnedReadOnly.filter((t) => t !== "daemon_hello"), "READ_ONLY_DAEMON_COMMANDS drifted from the pin");
		for (const type of allCommands) {
			if (type === "daemon_hello") continue;
			assert.equal(commandKind(type), mod.isDaemonMutatingCommand({ type }) ? "mutating" : "read", type);
		}
		assert.equal(commandKind("execute_bash_and_wait"), "mutating");
		assert.equal(commandKind("create"), "mutating");
		assert.equal(commandKind("import_jsonl"), "mutating");
		assert.equal(commandKind("prompt"), "mutating");
		assert.equal(commandKind("list"), "read");
		// Every reviewed pair names a real, mutating, journaled command of the pin.
		for (const review of REVIEWED_PROOFS) {
			assert.ok(allCommands.includes(review.commandType), `${review.commandType} is not a pinned daemon command`);
			assert.equal(mod.isDaemonMutatingCommand({ type: review.commandType }), true, `${review.commandType} is not mutating; a proof for it is meaningless`);
		}
	});

	it("error-code vocabulary equals the pinned serializer's plus the journal's uncertain code", () => {
		// serializeDaemonError is the ONLY producer of errorInfo in both the supervisor's and the
		// worker's catch paths; the codes it can emit are the codes that exist, plus the journal's own.
		const source = dist("modes", "daemon", "daemon-errors.js");
		const emitted = [...source.matchAll(/code:\s*"([a-z_]+)"/g)].map((m) => m[1]!);
		const protocolSource = dist("modes", "daemon", "daemon-protocol.d.ts");
		const declared = [...protocolSource.matchAll(/code:\s*"([a-z_]+)"/g)].map((m) => m[1]!);
		const pinned = [...new Set([...emitted, ...declared])].sort();
		assert.deepEqual([...DAEMON_ERROR_CODES].sort(), pinned, "DAEMON_ERROR_CODES drifted from the pin");
		assert.ok(declared.includes("command_result_uncertain"));
		assert.ok(!emitted.includes("command_result_uncertain"), "the uncertain code is emitted by the journal path, not the serializer");
		assert.deepEqual([...SERIALIZED_ERROR_CODES].sort(), [...new Set(emitted)].sort(), "SERIALIZED_ERROR_CODES is exactly what serializeDaemonError can produce");
		assert.ok(!SERIALIZED_ERROR_CODES.has("command_result_uncertain"));
	});

	it("typed codes are relayed from WORKERS too, so a code is not evidence of a supervisor-side rejection", () => {
		// The premise the pre-review classifier rested on -- "serializeDaemonError runs only in the
		// supervisor, before a worker sees the command" -- is false at the pin: the worker's own
		// command catch path serialises typed codes for handlers that threw after acting.
		const worker = dist("modes", "daemon", "daemon-mode.js");
		assert.match(worker, /import \{[^}]*serializeDaemonError[^}]*\} from "\.\/daemon-errors\.js"/, "the worker imports the serializer");
		assert.match(worker, /this\.write\(client, failure\(command\.id, command\.type, error, serializeDaemonError\(error\)\)\)/, "the worker's catch path serialises typed codes");
		assert.match(worker, /case "import_jsonl":[\s\S]{0,400}state\.runtime\.importFromJsonl\(command\.inputPath, command\.cwdOverride\)/, "import_jsonl is handled in the worker");
	});

	it("the reviewed proof matrix's source facts still hold at the pin", () => {
		const runtime = dist("core", "agent-session-runtime.js");
		const importBody = bodyOf(runtime, "importFromJsonl");

		// import_jsonl + session_import_file_not_found is PRE-effect: the existence check is the first statement.
		const notFoundThrow = indexOfOrFail(importBody, "throw new SessionImportFileNotFoundError(resolvedPath)", "importFromJsonl");
		const firstMutation = Math.min(
			indexOfOrFail(importBody, "mkdirSync(", "importFromJsonl"),
			indexOfOrFail(importBody, "copyFileSync(", "importFromJsonl"),
			indexOfOrFail(importBody, "acquireReplacementLease(", "importFromJsonl"),
		);
		assert.ok(notFoundThrow < firstMutation, "SessionImportFileNotFoundError is thrown before any filesystem mutation in importFromJsonl");
		assert.equal(PRE_EFFECT_PROOF_MATRIX.lookup("import_jsonl", "session_import_file_not_found")?.timing, "pre_effect");

		// import_jsonl + missing_session_cwd is POST-effect: the copy precedes the cwd check, and nothing between them undoes it.
		const copy = indexOfOrFail(importBody, "copyFileSync(resolvedPath, destinationPath)", "importFromJsonl");
		const cwdCheck = indexOfOrFail(importBody, "assertSessionCwdExists(sessionManager, this.cwd)", "importFromJsonl");
		assert.ok(copy < cwdCheck, "copyFileSync runs before assertSessionCwdExists in importFromJsonl");
		const between = importBody.slice(copy, cwdCheck);
		assert.ok(!/unlinkSync|rmSync|rm\(/.test(between), "no cleanup of the copy between the copy and the cwd check");
		const catchClause = importBody.slice(cwdCheck);
		assert.ok(!/unlinkSync\(destinationPath\)|rmSync\(destinationPath/.test(catchClause), "the catch path releases the lease but does not remove the copied transcript");
		assert.equal(PRE_EFFECT_PROOF_MATRIX.lookup("import_jsonl", "missing_session_cwd")?.timing, "post_effect");
		// And the same class is what the serializer types as missing_session_cwd.
		assert.match(dist("core", "session-cwd.js"), /export function assertSessionCwdExists[\s\S]{0,200}throw new MissingSessionCwdError\(issue\)/);
		assert.match(dist("modes", "daemon", "daemon-errors.js"), /error instanceof MissingSessionCwdError[\s\S]{0,80}code: "missing_session_cwd"/);

		// create + session_already_active is AMBIGUOUS: the supervisor's reuse path throws it before launchWorker, but the
		// worker throws the same class from acquireSessionLease inside createRuntime -- after launchWorker spawned it and after
		// reclaimStaleLease may have renamed/removed another process's lease directory -- and the supervisor re-serialises it
		// identically (independent review of 50762f4 produced the worker-side response against the pinned daemon).
		const supervisor = dist("modes", "daemon", "daemon-supervisor.js");
		const createBody = bodyOf(supervisor, "createOrReuseWorker");
		const reuse = indexOfOrFail(createBody, "return this.reuseWorkerForCreate(", "createOrReuseWorker");
		const launch = indexOfOrFail(createBody, "this.launchWorker(", "createOrReuseWorker");
		assert.ok(reuse < launch, "reuseWorkerForCreate is consulted before launchWorker in createOrReuseWorker");
		assert.match(bodyOf(supervisor, "reuseWorkerForCreate"), /throw new SessionAlreadyActiveError\(sessionPath, worker\.descriptor\.rootActiveSessionId\)/);
		const sessionLease = dist("core", "session-lease.js");
		const acquireBody = bodyOf(sessionLease, "acquireSessionLease");
		assert.match(acquireBody, /throw new SessionAlreadyActiveError\(/, "the worker-side site: acquireSessionLease throws the same class");
		assert.match(acquireBody, /reclaimStaleLease\(/, "and may reclaim (rename/remove) a stale lease directory before it does");
		const worker = dist("modes", "daemon", "daemon-mode.js");
		assert.match(bodyOf(worker, "createRuntime"), /acquireSessionLease\(/, "the worker's createRuntime acquires the session lease");
		assert.match(bodyOf(supervisor, "launchWorker"), /throw deserializeDaemonError\(response\)/, "launchWorker rethrows the worker's typed failure, which the supervisor catch re-serialises");
		assert.equal(PRE_EFFECT_PROOF_MATRIX.lookup("create", "session_already_active")?.timing, "ambiguous");

		// Nothing else is reviewed. If a pair is added, its source fact must be pinned above.
		assert.equal(REVIEWED_PROOFS.length, 3, "a new reviewed pair needs its own source-fact assertion in this test");
	});

	it("the D2 defect is still present in the pinned supervisor (so the Governor guard is still load-bearing)", () => {
		// The supervisor's catch path records the failure into the journal unless the error is a stale
		// supervisor generation. If a future pin adds an exclusion for worker transport loss, this
		// assertion fails and the guard can be re-evaluated deliberately rather than kept by inertia.
		const supervisor = dist("modes", "daemon", "daemon-supervisor.js");
		assert.match(supervisor, /if \(journalIdentity && !isSupervisorGenerationStale\(error\)\)/, "the journal-on-catch condition changed; re-read D2 against the new pin");
		const workerClient = dist("modes", "daemon", "daemon-worker-client.js");
		assert.match(workerClient, /new Error\("Daemon worker socket closed"\)/, "worker socket loss is still a bare, untyped Error");
	});
});
