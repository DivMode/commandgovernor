/**
 * PID — process identity beyond the pid: the verdicts a fence may act on.
 *
 * Reclaim only on `gone` or `replaced`; never on `unknown` or `current`.
 * The real probe is exercised against this process and against a pid that
 * cannot exist; the fabricated probe exercises every verdict, including the
 * pid-reuse case a real test cannot stage on demand.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { classifyProcessIdentity, currentProcessIdentity, identityProvesProcessOver, LIVE_PROBE, type ProcessProbe, processStartId } from "../../governor/process/identity.ts";

describe("PID: process start identity", () => {
	it("reads this process's own start identity, stably", () => {
		const own = currentProcessIdentity();
		assert.equal(own.pid, process.pid);
		assert.ok(own.processStartId, "the platform reports a start identity for a live process");
		assert.match(own.processStartId!, process.platform === "linux" ? /^proc:\d+$/ : /^ps:.+/);
		assert.equal(processStartId(process.pid), own.processStartId, "stable across reads");
		assert.deepEqual(currentProcessIdentity(), own, "cached");
	});

	it("reports no identity for a pid that cannot exist, and never throws", () => {
		assert.equal(processStartId(2147483646), undefined);
		assert.equal(processStartId(0), undefined);
		assert.equal(processStartId(-1), undefined);
		assert.equal(processStartId(1.5), undefined);
	});

	it("the live probe: this process is current; a dead pid is gone; a missing start id is unknown", () => {
		const own = currentProcessIdentity();
		assert.equal(classifyProcessIdentity(own, LIVE_PROBE), "current");
		assert.equal(classifyProcessIdentity({ pid: 2147483646, processStartId: "ps:whatever" }, LIVE_PROBE), "gone");
		assert.equal(classifyProcessIdentity({ pid: process.pid }, LIVE_PROBE), "unknown", "a record without a start id cannot prove anything about a live pid");
		assert.equal(classifyProcessIdentity({ pid: process.pid, processStartId: "ps:Thu Jan  1 00:00:00 1970" }, LIVE_PROBE), "replaced", "a live pid with a different start identity is a recycled pid");
	});

	it("the fabricated probe covers pid reuse and the conservative branches", () => {
		const probe = (alive: boolean, observed: string | undefined): ProcessProbe => ({ alive: () => alive, startId: () => observed });
		assert.equal(classifyProcessIdentity({ pid: 7, processStartId: "a" }, probe(true, "a")), "current");
		assert.equal(classifyProcessIdentity({ pid: 7, processStartId: "a" }, probe(true, "b")), "replaced");
		assert.equal(classifyProcessIdentity({ pid: 7, processStartId: "a" }, probe(false, "b")), "gone");
		assert.equal(classifyProcessIdentity({ pid: 7, processStartId: "a" }, probe(false, undefined)), "gone");
		assert.equal(classifyProcessIdentity({ pid: 7, processStartId: "a" }, probe(true, undefined)), "unknown", "cannot read the live process: unknown");
		assert.equal(classifyProcessIdentity({ pid: 7 }, probe(true, "a")), "unknown", "nothing recorded: unknown, even though the pid is readable");
		assert.equal(classifyProcessIdentity({ pid: 7 }, probe(false, "a")), "gone", "a dead pid is gone whatever was recorded");
	});

	it("only gone and replaced license a fence to take over", () => {
		assert.equal(identityProvesProcessOver("gone"), true);
		assert.equal(identityProvesProcessOver("replaced"), true);
		assert.equal(identityProvesProcessOver("current"), false);
		assert.equal(identityProvesProcessOver("unknown"), false);
	});
});
