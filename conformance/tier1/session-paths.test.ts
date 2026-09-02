/**
 * D8 (pure) — session path preflight and canonicalisation.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { canonicalSessionPath, isAcceptableSessionPath, SessionPathError } from "../../governor/session/paths.ts";

const sessionDir = mkdtempSync(join(tmpdir(), "cg-paths-"));
mkdirSync(join(sessionDir, "nested"));
const linkDir = mkdtempSync(join(tmpdir(), "cg-paths-link-"));
symlinkSync(sessionDir, join(linkDir, "via-link"));

describe("canonicalSessionPath (D8)", () => {
	it("refuses omission, wrong type, empty, relative, non-jsonl, NUL and out-of-tree paths, each with a typed issue", () => {
		const cases: [unknown, string][] = [
			[undefined, "missing"],
			[null, "missing"],
			[42, "not_a_string"],
			["", "empty"],
			["relative.jsonl", "relative"],
			[join(sessionDir, "notes.txt"), "not_jsonl"],
			[`${join(sessionDir, "x.jsonl")}\0`, "contains_nul"],
			[join(tmpdir(), "outside.jsonl"), "outside_session_dir"],
			[join(sessionDir, "..", "escape.jsonl"), "outside_session_dir"],
			[sessionDir, "not_jsonl"],
			[join(sessionDir, "no-such-dir", "x.jsonl"), "parent_missing"],
		];
		for (const [candidate, issue] of cases) {
			assert.throws(() => canonicalSessionPath(candidate, sessionDir), (e: unknown) => e instanceof SessionPathError && e.issue === issue, `${String(candidate)} -> ${issue}`);
			assert.equal(isAcceptableSessionPath(candidate, sessionDir), false);
		}
	});

	it("maps every spelling of one transcript to one canonical string", () => {
		const canonical = canonicalSessionPath(join(sessionDir, "root.jsonl"), sessionDir);
		assert.equal(canonicalSessionPath(join(sessionDir, "nested", "..", "root.jsonl"), sessionDir), canonical);
		assert.equal(canonicalSessionPath(join(sessionDir, ".", "root.jsonl"), sessionDir), canonical);
		assert.equal(canonicalSessionPath(join(linkDir, "via-link", "root.jsonl"), sessionDir), canonical, "a symlinked parent canonicalises to the real directory");
		writeFileSync(canonical, "");
		assert.equal(canonicalSessionPath(join(linkDir, "via-link", "root.jsonl"), sessionDir), canonical, "and still does once the file exists");
	});

	it("accepts nested paths inside the session dir and refuses a sibling dir with a shared prefix", () => {
		assert.ok(isAcceptableSessionPath(join(sessionDir, "nested", "child.jsonl"), sessionDir));
		const sibling = `${sessionDir}-evil`;
		mkdirSync(sibling, { recursive: true });
		assert.throws(() => canonicalSessionPath(join(sibling, "x.jsonl"), sessionDir), (e: unknown) => e instanceof SessionPathError && e.issue === "outside_session_dir");
	});
});
