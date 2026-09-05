/**
 * JSON — every JSON file in the repository parses. A walk, not a list, so a
 * file added next month joins without anyone remembering.
 *
 * The coverage check that follows is derived from `pins/pins.json` rather than
 * written out, because a hardcoded `pins/prime-<version>/…` is a second place
 * the pinned version lives and it goes stale silently on the next re-pin.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { readFileSync } from "node:fs";

import { readPins, repoRelative, walkRepoFiles } from "../lib/repo.ts";

describe("JSON: every committed JSON document parses", () => {
	const files = walkRepoFiles().filter((path) => path.endsWith(".json"));

	it("finds the documents it is supposed to cover", () => {
		const relative = new Set(files.map(repoRelative));
		const installRoot = readPins().substrate.installRoot;
		const expected = [
			"pins/pins.json",
			`${installRoot}/package.json`,
			`${installRoot}/package-lock.json`,
			"harness/package.json",
			"harness/settings.project.json",
			"package.json",
			"tsconfig.json",
		];
		for (const path of expected) assert.ok(relative.has(path), `${path} is not in the walk`);
	});

	for (const path of files) {
		it(repoRelative(path), () => {
			assert.doesNotThrow(() => JSON.parse(readFileSync(path, "utf8")));
		});
	}
});
