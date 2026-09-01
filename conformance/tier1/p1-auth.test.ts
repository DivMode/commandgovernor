/**
 * P1-AUTH — one authority per concern, checked rather than trusted.
 *
 * The reason this is a test and not a convention: Pi 0.84.4 resolves competing
 * extension handlers silently by load order. For a `session_before` event every
 * handler's result overwrites the previous one, so two extensions that both
 * answer `session_before_compact` do not conflict -- the last one loaded wins,
 * with no error and no warning. Pi also exposes no runtime API for enumerating
 * loaded extensions, so nothing can detect the collision from inside a session
 * either. A second owner is therefore an undetectable failure mode, and the
 * only place it can be caught is over the distribution's own pinned manifest.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { join } from "node:path";

import { checkAuthorities, checkPackageSet } from "../lib/policy.ts";
import {
	AUTHORITIES_JSON,
	exists,
	PROJECT_SETTINGS,
	readJson,
	readPins,
	REPO_ROOT,
} from "../lib/repo.ts";

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

interface ProjectSettings {
	packages?: (string | { source?: string })[];
}

const authorities = readJson(AUTHORITIES_JSON) as Authorities;
const pins = readPins();

/** Owner paths are repository-relative. */
function ownerExists(owner: string): boolean {
	return exists(join(REPO_ROOT, owner));
}

function packageSource(entry: string | { source?: string }): string {
	return typeof entry === "string" ? entry : (entry.source ?? "");
}

describe("P1-AUTH: the authorities manifest", () => {
	it("is a list, so that two owners for one concern is representable", () => {
		// If this were an object keyed by concern, a duplicate would be
		// impossible to write and the next assertion would be vacuous. The check
		// has to be able to fail.
		assert.ok(Array.isArray(authorities.concerns), "concerns must be an array");
		assert.ok(authorities.concerns.length > 0);
	});

	it("satisfies the whole authority policy", () => {
		const violations = checkAuthorities(authorities, ownerExists);
		assert.deepEqual(violations, [], violations.join("; "));
	});

	it("would reject a duplicate owner, a missing owner, and an unowned gap", () => {
		// The real document passes, which proves nothing about whether the rule
		// can fail. These fabricated documents are what establish that.
		const base = {
			concern: "compaction-summary",
			status: "assigned",
			owner: "conformance",
			note: "the real one",
		};

		const duplicate = { concerns: [base, { ...base, note: "a second owner" }] };
		assert.ok(
			checkAuthorities(duplicate, ownerExists).some((m) => m.includes("has 2 owners")),
			"two entries for one concern must be rejected",
		);

		const missingOwner = { concerns: [{ ...base, owner: undefined }] };
		assert.ok(
			checkAuthorities(missingOwner, ownerExists).some((m) =>
				m.includes("assigned with no owner"),
			),
		);

		const phantomOwner = { concerns: [{ ...base, owner: "harness/does-not-exist.ts" }] };
		assert.ok(
			checkAuthorities(phantomOwner, ownerExists).some((m) =>
				m.includes("does not exist in the repository"),
			),
			"an owner path that does not exist must be rejected",
		);

		const vagueGap = {
			concerns: [{ concern: "memory", status: "unassigned", note: "someday" }],
		};
		assert.ok(
			checkAuthorities(vagueGap, ownerExists).some((m) =>
				m.includes("must name a planned owner and a phase"),
			),
			"an unassigned concern with no plan must be rejected",
		);

		const noNote = { concerns: [{ ...base, note: undefined }] };
		assert.ok(checkAuthorities(noNote, ownerExists).some((m) => m.includes("needs a note")));

		const badStatus = { concerns: [{ ...base, status: "maybe" }] };
		assert.ok(
			checkAuthorities(badStatus, ownerExists).some((m) => m.includes("must be assigned")),
		);
	});

	it("would reject a concerns map that cannot express a duplicate", () => {
		// If `concerns` were an object keyed by concern, a second owner would be
		// unrepresentable and the duplicate rule above would be decoration.
		const asObject = { concerns: { "compaction-summary": { status: "assigned" } } };
		assert.ok(
			checkAuthorities(asObject, ownerExists).some((m) => m.includes("must be an array")),
		);
	});

	it("names the concerns this revision actually implements", () => {
		const assigned = new Set(
			authorities.concerns.filter((c) => c.status === "assigned").map((c) => c.concern),
		);
		for (const required of [
			"pi-runtime",
			"runtime-version-gate",
			"launch-preflight-and-trust",
			"conformance-suite",
		]) {
			assert.ok(assigned.has(required), `${required} should be an assigned concern`);
		}
	});

	it("records compaction ownership as deliberately unassigned", () => {
		// The highest-risk gap: it is the one where a second owner is silently
		// last-loaded-wins. Leaving it unnamed is how it gets adopted by
		// whichever memory package is installed next.
		const compaction = authorities.concerns.find((c) => c.concern === "compaction-summary");
		assert.ok(compaction, "compaction-summary must appear, even unowned");
		assert.equal(compaction.status, "unassigned");
	});
});

describe("P1-AUTH: settings and pins agree", () => {
	const settings = readJson(PROJECT_SETTINGS) as ProjectSettings;

	it("pins every package the project settings install", () => {
		// Pi installs missing packages automatically on trusted startup and
		// keeps no lockfile for them. If someone runs `pi install` by hand,
		// settings.json and pins.json diverge silently; this is the diff.
		const pinnedSources = new Set(pins.packages.map((pkg) => pkg.source));
		for (const entry of settings.packages ?? []) {
			const source = packageSource(entry);
			if (source === "" || source.startsWith(".") || source.startsWith("/")) {
				// A local path source is this repository dogfooding itself. It is
				// pinned by the checkout, not by pins.json.
				continue;
			}
			assert.ok(
				pinnedSources.has(source),
				`${source} is installed by .pi/settings.json but is not recorded in pins/pins.json`,
			);
		}
	});

	it("makes every pinned package name a known authority, uniquely", () => {
		const concerns = new Set(authorities.concerns.map((c) => c.concern));
		const violations = checkPackageSet(pins.packages, concerns);
		assert.deepEqual(violations, [], violations.join("; "));
	});

	it("would reject a package claiming an authority nobody declared", () => {
		const concerns = new Set(authorities.concerns.map((c) => c.concern));
		const rogue = [
			{
				source: "npm:x@1.0.0",
				exactVersion: "1.0.0",
				license: "MIT",
				reviewedAt: "2026-09-01",
				authority: "a-concern-nobody-declared",
			},
		];
		assert.ok(
			checkPackageSet(rogue, concerns).some((m) => m.includes("which is not a concern")),
		);
	});

	it("keeps the dogfooding settings honest about trust", () => {
		// The repo's own settings are only honoured because bin/cg-pi passes
		// --approve. A settings file that quietly relied on a machine-wide
		// `defaultProjectTrust: "always"` would work here and nowhere else.
		const raw = readJson(PROJECT_SETTINGS) as Record<string, unknown>;
		assert.equal(
			raw.defaultProjectTrust,
			undefined,
			"project settings must not set defaultProjectTrust; trust is the launcher's decision, and a project cannot grant itself trust it needs in order to be read",
		);
	});
});
