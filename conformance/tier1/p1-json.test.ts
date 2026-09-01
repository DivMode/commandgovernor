/**
 * P1-JSON — every committed JSON file parses.
 *
 * The check is worth having because most of this distribution's behaviour is
 * carried by JSON that nothing compiles: the pin record, the authorities map,
 * the project settings, the profile templates, the role schema. Pi's own
 * manifest parser returns `null` for a malformed package.json and drops the
 * package silently, so a stray comma would present as "my extension did not
 * load" rather than as a syntax error.
 *
 * The file list comes from walking the tree, not from an inventory, so a JSON
 * file added later joins the check without anyone remembering to add it.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { repoRelative, walkRepoFiles } from "../lib/repo.ts";

describe("P1-JSON: committed JSON is parseable", () => {
	const jsonFiles = walkRepoFiles().filter((path) => path.endsWith(".json"));

	it("finds the JSON files it is supposed to be checking", () => {
		// A walk that silently matched nothing would report a green suite while
		// checking nothing at all.
		const relative = jsonFiles.map(repoRelative);
		for (const expected of [
			"package.json",
			"pins/pins.json",
			"harness/authorities.json",
			"harness/agents/role.schema.json",
			".pi/settings.json",
		]) {
			assert.ok(relative.includes(expected), `expected ${expected} in the JSON sweep`);
		}
	});

	for (const path of jsonFiles) {
		it(`parses ${repoRelative(path)}`, () => {
			const text = readFileSync(path, "utf8");
			assert.doesNotThrow(
				() => JSON.parse(text),
				`${repoRelative(path)} is not valid JSON`,
			);
		});
	}
});
