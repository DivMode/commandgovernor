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

import {
	AUTHORITIES_JSON,
	exists,
	PROJECT_SETTINGS,
	readJson,
	readPins,
	REPO_ROOT,
} from "../lib/repo.ts";
import { join } from "node:path";

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

	it("gives no concern two owners", () => {
		const seen = new Map<string, number>();
		for (const entry of authorities.concerns) {
			seen.set(entry.concern, (seen.get(entry.concern) ?? 0) + 1);
		}
		const duplicates = [...seen].filter(([, count]) => count > 1).map(([name]) => name);
		assert.deepEqual(duplicates, [], `concerns with more than one owner: ${duplicates}`);
	});

	it("gives every concern a resolved status and a real owner or a named gap", () => {
		for (const entry of authorities.concerns) {
			assert.ok(entry.concern.length > 0);
			assert.ok(
				entry.status === "assigned" || entry.status === "unassigned",
				`${entry.concern}: status must be assigned or unassigned`,
			);
			if (entry.status === "assigned") {
				assert.ok(entry.owner, `${entry.concern}: assigned with no owner`);
				assert.ok(
					exists(join(REPO_ROOT, entry.owner)),
					`${entry.concern}: owner ${entry.owner} does not exist in the repository`,
				);
			} else {
				assert.ok(
					entry.plannedOwner && entry.phase,
					`${entry.concern}: an unassigned concern must name a planned owner and a phase, so it cannot be adopted by accident`,
				);
			}
			assert.ok(entry.note && entry.note.length > 0, `${entry.concern}: needs a note`);
		}
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

	it("makes every pinned package name the authority it owns", () => {
		for (const pkg of pins.packages) {
			assert.ok(
				pkg.authority,
				`${pkg.source}: must name the concern it owns, so a second owner is visible`,
			);
			const known = authorities.concerns.some((c) => c.concern === pkg.authority);
			assert.ok(
				known,
				`${pkg.source}: claims authority '${pkg.authority}', which is not a concern in harness/authorities.json`,
			);
		}
	});

	it("lets exactly one package own a concern", () => {
		const owners = new Map<string, string[]>();
		for (const pkg of pins.packages) {
			const list = owners.get(pkg.authority ?? "") ?? [];
			list.push(pkg.source);
			owners.set(pkg.authority ?? "", list);
		}
		for (const [concern, sources] of owners) {
			assert.equal(
				sources.length,
				1,
				`${concern} is claimed by ${sources.join(" and ")}; Pi would resolve that silently by load order`,
			);
		}
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
