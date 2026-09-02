/**
 * A child process for the registry race test: waits for a "go" file, then
 * performs ONE registry write on the named session and prints what happened.
 *
 *   node registry-race-child.ts <stateDir> <goFile> <sessionId> <role> <tag>
 *
 * Roles: bind (bind generation `gen-<tag>` to incarnation A) | append (append incarnation `<tag>`)
 * Output: one JSON line `{ tag, role, outcome, version?, index? }`.
 */

import { existsSync } from "node:fs";

import { SessionRegistry } from "../../governor/session/registry.ts";

const [stateDir, goFile, sessionId, role, tag] = process.argv.slice(2);
if (!stateDir || !goFile || !sessionId || !role || !tag) {
	console.error("usage: registry-race-child.ts <stateDir> <goFile> <sessionId> <role> <tag>");
	process.exit(2);
}

const registry = new SessionRegistry(stateDir);

const deadline = Date.now() + 10_000;
while (!existsSync(goFile)) {
	if (Date.now() > deadline) {
		console.error("no go file");
		process.exit(3);
	}
}

let line: Record<string, unknown>;
try {
	if (role === "bind") {
		const incarnation = registry.recordGeneration(sessionId, "A", `gen-${tag}`);
		line = { tag, role, outcome: incarnation.generation === `gen-${tag}` ? "ok" : "lost", version: registry.require(sessionId).version };
	} else if (role === "append") {
		const { incarnation, appended, record } = registry.recordIncarnation({ sessionId, activeSessionId: tag, cause: "reopen", openedBy: tag });
		line = { tag, role, outcome: appended ? "ok" : "not_appended", version: record.version, index: incarnation.index };
	} else {
		console.error(`unknown role ${role}`);
		process.exit(2);
	}
} catch (error) {
	line = { tag, role, outcome: "error", detail: String(error) };
}
process.stdout.write(`${JSON.stringify(line)}\n`);
