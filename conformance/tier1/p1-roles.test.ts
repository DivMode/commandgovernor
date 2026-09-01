/**
 * P1-ROLES — the agent role files say what they claim to say.
 *
 * Pi core has no agents concept, so nothing about these files is validated by
 * the runtime: the `tools` and `model` fields bind only whatever extension
 * reads them, and today no extension does. That is recorded in
 * harness/authorities.json as an unassigned concern, and it is exactly why the
 * files need checking here. A role file that nothing validates and nothing
 * enforces is a comment.
 *
 * Two of the checks below run against the *pinned runtime* rather than against
 * a list written from memory: tool names are compared to the built-in tool set
 * the pinned Pi actually constructs, and models are compared to the pinned
 * model catalog. Both therefore fail on a re-pin that moves them, which is the
 * behaviour a pin is for.
 */

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import {
	createCodingTools,
	createReadOnlyTools,
} from "@earendil-works/pi-coding-agent";

import { parseFrontmatter, validate, type SchemaNode } from "../lib/frontmatter.ts";
import { pinnedModelCatalog } from "../lib/pi-runtime.ts";
import { AGENTS_DIR, readJson, REPO_ROOT } from "../lib/repo.ts";

const MODELS = await pinnedModelCatalog();

const SCHEMA = readJson(join(AGENTS_DIR, "role.schema.json")) as SchemaNode;

const ROLE_FILES = readdirSync(AGENTS_DIR)
	.filter((name) => name.endsWith(".md"))
	.sort();

interface Role {
	readonly file: string;
	readonly frontmatter: Record<string, string | string[]>;
	readonly body: string;
}

const ROLES: Role[] = ROLE_FILES.map((file) => {
	const parsed = parseFrontmatter(readFileSync(join(AGENTS_DIR, file), "utf8"), file);
	return { file, frontmatter: parsed.frontmatter, body: parsed.body };
});

/**
 * Built-in tool names, taken from the pinned runtime rather than remembered.
 *
 * Both factories take the working directory the tools will operate against; the
 * repository root is the right answer here because that is where a Command
 * Governor session runs. Only the names are read, but passing a real path keeps
 * this honest rather than constructing tools against a directory that does not
 * exist.
 *
 * The union of the coding and read-only bundles is the complete set reachable
 * from the package's public exports: read, bash, edit, write, grep, find, ls.
 * `powershell` is a built-in too but is only exported through its own factory
 * and is Windows-only, so a role naming it would be rejected here. That is the
 * behaviour we want on this platform, and it is stated rather than accidental.
 */
function builtinToolNames(): Set<string> {
	const names = new Set<string>();
	for (const tool of [
		...createCodingTools(REPO_ROOT),
		...createReadOnlyTools(REPO_ROOT),
	]) {
		names.add(tool.name);
	}
	return names;
}

function modelExists(pattern: string): boolean {
	const slash = pattern.indexOf("/");
	const provider = pattern.slice(0, slash);
	const id = pattern.slice(slash + 1);
	const catalog = MODELS[provider];
	return catalog !== undefined && id in catalog;
}

