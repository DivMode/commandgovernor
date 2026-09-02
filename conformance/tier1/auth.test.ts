/**
 * AUTH — one authority per concern, and an explicit architecture disposition
 * for every assigned concern.
 *
 * This test deliberately does NOT freeze temporary workaround ownership to a
 * specific governor/* path. A package/upstream implementation replacing a
 * workaround is success, not a regression.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { join } from "node:path";

import { checkAuthorities, checkPackageSet } from "../lib/policy.ts";
import { AUTHORITIES_JSON, exists, readJson, readPins, REPO_ROOT } from "../lib/repo.ts";

interface Concern {
	concern: string;
	status: "assigned" | "unassigned";
	disposition?: "USE EXISTING" | "PLUGIN" | "TEMP WORKAROUND";
	owner?: string;
	removalCondition?: string;
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
	it("the checker fails on fabricated ownership and disposition violations", () => {
		assert.ok(checkAuthorities({ concerns: { a: 1 } }, ownerExists).length > 0);
		assert.ok(checkAuthorities({ concerns: [] }, ownerExists).length > 0);

		const duplicate = { concerns: [
			{ concern: "x", status: "assigned", disposition: "PLUGIN", owner: "governor", note: "n" },
			{ concern: "x", status: "assigned", disposition: "PLUGIN", owner: "governor", note: "n" },
		] };
		assert.ok(checkAuthorities(duplicate, ownerExists).some((e) => /2 owners/.test(e)));

		const ghost = { concerns: [{ concern: "x", status: "assigned", disposition: "PLUGIN", owner: "does/not/exist", note: "n" }] };
		assert.ok(checkAuthorities(ghost, ownerExists).some((e) => /does not exist/.test(e)));

		const noDisposition = { concerns: [{ concern: "x", status: "assigned", owner: "governor", note: "n" }] };
		assert.ok(checkAuthorities(noDisposition, ownerExists).some((e) => /needs disposition/.test(e)));

		const immortalWorkaround = { concerns: [{ concern: "x", status: "assigned", disposition: "TEMP WORKAROUND", owner: "governor", note: "n" }] };
		assert.ok(checkAuthorities(immortalWorkaround, ownerExists).some((e) => /removalCondition/.test(e)));

		const adoptable = { concerns: [{ concern: "x", status: "unassigned", note: "n" }] };
		assert.ok(checkAuthorities(adoptable, ownerExists).some((e) => /planned owner/.test(e)));
	});

	it("the real file passes the ownership/disposition policy", () => {
		assert.deepEqual(checkAuthorities(authorities, ownerExists), []);
		assert.equal(authorities.schemaVersion, 3);
	});

	it("Prime owns generic runtime/session concerns; custom D1/D2 owners are explicitly temporary", () => {
		const byName = new Map(authorities.concerns.map((c) => [c.concern, c]));

		for (const concern of ["runtime-substrate", "session-persistence"]) {
			const entry = byName.get(concern);
			assert.equal(entry?.status, "assigned", concern);
			assert.equal(entry?.disposition, "USE EXISTING", concern);
			assert.equal(entry?.owner, "pins/prime-0.8.1", concern);
		}

		for (const concern of [
			"session-path-policy",
			"session-identity-and-incarnations",
			"resident-root-recovery",
			"mutation-outcome-classification",
			"mutation-ledger",
		]) {
			const entry = byName.get(concern);
			assert.equal(entry?.status, "assigned", concern);
			assert.equal(entry?.disposition, "TEMP WORKAROUND", concern);
			assert.ok(entry?.removalCondition && entry.removalCondition.length > 20, `${concern} needs a real deletion gate`);
		}

		for (const concern of ["substrate-version-gate", "environment-boundary", "agent-role-definitions", "conformance-suite"]) {
			assert.equal(byName.get(concern)?.disposition, "PLUGIN", concern);
		}
	});

	it("future generic capabilities remain unassigned until their bake-off proves an owner", () => {
		const byName = new Map(authorities.concerns.map((c) => [c.concern, c]));
		for (const concern of [
			"role-loadout-enforcement",
			"foreman-transport",
			"foreman-event-ledger",
			"compaction-summary",
			"observational-memory",
			"tool-gating-and-veto",
			"acp-boundary",
			"sandbox-profile",
		]) {
			assert.equal(byName.get(concern)?.status, "unassigned", concern);
		}
	});

	it("third-party package policy fails closed on mutable/unowned package entries", () => {
		const known = new Set(authorities.concerns.map((c) => c.concern));
		assert.deepEqual(checkPackageSet(readPins().packages as never, known), []);
		const errors = checkPackageSet([{ source: "some-pkg", exactVersion: "main", authority: "nope" }] as never, known);
		assert.ok(errors.some((e) => /not a pin/.test(e)));
		assert.ok(errors.some((e) => /not a concern/.test(e)));
		assert.ok(errors.some((e) => /license/.test(e)));

		const twice = checkPackageSet([
			{ source: "a", exactVersion: "1.0.0", authority: "conformance-suite", license: "MIT", reviewedAt: "2026-09-01" },
			{ source: "b", exactVersion: "1.0.0", authority: "conformance-suite", license: "MIT", reviewedAt: "2026-09-01" },
		] as never, known);
		assert.ok(twice.some((e) => /claimed by a and b/.test(e)));
	});
});
