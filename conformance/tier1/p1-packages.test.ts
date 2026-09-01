/**
 * P1-PACKAGES — the repository exposes exactly what Pi exposes.
 *
 * Pi makes five packages available to an extension: `@earendil-works/
 * pi-coding-agent`, `-pi-agent-core`, `-pi-ai`, `-pi-tui`, and `typebox`.
 * Everything else must come from the extension's own dependencies.
 *
 * The local development environment has to match that list in *both*
 * directions, and getting it wrong is silent either way. Expose too much and an
 * extension importing `@earendil-works/pi-protocol` typechecks here, passes the
 * suite, and throws on load inside Pi. Expose too little -- the original bug
 * omitted `typebox`, which is not under the `@earendil-works` scope and so was
 * missed by a scope-wide symlink -- and a legitimate import fails locally for a
 * reason that has nothing to do with the code.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

/** Exactly the packages Pi provides. */
const PROVIDED = [
	"@earendil-works/pi-coding-agent",
	"@earendil-works/pi-agent-core",
	"@earendil-works/pi-ai",
	"@earendil-works/pi-tui",
	"typebox",
] as const;

/**
 * Published in the same scope, at the same version, and NOT available to an
 * extension. These are the ones a scope-wide symlink leaks.
 */
const NOT_PROVIDED = [
	"@earendil-works/pi-client",
	"@earendil-works/pi-protocol",
	"@earendil-works/pi-telemetry",
] as const;

describe("P1-PACKAGES: the five Pi-provided packages resolve", () => {
	for (const name of PROVIDED) {
		it(`imports ${name}`, async () => {
			const module: unknown = await import(name);
			assert.equal(typeof module, "object");
			assert.notEqual(module, null);
		});
	}
});

describe("P1-PACKAGES: nothing else from the pinned tree resolves", () => {
	for (const name of NOT_PROVIDED) {
		it(`does not import ${name}`, async () => {
			// Not a style preference. An extension cannot import these at
			// runtime, so a development environment where they resolve is more
			// permissive than the runtime and hides the failure until load time.
			await assert.rejects(
				async () => {
					await import(name);
				},
				(error: unknown) => {
					const code = (error as { code?: string }).code;
					assert.equal(
						code,
						"ERR_MODULE_NOT_FOUND",
						`${name} resolved, or failed for an unexpected reason: ${String(error)}`,
					);
					return true;
				},
			);
		});
	}
});

describe("P1-PACKAGES: the pinned runtime is the one that resolves", () => {
	it("resolves pi-coding-agent to the pinned install, not a stray copy", async () => {
		const { VERSION } = await import("@earendil-works/pi-coding-agent");
		const { readPins } = await import("../lib/repo.ts");
		assert.equal(
			VERSION,
			readPins().pi.version,
			"the resolved pi is not the pinned one; check scripts/bootstrap.sh's links",
		);
	});
});
