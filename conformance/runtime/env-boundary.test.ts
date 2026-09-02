/**
 * Credential / environment boundary (Issue #17; Issue #15 S0 finding 4).
 *
 * Prime's own client forwards the whole process environment to the daemon.
 * The Governor forwards a positive allowlist. This test plants sentinels in
 * the Governor's OWN environment and proves, from the far side of the socket,
 * that they did not cross:
 *
 * - `CG_SENTINEL_ORDINARY_NAME`: a secret whose name contains none of TOKEN,
 *   SECRET, PASSWORD, KEY, CREDENTIAL. A name-based denylist would forward it.
 * - `CG_SENTINEL_API_TOKEN`: the secret-shaped case, for completeness.
 * - `CG_GRANTED_MARKER`: explicitly granted, so it MUST cross. This is the
 *   control that proves the far-side probe can actually see variables.
 *
 * The far side is a worker's `env`, read through `execute_bash_and_wait`. The
 * two edges (supervisor spawn env, wire launchEnv) are both exercised because
 * the fixture spawns the supervisor with the same allowlist the Governor puts
 * on the wire.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { DEFAULT_LAUNCH_ENV_ALLOWLIST } from "../../governor/prime/env.ts";
import { type PrimeFixture, startPrimeFixture } from "../lib/prime-fixture.ts";

const ORDINARY = "CG_SENTINEL_ORDINARY_NAME";
const SHAPED = "CG_SENTINEL_API_TOKEN";
const GRANTED = "CG_GRANTED_MARKER";
const ORDINARY_VALUE = `ordinary-${process.pid}-${Date.now().toString(36)}`;
const SHAPED_VALUE = `shaped-${process.pid}-${Date.now().toString(36)}`;
const GRANTED_VALUE = `granted-${process.pid}-${Date.now().toString(36)}`;

let fixture: PrimeFixture;

before(async () => {
	const sourceEnv = { ...process.env, [ORDINARY]: ORDINARY_VALUE, [SHAPED]: SHAPED_VALUE, [GRANTED]: GRANTED_VALUE };
	fixture = await startPrimeFixture({ sourceEnv, grant: [GRANTED] });
});

after(async () => {
	await fixture.stop();
});

describe("environment boundary: Governor -> Prime is a positive allowlist", () => {
	it("withholds a sentinel with an ordinary name, withholds a secret-shaped one, and forwards an explicit grant", async () => {
		assert.ok(fixture.supervisor.withheldEnv.includes(ORDINARY), "the supervisor spawn withheld the ordinary-named sentinel");
		assert.ok(fixture.supervisor.withheldEnv.includes(SHAPED));
		assert.ok(!(ORDINARY in fixture.supervisor.env));
		assert.equal(fixture.supervisor.env[GRANTED], GRANTED_VALUE);

		const governor = await fixture.governor("env");
		const created = await governor.createSession({ sessionPath: join(fixture.sessionDir, "env-root.jsonl") });
		assert.ok(created.withheldEnv.includes(ORDINARY), "the wire launchEnv withheld the ordinary-named sentinel");
		assert.ok(created.withheldEnv.includes(SHAPED));
		const { sessionId } = created.record;
		const active = created.record.incarnations[0]!.activeSessionId;
		await governor.attach(sessionId);

		// Far side: what the worker actually has.
		const probe = await governor.dispatchMutation(sessionId, active, { type: "execute_bash_and_wait", command: "env" });
		assert.equal(probe.verdict.verdict, "completed");
		const output = String((probe.verdict.verdict === "completed" ? (probe.verdict.response.data as { output?: string }).output : "") ?? "");
		assert.ok(output.includes(`${GRANTED}=${GRANTED_VALUE}`), "control: the granted variable IS visible in the worker, so the probe can see variables");
		assert.ok(!output.includes(ORDINARY_VALUE), "the ordinary-named sentinel value never reached the worker");
		assert.ok(!output.includes(`${ORDINARY}=`), "nor its name");
		assert.ok(!output.includes(SHAPED_VALUE));
		assert.ok(!output.includes(`${SHAPED}=`));

		// Every variable the worker has is either allowlisted, granted, or set by Prime itself.
		const workerKeys = output.split("\n").filter((line) => /^[A-Za-z_][A-Za-z0-9_]*=/.test(line)).map((line) => line.slice(0, line.indexOf("=")));
		const permitted = new Set([...DEFAULT_LAUNCH_ENV_ALLOWLIST, GRANTED]);
		const unexpected = workerKeys.filter((key) => !permitted.has(key) && !key.startsWith("PRIME_AGENT_") && !key.startsWith("PI_") && !["PWD", "OLDPWD", "SHLVL", "_", "__CF_USER_TEXT_ENCODING"].includes(key));
		assert.deepEqual(unexpected, [], `worker environment carries variables outside the allowlist: ${unexpected.join(", ")}`);

		// The wire evidence log never carries launchEnv values, only key names. (Inbound frames are
		// recorded verbatim, and this test's own `env` output legitimately contains the granted value,
		// so the check is over what the Governor SENT.)
		const outbound = readFileSync(join(fixture.root, "wire.jsonl"), "utf8").split("\n").filter((line) => line.includes('"direction":"out"'));
		assert.ok(outbound.some((line) => line.includes('"launchEnv":{"$redacted":true')), "outbound create/attach frames log launchEnv as key names only");
		assert.ok(!outbound.some((line) => line.includes(GRANTED_VALUE)), "even a granted value is not written to the wire log");
		assert.ok(!outbound.some((line) => line.includes(ORDINARY_VALUE)));
		assert.ok(!outbound.some((line) => line.includes(SHAPED_VALUE)));
		governor.close();
	});
});
