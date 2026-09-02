/**
 * A child process for the Governor crash-recovery test: builds a Governor
 * over the given state directory, connects to the given (fake) daemon
 * socket, registers a session, and dispatches one mutating command whose
 * response will never come. It prints one JSON line once the DISPATCHED
 * record is on disk and then waits to be killed.
 *
 *   node governor-crash-child.ts <stateDir> <socketPath> <root>
 *
 * Output: `{ "pid": ..., "clientId": ..., "ownerToken": ... }`.
 */

import { join } from "node:path";

import { Governor } from "../../governor/governor.ts";

const [stateDir, socketPath, root] = process.argv.slice(2);
if (!stateDir || !socketPath || !root) {
	console.error("usage: governor-crash-child.ts <stateDir> <socketPath> <root>");
	process.exit(2);
}

const governor = new Governor({
	stateDir,
	socketPath,
	agentDir: join(root, "agent"),
	home: join(root, "home"),
	tmpDir: root,
	sessionDir: join(root, "sessions"),
	cwd: root,
	provider: "mock",
	model: "mock-1",
	sourceEnv: { PATH: process.env.PATH },
});
await governor.connect(5000);
governor.registry.create({ sessionId: "crash-session", sessionPath: join(root, "sessions", "crash.jsonl") as never, lifecycle: "resident", activeSessionId: "active-0", openedBy: governor.ownerToken });
// The record is written synchronously before the envelope goes out; the promise never settles.
void governor.dispatchMutation("crash-session", "active-0", { type: "execute_bash_and_wait", command: "echo effect >> /nowhere" }, { timeoutMs: 600_000 });
process.stdout.write(`${JSON.stringify({ pid: process.pid, clientId: governor.clientId, ownerToken: governor.ownerToken })}\n`);
setInterval(() => {}, 1_000); // stay alive until killed
