/**
 * The teardown assertion every runtime fixture is held to.
 *
 * Three separate leaks, because each is invisible to the checks for the other
 * two: a process still in this root's tree, a supervisor still answering on
 * this root's socket (which the process sweep cannot see, because Prime
 * retitles its processes and a replacement supervisor is nobody's child), and
 * the root directory itself surviving the run.
 */

import assert from "node:assert/strict";

import type { StopReport } from "./prime.ts";

export function assertCleanTeardown(report: StopReport): void {
	assert.deepEqual(report.survivors, [], report.survivors.map((row) => `${row.pid} ${row.command.slice(0, 120)}`).join(" | "));
	assert.equal(report.daemonStillAnswering, false, "a supervisor was still answering on the fixture socket after teardown");
	assert.equal(
		report.rootLeaked,
		false,
		`the fixture root kept coming back after teardown removed it; what reappeared: ${JSON.stringify(report.recreatedAfterRemoval)}`,
	);
}
