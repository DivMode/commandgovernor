/**
 * ROLES — the agent role files are well-formed and encode the independence
 * rule. Transplanted from PR #16; the model-catalog and tool-set checks that
 * read the pinned Pi runtime are not carried over (Prime's catalog is not a
 * Governor contract in this issue), and the file says so rather than
 * implying coverage it does not have.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { parseFrontmatter } from "../lib/frontmatter.ts";
import { AGENTS_DIR, readJson } from "../lib/repo.ts";

interface Role {
	name: string;
	description: string;
	tools: string[];
	model: string;
	delegation: string[];
	authority: string;
}

const schema = readJson(join(AGENTS_DIR, "role.schema.json")) as { required: string[]; properties: Record<string, { pattern?: string }> };
const files = readdirSync(AGENTS_DIR).filter((name) => name.endsWith(".md"));
const roles = new Map<string, { role: Role; body: string }>();
for (const file of files) {
	const parsed = parseFrontmatter(readFileSync(join(AGENTS_DIR, file), "utf8"), file);
	roles.set(file, { role: parsed.frontmatter as unknown as Role, body: parsed.body });
}

describe("ROLES: harness/agents/*.md", () => {
	it("every role carries every required field with the schema's patterns", () => {
		assert.ok(files.length >= 4);
		for (const [file, { role }] of roles) {
			for (const key of schema.required) assert.ok(key in role, `${file}: missing ${key}`);
			assert.match(role.name, new RegExp(schema.properties.name!.pattern!), file);
			assert.match(role.model, new RegExp(schema.properties.model!.pattern!), file);
			assert.ok(Array.isArray(role.tools) && new Set(role.tools).size === role.tools.length, `${file}: tools unique`);
			assert.ok(Array.isArray(role.delegation), file);
			assert.equal(role.name, file.replace(/\.md$/, ""), `${file}: name matches file`);
		}
	});

	it("delegation targets exist and the graph has no cycles", () => {
		const names = new Set([...roles.values()].map(({ role }) => role.name));
		const edges = new Map([...roles.values()].map(({ role }) => [role.name, role.delegation]));
		for (const [from, targets] of edges) for (const to of targets) assert.ok(names.has(to), `${from} delegates to unknown role ${to}`);
		const visiting = new Set<string>();
		const done = new Set<string>();
		const visit = (name: string): void => {
			if (done.has(name)) return;
			assert.ok(!visiting.has(name), `delegation cycle through ${name}`);
			visiting.add(name);
			for (const next of edges.get(name) ?? []) visit(next);
			visiting.delete(name);
			done.add(name);
		};
		for (const name of names) visit(name);
	});

	it("the reviewer has no mutating tool, delegates to no implementer, and its text states the independence rule", () => {
		const reviewer = roles.get("reviewer.md");
		assert.ok(reviewer);
		for (const tool of reviewer.role.tools) assert.ok(!/write|edit/.test(tool), `reviewer must not hold ${tool}`);
		assert.ok(!reviewer.role.delegation.includes("implementer"));
		assert.match(reviewer.body, /never review work you implemented/i);
		assert.match(reviewer.body, /do not merge|you do not merge/i);
	});

	it("the implementer never approves its own work", () => {
		const implementer = roles.get("implementer.md");
		assert.ok(implementer);
		assert.match(implementer.role.description, /never approves its own work/i);
	});
});
