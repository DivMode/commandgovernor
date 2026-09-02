/**
 * A child process for the client-identity race test: waits for a "go" file
 * so every contender starts as close to simultaneously as a filesystem
 * barrier allows, then performs first initialisation of the identity in the
 * given state directory and prints what it got.
 *
 *   node client-identity-race-child.ts <stateDir> <goFile>
 *
 * Output: one JSON line `{ "clientId": ..., "created": ... }`.
 */

import { existsSync } from "node:fs";

import { loadOrCreateClientIdentity } from "../../governor/prime/client-identity.ts";

const [stateDir, goFile] = process.argv.slice(2);
if (!stateDir || !goFile) {
	console.error("usage: client-identity-race-child.ts <stateDir> <goFile>");
	process.exit(2);
}

const deadline = Date.now() + 10_000;
while (!existsSync(goFile)) {
	if (Date.now() > deadline) {
		console.error("no go file");
		process.exit(3);
	}
	// A tight spin keeps the contenders within microseconds of each other once the file appears.
}

const loaded = loadOrCreateClientIdentity(stateDir);
process.stdout.write(`${JSON.stringify({ clientId: loaded.record.clientId, created: loaded.created })}\n`);
