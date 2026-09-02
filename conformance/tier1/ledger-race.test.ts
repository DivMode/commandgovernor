/**
 * D2 (real processes) — many Governors writing one mutation record at the
 * same instant: no state regression, no lost evidence, exactly one legal
 * conflicting resolution, and one adoption.
 *
 * The contenders are real child processes released by a filesystem barrier,
 * so the interleavings are whatever the kernel produces; the assertions hold
 * for every one of them because each write is a compare-and-swap on the
 * record's version. `ledger-cas.test.ts` stages the specific interleavings
 * deterministically; this file is the non-deterministic complement.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { MutationLedger } from "../../governor/mutation/ledger.ts";
import { REPO_ROOT } from "../lib/repo.ts";

type Outcome = { tag: string; role: string; outcome: string; code?: string; state?: string; version?: number };

const command = { type: "execute_bash_and_wait", activeSessionId: "a", command: "true" };

async function race(stateDir: string, commandId: string, roles: string[]): Promise<Outcome[]> {
	const goFile = join(stateDir, `go-${commandId}`);
	const child = join(REPO_ROOT, "conformance", "lib", "ledger-race-child.ts");
	return new Promise<Outcome[]>((resolve, reject) => {
		const results: Outcome[] = [];
		let running = roles.length;
		const procs = roles.map((role, index) => spawn(process.execPath, [child, stateDir, goFile, commandId, role, `${role}-${index}`], { stdio: ["ignore", "pipe", "inherit"] }));
		for (const proc of procs) {
			let out = "";
			proc.stdout.on("data", (chunk: Buffer) => (out += chunk.toString("utf8")));
			proc.once("exit", (code) => {
				if (code !== 0) reject(new Error(`race child exited ${String(code)}`));
				results.push(JSON.parse(out.trim()) as Outcome);
				running -= 1;
				if (running === 0) resolve(results);
			});
		}
		setTimeout(() => writeFileSync(goFile, "go"), 400); // release once every contender is spinning
	});
}

describe("D2: real processes racing on one record", () => {
	it("probes, observed and absent resolutions at once: one resolution wins, every probe survives, nothing regresses", async () => {
		const stateDir = mkdtempSync(join(tmpdir(), "cg-race-"));
		const ledger = new MutationLedger(stateDir);
		ledger.recordDispatch({ commandId: "cg-r", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		ledger.markUncertain("cg-r", "transport_lost");

		const roles = ["probe", "probe", "probe", "probe", "resolve_observed", "resolve_observed", "resolve_absent", "resolve_absent"];
		const outcomes = await race(stateDir, "cg-r", roles);

		const resolvers = outcomes.filter((o) => o.role.startsWith("resolve"));
		const won = resolvers.filter((o) => o.outcome === "ok");
		assert.equal(won.length, 1, `exactly one resolution succeeded: ${JSON.stringify(outcomes)}`);
		for (const lost of resolvers.filter((o) => o.outcome !== "ok")) {
			assert.equal(lost.code, "illegal_transition", `a losing resolver is refused, not last-writer-wins: ${JSON.stringify(lost)}`);
		}
		const expected = won[0]!.role === "resolve_observed" ? "COMPLETED" : "FAILED";
		const final = ledger.require("cg-r");
		assert.equal(final.state, expected, "the final state is the winner's");
		assert.equal(final.transitions.filter((t) => t.evidence).length, 1, "exactly one resolution transition exists");
		assert.equal(final.transitions.find((t) => t.evidence)?.evidence?.by, won[0]!.tag, "and it is the winner's evidence");

		const probes = outcomes.filter((o) => o.role === "probe");
		assert.ok(probes.every((o) => o.outcome === "ok"), `every probe landed: ${JSON.stringify(probes)}`);
		assert.deepEqual(new Set(final.probes?.map((p) => p.detail)), new Set(probes.map((p) => `probe ${p.tag}`)), "every probe is on the final record");

		const history = ledger.history("cg-r");
		assert.equal(history.length, 2 + probes.length + 1, "one version per successful write, none lost");
		assert.deepEqual(history.map((v) => v.version), history.map((_, i) => i + 1), "versions are contiguous");
		const states = history.map((v) => v.state);
		const firstResolved = states.findIndex((s) => s === expected);
		assert.ok(firstResolved >= 2);
		assert.ok(states.slice(firstResolved).every((s) => s === expected), `no version after the resolution regresses: ${states.join(">")}`);
		assert.ok(states.slice(0, firstResolved).every((s, i) => (i === 0 ? s === "DISPATCHED" : s === "UNCERTAIN")));
	});

	it("many adopters of one abandoned record: exactly one adoption transition", async () => {
		const stateDir = mkdtempSync(join(tmpdir(), "cg-race-"));
		// A dispatcher that is certainly over: a pid the kernel has already reaped, with a start id nothing alive will match.
		const reaped = spawn("true", [], { stdio: "ignore" });
		await new Promise<void>((resolve) => reaped.once("exit", () => resolve()));
		const dead = new MutationLedger(stateDir, { self: { pid: reaped.pid!, processStartId: "start:reaped-for-the-test" } });
		dead.recordDispatch({ commandId: "cg-a", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });

		const outcomes = await race(stateDir, "cg-a", ["adopt", "adopt", "adopt", "adopt", "adopt", "adopt"]);
		assert.equal(outcomes.filter((o) => o.outcome === "ok").length, 1, `one adopter adopted: ${JSON.stringify(outcomes)}`);
		assert.ok(outcomes.filter((o) => o.outcome === "not_adopted").length === 5);
		const final = new MutationLedger(stateDir).require("cg-a");
		assert.equal(final.state, "UNCERTAIN");
		assert.equal(final.version, 2);
		assert.equal(final.transitions.filter((t) => t.adoption).length, 1);
	});
});
