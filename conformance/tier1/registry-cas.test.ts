/**
 * D1 (pure) — session records are compare-and-swap too: a generation bound
 * from a stale snapshot cannot drop an incarnation another Governor
 * appended in between, and two appends of different incarnations both
 * land. The interleavings are staged through the store's `beforeCommit`
 * seam; the negative control performs the pre-review rename-in-place from
 * a stale snapshot and shows the incarnation vanish, so the assertions are
 * known to be able to fail. The real multi-process race is
 * `registry-race.test.ts`.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { writeFileDurable } from "../../governor/fs/durable.ts";
import { canonicalSessionPath } from "../../governor/session/paths.ts";
import { SessionRegistry, type SessionRegistryOptions, StaleIncarnationError } from "../../governor/session/registry.ts";

function fresh(options?: SessionRegistryOptions) {
	const stateDir = mkdtempSync(join(tmpdir(), "cg-regcas-"));
	const sessionDir = join(stateDir, "sessions-dir");
	mkdirSync(sessionDir);
	const registry = new SessionRegistry(stateDir, options);
	registry.create({ sessionId: "s", sessionPath: canonicalSessionPath(join(sessionDir, "s.jsonl"), sessionDir), lifecycle: "resident", activeSessionId: "A", openedBy: "g1" });
	return { stateDir, registry };
}

describe("D1: registry writes are compare-and-swap", () => {
	it("the foreman's interleaving: G1 binds a generation from [A] while G2 appends B; both survive and B stays current", () => {
		const { stateDir, registry: g1 } = fresh({
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					g2.recordIncarnation({ sessionId: "s", activeSessionId: "B", cause: "reopen", openedBy: "g2" });
				},
			},
		});
		let competed = false;
		const g2 = new SessionRegistry(stateDir);
		const bound = g1.recordGeneration("s", "A", "gen-A");
		assert.equal(bound.generation, "gen-A");
		const record = g2.require("s");
		assert.deepEqual(record.incarnations.map((inc) => [inc.activeSessionId, inc.generation]), [["A", "gen-A"], ["B", undefined]], "G1's write was re-applied on top of G2's append");
		assert.equal(g2.current("s").activeSessionId, "B", "the stale-incarnation authority did not regress");
		assert.equal(record.version, 3);
		assert.throws(() => g2.assertCurrent("s", "A"), StaleIncarnationError);
	});

	it("the reverse: G2 appends B from [A] while G1 binds A's generation; both survive", () => {
		let competed = false;
		const { stateDir, registry: g1 } = fresh();
		const g2 = new SessionRegistry(stateDir, {
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					g1.recordGeneration("s", "A", "gen-A");
				},
			},
		});
		const { incarnation, appended } = g2.recordIncarnation({ sessionId: "s", activeSessionId: "B", cause: "reopen", openedBy: "g2" });
		assert.equal(appended, true);
		assert.equal(incarnation.index, 1);
		assert.deepEqual(g1.require("s").incarnations.map((inc) => [inc.activeSessionId, inc.generation]), [["A", "gen-A"], ["B", undefined]]);
	});

	it("two appends of different incarnations from the same snapshot: both land, indices are consistent", () => {
		let competed = false;
		const { stateDir, registry: g1 } = fresh();
		const g2 = new SessionRegistry(stateDir, {
			hooks: {
				beforeCommit: () => {
					if (competed) return;
					competed = true;
					g1.recordIncarnation({ sessionId: "s", activeSessionId: "B", cause: "reopen", openedBy: "g1" });
				},
			},
		});
		const { incarnation } = g2.recordIncarnation({ sessionId: "s", activeSessionId: "C", cause: "converged", openedBy: "g2" });
		assert.equal(incarnation.index, 2, "C's index was derived from the record that was current when it landed");
		assert.deepEqual(g1.require("s").incarnations.map((inc) => inc.activeSessionId), ["A", "B", "C"]);
		assert.deepEqual(g1.history("s").map((v) => v.version), [1, 2, 3]);
	});

	it("an idempotent write costs no version, and a same-id create converges instead of failing", () => {
		const { stateDir, registry } = fresh();
		const same = registry.recordIncarnation({ sessionId: "s", activeSessionId: "A", cause: "converged", openedBy: "g1" });
		assert.equal(same.appended, false);
		assert.equal(registry.require("s").version, 1, "nothing was written");
		registry.recordGeneration("s", "A", "gen-A");
		assert.equal(registry.recordGeneration("s", "A", "gen-A").generation, "gen-A");
		assert.equal(registry.require("s").version, 2, "binding the same generation again wrote nothing");
		// Two Governors both saw Prime converge a create on one path to one session.
		const other = new SessionRegistry(stateDir);
		const converged = other.create({ sessionId: "s", sessionPath: registry.require("s").sessionPath, lifecycle: "resident", activeSessionId: "A2", openedBy: "g2" });
		assert.deepEqual(converged.incarnations.map((inc) => [inc.activeSessionId, inc.cause]), [["A", "create"], ["A2", "converged"]]);
	});

	it("negative control: the pre-review rename-in-place from a stale snapshot drops the appended incarnation", () => {
		const { stateDir, registry: g1 } = fresh();
		const g2 = new SessionRegistry(stateDir);
		const stale = g1.require("s"); // G1's snapshot: [A]
		g2.recordIncarnation({ sessionId: "s", activeSessionId: "B", cause: "reopen", openedBy: "g2" });
		assert.equal(g2.current("s").activeSessionId, "B");
		// What the old recordGeneration did: modify the stale snapshot and rename it over the record.
		const regressed = { ...stale, incarnations: [{ ...stale.incarnations[0]!, generation: "gen-A" }] };
		writeFileDurable(g2.currentVersionPath("s"), `${JSON.stringify(regressed, null, 2)}\n`);
		assert.equal(g2.current("s").activeSessionId, "A", "the control demonstrates the lost incarnation the CAS prevents");
	});
});
