/**
 * JSON — every JSON file in the repository parses. A walk, not a list, so a
 * file added next month joins without anyone remembering.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { readFileSync } from "node:fs";

import { repoRelative, walkRepoFiles } from "../lib/repo.ts";

describe("JSON: every committed JSON document parses", () => {
	const files = walkRepoFiles().filter((path) => path.endsWith(".json"));
	it("finds the documents it is supposed to cover", () => {
		const relative = files.map(repoRelative);
		for (const expected of ["pins/pins.json", "pins/prime-0.8.1/package.json", "pins/prime-0.8.1/package-lock.json", "harness/authorities.json", "harness/agents/role.schema.json", "package.json", "tsconfig.json"]) {
			assert.ok(relative.includes(expected), expected);
		}
	});
	for (const path of files) {
		it(repoRelative(path), () => {
			assert.doesNotThrow(() => JSON.parse(readFileSync(path, "utf8")));
		});
	}
});
