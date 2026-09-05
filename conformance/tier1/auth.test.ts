/**
 * AUTH — one authority per concern, and an explicit architecture disposition
 * for every assigned concern.
 *
 * The record lives in `pins/pins.json` `concerns[]`, next to the packages and
 * the substrate it assigns ownership to, so "who owns this?" and "what exact
 * version is that?" cannot drift apart.
 *
 * This test deliberately does NOT name individual concerns or freeze who owns
 * them. Ownership is exactly what the composition-first architecture keeps
 * moving: a concern that Prime, a package or upstream takes over is success,
 * not a regression, and a test that hardcodes today's owner turns that success
 * into a red build. What must not move is the shape of the record — every
 * assigned concern has one owner that really exists, one disposition, and a
 * stated removal condition if it is temporary — so that is what is asserted,
 * over whatever the manifest currently says.
 *
 * An owner is "real" in exactly three forms, and each is checked against
 * something outside the concern record itself:
 *   - `prime-agent`, the pinned substrate (`substrate.name`);
 *   - a `packages[]` source string, so an owner cannot name a package the
 *     distribution does not pin;
 *   - a path in this repository.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { join } from "node:path";

import { checkAuthorities, checkPackageSet } from "../lib/policy.ts";
import { exists, readPins, REPO_ROOT } from "../lib/repo.ts";

const pins = readPins();
const concerns = pins.concerns;
const packageSources = new Set(pins.packages.map((entry) => String(entry.source)));

/** Is this owner string something that actually exists outside the record? */
function ownerExists(owner: string): boolean {
	if (owner === pins.substrate.name) return true;
	if (packageSources.has(owner)) return true;
	return exists(join(REPO_ROOT, owner));
}

describe("AUTH: pins.json concerns[]", () => {
	it("the checker fails on fabricated ownership and disposition violations", () => {
		assert.ok(checkAuthorities({ concerns: { a: 1 } }, ownerExists).length > 0, "a non-array concerns must be rejected");
		assert.ok(checkAuthorities({ concerns: [] }, ownerExists).length > 0, "an empty concerns list must be rejected");

		const duplicate = {
			concerns: [
				{ concern: "x", status: "assigned", disposition: "PLUGIN", owner: "conformance", note: "n" },
				{ concern: "x", status: "assigned", disposition: "PLUGIN", owner: "conformance", note: "n" },
			],
		};
		assert.ok(checkAuthorities(duplicate, ownerExists).some((error) => /2 owners/.test(error)));

		const ghost = { concerns: [{ concern: "x", status: "assigned", disposition: "PLUGIN", owner: "does/not/exist", note: "n" }] };
		assert.ok(checkAuthorities(ghost, ownerExists).some((error) => /does not exist/.test(error)));

		const unpinnedPackage = { concerns: [{ concern: "x", status: "assigned", disposition: "USE EXISTING", owner: "npm:not-pinned@1.0.0", note: "n" }] };
		assert.ok(
			checkAuthorities(unpinnedPackage, ownerExists).some((error) => /does not exist/.test(error)),
			"an owner may not name a package the distribution does not pin",
		);

		const noDisposition = { concerns: [{ concern: "x", status: "assigned", owner: "conformance", note: "n" }] };
		assert.ok(checkAuthorities(noDisposition, ownerExists).some((error) => /needs disposition/.test(error)));

		const immortalWorkaround = { concerns: [{ concern: "x", status: "assigned", disposition: "TEMP WORKAROUND", owner: "conformance", note: "n" }] };
		assert.ok(checkAuthorities(immortalWorkaround, ownerExists).some((error) => /removalCondition/.test(error)));

		const adoptable = { concerns: [{ concern: "x", status: "unassigned", note: "n" }] };
		assert.ok(checkAuthorities(adoptable, ownerExists).some((error) => /planned owner/.test(error)));

		assert.deepEqual(
			checkAuthorities({ concerns: [{ concern: "x", status: "assigned", disposition: "USE EXISTING", owner: "prime-agent", note: "n" }] }, ownerExists),
			[],
			"a well-formed record must pass, or the checker only ever says no",
		);
	});

	it("the real record passes the ownership/disposition policy", () => {
		assert.deepEqual(checkAuthorities({ concerns }, ownerExists), []);
		assert.ok(concerns.length > 0, "pins.json declares no concerns");
	});

	it("every assigned owner is the pinned substrate, a pinned package, or a path in this repository", () => {
		for (const entry of concerns) {
			if (entry.status !== "assigned") continue;
			const owner = String(entry.owner);
			assert.ok(ownerExists(owner), `${String(entry.concern)}: owner ${owner} is neither the substrate, a pinned package, nor a repository path`);
			if (entry.disposition === "TEMP WORKAROUND") {
				assert.ok(
					typeof entry.removalCondition === "string" && entry.removalCondition.length > 20,
					`${String(entry.concern)}: a TEMP WORKAROUND needs a real deletion gate, not a placeholder`,
				);
			}
		}
	});

	it("an unassigned concern cannot be adopted by accident", () => {
		for (const entry of concerns) {
			if (entry.status !== "unassigned") continue;
			assert.ok(typeof entry.plannedOwner === "string" && entry.plannedOwner.length > 0, `${String(entry.concern)}: needs a planned owner`);
			assert.ok(typeof entry.phase === "string" && entry.phase.length > 0, `${String(entry.concern)}: needs the phase that decides it`);
			assert.equal(entry.owner, undefined, `${String(entry.concern)}: unassigned but already names an owner`);
		}
	});

	it("every pinned package claims a concern this record declares, and no two claim the same one", () => {
		const known = new Set(concerns.map((entry) => String(entry.concern)));
		assert.deepEqual(checkPackageSet(pins.packages, known), []);
	});
});
