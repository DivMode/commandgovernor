/**
 * A child process for the ledger race test: waits for a "go" file so every
 * contender starts as close to simultaneously as a filesystem barrier
 * allows, then performs ONE ledger operation on the named record and prints
 * what happened.
 *
 *   node ledger-race-child.ts <stateDir> <goFile> <commandId> <role> <tag>
 *
 * Roles: probe | resolve_observed | resolve_absent | adopt
 * Output: one JSON line `{ tag, role, outcome, code?, state?, version? }`.
 */

import { existsSync } from "node:fs";

import { MutationLedger, MutationLedgerError } from "../../governor/mutation/ledger.ts";

const [stateDir, goFile, commandId, role, tag] = process.argv.slice(2);
if (!stateDir || !goFile || !commandId || !role || !tag) {
	console.error("usage: ledger-race-child.ts <stateDir> <goFile> <commandId> <role> <tag>");
	process.exit(2);
}

const ledger = new MutationLedger(stateDir);

const deadline = Date.now() + 10_000;
while (!existsSync(goFile)) {
	if (Date.now() > deadline) {
		console.error("no go file");
		process.exit(3);
	}
	// A tight spin keeps the contenders within microseconds of each other once the file appears.
}

let line: Record<string, unknown>;
try {
	let record;
	switch (role) {
		case "probe":
			record = ledger.recordProbe(commandId, { detail: `probe ${tag}` });
			break;
		case "resolve_observed":
			record = ledger.resolveUncertain(commandId, { kind: "effect_observed", by: tag, detail: "observed", observedAt: new Date().toISOString() });
			break;
		case "resolve_absent":
			record = ledger.resolveUncertain(commandId, { kind: "effect_absent_proven", by: tag, detail: "absent", observedAt: new Date().toISOString() });
			break;
		case "adopt": {
			const report = ledger.adoptAbandoned();
			record = report.adopted.find((r) => r.commandId === commandId);
			line = { tag, role, outcome: record ? "ok" : "not_adopted", state: record?.state, version: record?.version };
			break;
		}
		default:
			console.error(`unknown role ${role}`);
			process.exit(2);
	}
	line ??= { tag, role, outcome: "ok", state: record?.state, version: record?.version };
} catch (error) {
	line = { tag, role, outcome: "error", code: error instanceof MutationLedgerError ? error.code : String(error) };
}
process.stdout.write(`${JSON.stringify(line)}\n`);
