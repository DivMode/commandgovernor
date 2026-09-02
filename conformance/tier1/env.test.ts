/**
 * Environment boundary (pure) — the allowlist builder over fabricated
 * environments. The runtime tier proves the far side; this proves the rule.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { buildLaunchEnv, DEFAULT_LAUNCH_ENV_ALLOWLIST, launchEnvIsWithinAllowlist } from "../../governor/prime/env.ts";

describe("buildLaunchEnv", () => {
	const source = {
		PATH: "/usr/bin",
		HOME: "/Users/x",
		CG_SENTINEL_ORDINARY_NAME: "s3cret-with-an-innocent-name",
		GITHUB_TOKEN: "ghp_x",
		OP_SERVICE_ACCOUNT_TOKEN: "ops_x",
		AWS_SECRET_ACCESS_KEY: "aws",
		DATABASE_URL: "postgres://user:pass@host/db",
		NODE_OPTIONS: "--require evil.js",
		PRIME_AGENT_INTERNAL_DAEMON_WORKER: "1",
		UNSET: undefined,
	};

	it("forwards only the allowlist; a secret with an ordinary name is withheld like any other", () => {
		const built = buildLaunchEnv(source);
		assert.deepEqual(Object.keys(built.env).sort(), ["HOME", "PATH"]);
		assert.ok(built.withheld.includes("CG_SENTINEL_ORDINARY_NAME"));
		assert.ok(built.withheld.includes("DATABASE_URL"));
		assert.ok(built.withheld.includes("NODE_OPTIONS"));
		assert.ok(built.withheld.includes("PRIME_AGENT_INTERNAL_DAEMON_WORKER"));
		assert.ok(!built.withheld.includes("UNSET"), "an undefined value is not a variable");
		assert.ok(launchEnvIsWithinAllowlist(built.env));
	});

	it("an explicit grant is the only way through, and it is per name", () => {
		const built = buildLaunchEnv(source, { grant: ["DATABASE_URL"] });
		assert.equal(built.env.DATABASE_URL, source.DATABASE_URL);
		assert.ok(!("CG_SENTINEL_ORDINARY_NAME" in built.env));
		assert.ok(launchEnvIsWithinAllowlist(built.env, DEFAULT_LAUNCH_ENV_ALLOWLIST, ["DATABASE_URL"]));
		assert.ok(!launchEnvIsWithinAllowlist(built.env));
	});

	it("overrides win and Prime-internal role variables can never be granted or overridden", () => {
		assert.equal(buildLaunchEnv(source, { overrides: { HOME: "/tmp/h" } }).env.HOME, "/tmp/h");
		assert.throws(() => buildLaunchEnv(source, { grant: ["PRIME_AGENT_INTERNAL_DAEMON_WORKER"] }), /may never be forwarded/);
		assert.throws(() => buildLaunchEnv(source, { overrides: { PRIME_AGENT_INTERNAL_DAEMON_WORKER: "1" } }), /may never be forwarded/);
	});

	it("negative control: a name-based denylist would have forwarded the ordinary-named secret", () => {
		const denylisted = Object.fromEntries(Object.entries(source).filter(([k, v]) => v !== undefined && !/TOKEN|SECRET|PASSWORD|KEY/i.test(k)));
		assert.ok("CG_SENTINEL_ORDINARY_NAME" in denylisted, "the denylist lets it through");
		assert.ok("DATABASE_URL" in denylisted, "and the connection string with the password in it");
		assert.ok(!launchEnvIsWithinAllowlist(denylisted), "which the allowlist check rejects");
	});

	it("the default allowlist contains no credential-bearing or code-injecting names", () => {
		for (const name of DEFAULT_LAUNCH_ENV_ALLOWLIST) {
			assert.ok(!/TOKEN|SECRET|PASSWORD|API_KEY|CREDENTIAL|NODE_OPTIONS|NODE_EXTRA_CA_CERTS|SSH_AUTH_SOCK/.test(name), name);
		}
	});
});
