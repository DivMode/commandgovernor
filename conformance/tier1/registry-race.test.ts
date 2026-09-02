/**
 * D1 (real processes) — many Governors writing one session record at the
 * same instant: every appended incarnation survives, the generation binding
 * survives, indices are contiguous, and nothing regresses.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { canonicalSessionPath } from "../../governor/session/paths.ts";
import { SessionRegistry } from "../../governor/session/registry.ts";
import { REPO_ROOT } from "../lib/repo.ts";

type Outcome = { tag: string; role: string; outcome: string; version?: number; index?: number; detail?: string };

async function race(stateDir: string, sessionId: string, roles: string[]): Promise<Outcome[]> {
	const goFile = join(stateDir, `go-${sessionId}`);
	const child = join(REPO_ROOT, "conformance", "lib", "registry-race-child.ts");
	return new Promise<Outcome[]>((resolve, reject) => {
		const results: Outcome[] = [];
		let running = roles.length;
		const procs = roles.map((role, index) => spawn(process.execPath, [child, stateDir, goFile, sessionId, role, `${role}-${index}`], { stdio: ["ignore", "pipe", "inherit"] }));
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
		setTimeout(() => writeFileSync(goFile, "go"), 400);
	});
}

describe("D1: real processes racing on one session record", () => {
	it("one generation binding and six incarnation appends at once: all survive, indices contiguous, current is the last append", async () => {
		const stateDir = mkdtempSync(join(tmpdir(), "cg-regrace-"));
		const sessionDir = join(stateDir, "sessions-dir");
		mkdirSync(sessionDir);
		const registry = new SessionRegistry(stateDir);
		registry.create({ sessionId: "s", sessionPath: canonicalSessionPath(join(sessionDir, "s.jsonl"), sessionDir), lifecycle: "resident", activeSessionId: "A", openedBy: "parent" });

		const roles = ["bind", "append", "append", "append", "append", "append", "append"];
		const outcomes = await race(stateDir, "s", roles);
		assert.ok(outcomes.every((o) => o.outcome === "ok"), `every write landed: ${JSON.stringify(outcomes)}`);

		const final = registry.require("s");
		const appends = outcomes.filter((o) => o.role === "append");
		assert.equal(final.incarnations.length, 1 + appends.length, "no appended incarnation was lost");
		assert.deepEqual(final.incarnations.map((inc) => inc.index), final.incarnations.map((_, i) => i), "indices are contiguous");
		assert.equal(final.incarnations[0]!.generation, "gen-bind-0", "the generation binding survived every append");
		assert.deepEqual(new Set(final.incarnations.slice(1).map((inc) => inc.activeSessionId)), new Set(appends.map((a) => a.tag)));
		assert.equal(final.version, 1 + roles.length, "one version per write");
		const history = registry.history("s");
		for (let i = 1; i < history.length; i += 1) {
			assert.ok(history[i]!.incarnations.length >= history[i - 1]!.incarnations.length, "no version has fewer incarnations than its predecessor");
		}
	});
});