describe("P1-ROLES: agent role definitions", () => {
	it("ships the four roles the distribution names", () => {
		assert.deepEqual(ROLE_FILES, [
			"implementer.md",
			"researcher.md",
			"reviewer.md",
			"scout.md",
		]);
	});

	for (const role of ROLES) {
		describe(role.file, () => {
			it("validates against harness/agents/role.schema.json", () => {
				const errors = validate(role.frontmatter, SCHEMA);
				assert.deepEqual(errors, [], `${role.file}: ${errors.join("; ")}`);
			});

			it("declares its tools explicitly", () => {
				// Required even when empty. An omitted list would mean "whatever
				// the defaults happen to be", and a resumed worker inheriting new
				// defaults is precisely the silent broadening the reliability
				// contract forbids.
				assert.ok(
					Array.isArray(role.frontmatter.tools),
					`${role.file}: tools must be a list, present even if empty`,
				);
			});

			it("names only tools the pinned runtime provides", () => {
				const builtins = builtinToolNames();
				for (const tool of role.frontmatter.tools as string[]) {
					assert.ok(
						builtins.has(tool),
						`${role.file}: '${tool}' is not a built-in tool of pi ${[...builtins].join("/")}`,
					);
				}
			});

			it("names a model the pinned catalog knows", () => {
				const model = role.frontmatter.model as string;
				assert.ok(
					modelExists(model),
					`${role.file}: '${model}' is not in the pinned pi model catalog`,
				);
			});

			it("has a real body, not just frontmatter", () => {
				assert.ok(
					role.body.trim().length > 200,
					`${role.file}: the system-prompt body is empty or trivial`,
				);
			});

			it("has a filename matching its declared name", () => {
				assert.equal(`${role.frontmatter.name as string}.md`, role.file);
			});
		});
	}

	it("delegates only to roles that exist", () => {
		const known = new Set(ROLES.map((role) => role.frontmatter.name as string));
		for (const role of ROLES) {
			for (const target of role.frontmatter.delegation as string[]) {
				assert.ok(
					known.has(target),
					`${role.file}: delegates to '${target}', which is not a defined role`,
				);
				assert.notEqual(
					target,
					role.frontmatter.name,
					`${role.file}: delegates to itself`,
				);
			}
		}
	});

	it("keeps the reviewer independent", () => {
		// ADR 0008 invariant 8. The rule has to be in the role body, because
		// nothing enforces it at runtime and the body is what the model reads.
		const reviewer = ROLES.find((role) => role.frontmatter.name === "reviewer");
		assert.ok(reviewer, "no reviewer role");
		assert.match(
			reviewer.body,
			/never review work you implemented/i,
			"the reviewer body must state the independence rule",
		);
		assert.match(
			reviewer.body,
			/do not work from the implementer's summary/i,
			"the reviewer body must require primary evidence over the implementer's account",
		);
	});

	it("gives the reviewer no way to edit what it is reviewing", () => {
		const reviewer = ROLES.find((role) => role.frontmatter.name === "reviewer");
		assert.ok(reviewer);
		const tools = reviewer.frontmatter.tools as string[];
		for (const forbidden of ["write", "edit"]) {
			assert.ok(
				!tools.includes(forbidden),
				`the reviewer must not hold the '${forbidden}' tool`,
			);
		}
	});

	it("stops an implementer from spawning its own reviewer", () => {
		const implementer = ROLES.find((role) => role.frontmatter.name === "implementer");
		assert.ok(implementer);
		assert.ok(
			!(implementer.frontmatter.delegation as string[]).includes("reviewer"),
			"an implementer that spawns its own reviewer has self-approved by a longer route",
		);
	});
});

describe("P1-ROLES: the frontmatter reader refuses what it does not understand", () => {
	// A lenient parser would turn a schema violation into a passing test, so the
	// parser's own strictness is asserted rather than assumed.
	it("rejects a missing fence", () => {
		assert.throws(() => parseFrontmatter("name: x\n", "t"), /does not start with/);
	});

	it("rejects an unclosed fence", () => {
		assert.throws(() => parseFrontmatter("---\nname: x\n", "t"), /never closed/);
	});

	it("rejects a duplicate key", () => {
		assert.throws(
			() => parseFrontmatter("---\nname: a\nname: b\n---\nbody\n", "t"),
			/duplicate/,
		);
	});

	it("rejects syntax outside its subset", () => {
		assert.throws(
			() => parseFrontmatter("---\ntools: [a, b]\n---\nbody\n", "t"),
			/flow sequences are not supported/,
		);
		assert.throws(
			() => parseFrontmatter("---\nauthority: |\n---\nbody\n", "t"),
			/literal block scalars/,
		);
	});

	it("reads the shapes the role files actually use", () => {
		const parsed = parseFrontmatter(
			"---\nname: r\ntools: []\ndelegation:\n  - a\n  - b\nauthority: >\n  one\n  two\n---\nbody\n",
			"t",
		);
		assert.deepEqual(parsed.frontmatter.tools, []);
		assert.deepEqual(parsed.frontmatter.delegation, ["a", "b"]);
		assert.equal(parsed.frontmatter.authority, "one two");
		assert.equal(parsed.body.trim(), "body");
	});
});

describe("P1-ROLES: the schema checker refuses what it does not understand", () => {
	it("errors on an unsupported keyword rather than ignoring it", () => {
		const errors = validate({ a: 1 }, {
			type: "object",
			properties: { a: { type: "string", multipleOf: 2 } as SchemaNode },
		});
		assert.ok(
			errors.some((message) => message.includes("unsupported keyword 'multipleOf'")),
			`expected an unsupported-keyword error, got: ${errors.join("; ")}`,
		);
	});

	it("reports the violations it is supposed to report", () => {
		const schema: SchemaNode = {
			type: "object",
			required: ["a"],
			additionalProperties: false,
			properties: { a: { type: "string", pattern: "^x" } },
		};
		assert.deepEqual(validate({}, schema), ["$: missing required property 'a'"]);
		assert.ok(validate({ a: "y" }, schema)[0].includes("does not match"));
		assert.ok(validate({ a: "x", b: 1 }, schema)[0].includes("unexpected property 'b'"));
	});
});
