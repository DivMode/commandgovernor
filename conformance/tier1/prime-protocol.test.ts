/**
 * PROTO — the Governor's copy of Prime's protocol facts is exactly what the
 * pinned build ships.
 *
 * `governor/prime/protocol.ts` restates two things from Prime source: the set
 * of read-only (unjournaled) commands, and the closed error-code vocabulary
 * whose pre-effect subset is the whole basis of the D2 classifier. Both are
 * loaded here from the pinned package's own module and compared. A re-pin
 * that widens the error vocabulary, or moves a command across the
 * read/mutating line, fails this test before it can weaken the guard.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { DAEMON_ERROR_CODES, PRE_EFFECT_ERROR_CODES, READ_ONLY_DAEMON_COMMANDS, commandKind } from "../../governor/prime/protocol.ts";
import { readPins, REPO_ROOT } from "../lib/repo.ts";

const installRoot = join(REPO_ROOT, readPins().substrate.installRoot, "node_modules", "prime-agent");

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
		assert.equal(commandKind("prompt"), "mutating");
		assert.equal(commandKind("list"), "read");
	});

	it("error-code vocabulary equals the pinned serializer's, and the pre-effect subset is exactly the create-time errors", () => {
		// serializeDaemonError is the ONLY producer of errorInfo in the supervisor's catch path;
		// the codes it can emit are the codes that exist.
		const source = readFileSync(join(installRoot, "dist", "modes", "daemon", "daemon-errors.js"), "utf8");
		const emitted = [...source.matchAll(/code:\s*"([a-z_]+)"/g)].map((m) => m[1]!);
		const protocolSource = readFileSync(join(installRoot, "dist", "modes", "daemon", "daemon-protocol.d.ts"), "utf8");
		const declared = [...protocolSource.matchAll(/code:\s*"([a-z_]+)"/g)].map((m) => m[1]!);
		const pinned = [...new Set([...emitted, ...declared])].sort();
		assert.deepEqual([...DAEMON_ERROR_CODES].sort(), pinned, "DAEMON_ERROR_CODES drifted from the pin");
		// The uncertain code is emitted by the supervisor's journal path, not the serializer.
		assert.ok(declared.includes("command_result_uncertain"));
		assert.ok(!emitted.includes("command_result_uncertain"));
		assert.deepEqual([...PRE_EFFECT_ERROR_CODES].sort(), emitted.sort(), "pre-effect proof is exactly what serializeDaemonError can produce");
		assert.ok(!PRE_EFFECT_ERROR_CODES.has("command_result_uncertain"));
	});

	it("the D2 defect is still present in the pinned supervisor (so the Governor guard is still load-bearing)", () => {
		// The supervisor's catch path records the failure into the journal unless the error is a stale
		// supervisor generation. If a future pin adds an exclusion for worker transport loss, this
		// assertion fails and the guard can be re-evaluated deliberately rather than kept by inertia.
		const supervisor = readFileSync(join(installRoot, "dist", "modes", "daemon", "daemon-supervisor.js"), "utf8");
		assert.match(supervisor, /if \(journalIdentity && !isSupervisorGenerationStale\(error\)\)/, "the journal-on-catch condition changed; re-read D2 against the new pin");
		const workerClient = readFileSync(join(installRoot, "dist", "modes", "daemon", "daemon-worker-client.js"), "utf8");
		assert.match(workerClient, /new Error\("Daemon worker socket closed"\)/, "worker socket loss is still a bare, untyped Error");
	});
});
