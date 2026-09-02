/**
 * AUTH — one authority per concern, checked rather than trusted.
 *
 * Transplanted from PR #16. The reason it is a test and not a convention has
 * not changed with the substrate: Prime, like upstream Pi, resolves competing
 * extension handlers silently by load order, and the Governor now also owns
 * durable state (registry, ledger) whose single-writer property is a claim
 * that must be visible in one place. The check runs over fabricated violating
 * records first, so it is shown to be able to fail.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { join } from "node:path";

import { checkAuthorities, checkPackageSet } from "../lib/policy.ts";
import { AUTHORITIES_JSON, exists, readJson, readPins, REPO_ROOT } from "../lib/repo.ts";

interface Concern {
	concern: string;
	status: "assigned" | "unassigned";
	owner?: string;
	plannedOwner?: string;
	phase?: string;
	note?: string;
}

interface Authorities {
	schemaVersion: number;
	concerns: Concern[];
}

const authorities = readJson(AUTHORITIES_JSON) as Authorities;
const ownerExists = (owner: string) => exists(join(REPO_ROOT, owner));

describe("AUTH: harness/authorities.json", () => {
	it("the checker fails on fabricated violations before it is trusted on the real file", () => {
		assert.ok(checkAuthorities({ concerns: { a: 1 } }, ownerExists).length > 0, "an object keyed by concern is rejected");
		assert.ok(checkAuthorities({ concerns: [] }, ownerExists).length > 0, "empty is rejected");
		const duplicate = { concerns: [{ concern: "x", status: "assigned", owner: "governor", note: "n" }, { concern: "x", status: "assigned", owner: "governor", note: "n" }] };
		assert.ok(checkAuthorities(duplicate, ownerExists).some((e) => /2 owners/.test(e)), "two owners for one concern is reported");
		const ghost = { concerns: [{ concern: "x", status: "assigned", owner: "does/not/exist", note: "n" }] };
		assert.ok(checkAuthorities(ghost, ownerExists).some((e) => /does not exist/.test(e)));
		const adoptable = { concerns: [{ concern: "x", status: "unassigned", note: "n" }] };
		assert.ok(checkAuthorities(adoptable, ownerExists).some((e) => /planned owner/.test(e)));
	});

	it("the real file passes", () => {
		assert.deepEqual(checkAuthorities(authorities, ownerExists), []);
		assert.equal(authorities.schemaVersion, 2);
	});

	it("names the concerns Issue #17 introduces, each with exactly one owner in governor/", () => {
		const byName = new Map(authorities.concerns.map((c) => [c.concern, c]));
		for (const [concern, owner] of [
			["runtime-substrate", "pins/prime-0.8.1"],
			["substrate-version-gate", "governor/prime/daemon-client.ts"],
			["environment-boundary", "governor/prime/env.ts"],
			["session-path-policy", "governor/session/paths.ts"],
			["session-identity-and-incarnations", "governor/session/registry.ts"],
			["resident-root-recovery", "governor/governor.ts"],
			["mutation-outcome-classification", "governor/mutation/classify.ts"],
			["mutation-ledger", "governor/mutation/ledger.ts"],
			["conformance-suite", "conformance"],
		] as const) {
			const entry = byName.get(concern);
			assert.ok(entry, `missing concern ${concern}`);
			assert.equal(entry.status, "assigned", concern);
			assert.equal(entry.owner, owner, concern);
		}
		for (const concern of ["foreman-transport", "foreman-event-ledger", "compaction-summary", "observational-memory", "acp-boundary", "sandbox-profile"]) {
			assert.equal(byName.get(concern)?.status, "unassigned", `${concern} must remain explicitly unassigned in this issue`);
		}
	});

	it("third-party package policy over pins.json packages[] (empty today) fails on fabricated entries", () => {
		const known = new Set(authorities.concerns.map((c) => c.concern));
		assert.deepEqual(checkPackageSet(readPins().packages as never, known), []);
		const errors = checkPackageSet([{ source: "some-pkg", exactVersion: "main", authority: "nope" }] as never, known);
		assert.ok(errors.some((e) => /not a pin/.test(e)));
		assert.ok(errors.some((e) => /not a concern/.test(e)));
		assert.ok(errors.some((e) => /license/.test(e)));
		const twice = checkPackageSet([{ source: "a", exactVersion: "1.0.0", authority: "conformance-suite", license: "MIT", reviewedAt: "2026-09-01" }, { source: "b", exactVersion: "1.0.0", authority: "conformance-suite", license: "MIT", reviewedAt: "2026-09-01" }] as never, known);
		assert.ok(twice.some((e) => /claimed by a and b/.test(e)));
	});
});
