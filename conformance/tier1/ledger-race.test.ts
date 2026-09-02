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

	it("six replacements and two resolutions racing on one uncertain record: at most one claim, at most one replacement record, resolution and claim never both pass the check-then-act", async () => {
		const stateDir = mkdtempSync(join(tmpdir(), "cg-race-"));
		const ledger = new MutationLedger(stateDir);
		ledger.recordDispatch({ commandId: "cg-o", clientId: "cg:test", command, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
		ledger.markUncertain("cg-o", "transport_lost");

		const roles = ["supersede", "supersede", "supersede", "supersede", "supersede", "supersede", "resolve_observed", "resolve_absent"];
		const outcomes = await race(stateDir, "cg-o", roles);
		const claims = outcomes.filter((o) => o.role === "supersede");
		const won = claims.filter((o) => o.outcome === "ok");
		assert.ok(won.length <= 1, `at most one replacement may be sent: ${JSON.stringify(outcomes)}`);
		for (const lost of claims.filter((o) => o.outcome !== "ok")) {
			// A loser is refused at the claim (someone else claimed, or a resolution landed first), or, having claimed,
			// finds at the confirm that a resolution landed in between: then its replacement is marked never sent.
			assert.ok(lost.code === "already_superseded" || lost.code === "supersedes_not_uncertain" || lost.code === "claim_lost", `a losing supersede is refused, not last-writer-wins: ${JSON.stringify(lost)}`);
		}
		const replacements = ledger.list().filter((r) => r.supersedes === "cg-o");
		const sendable = replacements.filter((r) => r.state === "DISPATCHED");
		assert.equal(sendable.length, won.length, "exactly the winning replacement is sendable, no other");
		for (const never of replacements.filter((r) => r.state !== "DISPATCHED")) {
			assert.equal(never.state, "FAILED");
			assert.ok(never.transitions[never.transitions.length - 1]!.neverSent, `a replacement that lost at the confirm is marked never sent: ${never.commandId}`);
			assert.ok(claims.some((o) => o.code === "claim_lost" && o.tag === never.commandId));
		}
		const final = ledger.require("cg-o");
		const history = ledger.history("cg-o");
		const claimVersion = history.findIndex((v) => v.supersededBy !== undefined);
		const confirmVersion = history.findIndex((v) => v.supersededBy?.confirmedAt !== undefined);
		const resolvedVersion = history.findIndex((v) => v.state !== "UNCERTAIN" && v.state !== "DISPATCHED");
		if (won.length === 1) {
			assert.equal(final.supersededBy?.commandId, won[0]!.tag);
			assert.ok(final.supersededBy?.confirmedAt, "the winner's claim is confirmed");
			// Claim and confirm both landed while the record was still UNCERTAIN: strictly before any resolution.
			if (resolvedVersion >= 0) assert.ok(confirmVersion < resolvedVersion, `confirm v${confirmVersion + 1} precedes resolution v${resolvedVersion + 1}`);
		} else {
			assert.ok(resolvedVersion >= 0, "if no replacement is sendable, a resolution must have landed (before any claim, or between a claim and its confirm)");
			if (claimVersion >= 0) assert.ok(claimVersion < resolvedVersion && final.supersededBy?.confirmedAt === undefined, "an unconfirmed claim overtaken by a resolution stays unconfirmed on the resolved record");
		}
		const resolvers = outcomes.filter((o) => o.role.startsWith("resolve"));
		assert.ok(resolvers.filter((o) => o.outcome === "ok").length <= 1);
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
