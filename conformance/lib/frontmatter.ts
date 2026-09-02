/**
 * A deliberately small YAML-frontmatter reader, and a deliberately small JSON
 * Schema checker.
 *
 * Neither is a general implementation, and both fail loudly on anything outside
 * the subset they claim. That is the point. A lenient parser that silently
 * accepts syntax it does not understand turns a schema violation into a passing
 * test, which is the failure mode this file exists to prevent.
 *
 * Adding a real YAML or JSON Schema dependency would be defensible later. It is
 * not defensible now: the conformance suite runs on `node --test` with no
 * package dependencies at all, and the surface being parsed is four files this
 * repository writes.
 */

// ---------------------------------------------------------------------------
// Frontmatter
// ---------------------------------------------------------------------------

export type YamlValue = string | string[];
export type Frontmatter = Record<string, YamlValue>;

export interface ParsedDocument {
	readonly frontmatter: Frontmatter;
	readonly body: string;
}

/**
 * The supported subset, in full:
 *
 *   key: value            a plain scalar
 *   key: []               an empty flow sequence
 *   key: >                a folded block scalar; indented lines follow
 *   key:                  a block sequence; `- item` lines follow
 *     - item
 *   # comment             ignored
 *
 * Anything else throws. Flow mappings, quoted keys, anchors, multi-document
 * streams, `|` literal scalars and nested structures are all unsupported and
 * are reported as such rather than guessed at.
 */
export function parseFrontmatter(source: string, label: string): ParsedDocument {
	const normalized = source.replace(/\r\n/g, "\n");
	if (!normalized.startsWith("---\n")) {
		throw new Error(`${label}: file does not start with a '---' frontmatter fence`);
	}
	const end = normalized.indexOf("\n---\n", 3);
	if (end === -1) {
		throw new Error(`${label}: frontmatter fence is never closed`);
	}

	const block = normalized.slice(4, end + 1);
	const body = normalized.slice(end + 5);
	const lines = block.split("\n");
	const frontmatter: Frontmatter = {};

	let index = 0;
	while (index < lines.length) {
		const line = lines[index];
		index += 1;

		if (line.trim() === "" || line.trimStart().startsWith("#")) continue;

		if (/^\s/.test(line)) {
			throw new Error(`${label}: unexpected indented line outside a block: ${line.trim()}`);
		}

		const match = /^([A-Za-z][A-Za-z0-9_-]*):(.*)$/.exec(line);
		if (match === null) {
			throw new Error(`${label}: cannot parse frontmatter line: ${line}`);
		}
		const key = match[1];
		const rest = match[2].trim();

		if (key in frontmatter) {
			throw new Error(`${label}: duplicate frontmatter key '${key}'`);
		}

		if (rest === ">") {
			const parts: string[] = [];
			while (index < lines.length && (/^\s+\S/.test(lines[index]) || lines[index].trim() === "")) {
				parts.push(lines[index].trim());
				index += 1;
			}
			frontmatter[key] = parts.join(" ").replace(/\s+/g, " ").trim();
			continue;
		}

		if (rest === "|") {
			throw new Error(`${label}: literal block scalars ('|') are not supported; use '>'`);
		}

		if (rest === "[]") {
			frontmatter[key] = [];
			continue;
		}

		if (rest === "") {
			const items: string[] = [];
			while (index < lines.length) {
				const candidate = lines[index];
				if (candidate.trim() === "") {
					index += 1;
					continue;
				}
				const item = /^\s+-\s+(.*)$/.exec(candidate);
				if (item === null) break;
				items.push(item[1].trim());
				index += 1;
			}
			if (items.length === 0) {
				throw new Error(
					`${label}: key '${key}' has no value and no block sequence beneath it`,
				);
			}
			frontmatter[key] = items;
			continue;
		}

		if (rest.startsWith("[")) {
			throw new Error(
				`${label}: non-empty flow sequences are not supported; use a block sequence`,
			);
		}

		frontmatter[key] = rest;
	}

	return { frontmatter, body };
}

// ---------------------------------------------------------------------------
// JSON Schema (the subset the role schema uses)
// ---------------------------------------------------------------------------

export interface SchemaNode {
	type?: string;
	properties?: Record<string, SchemaNode>;
	required?: string[];
	additionalProperties?: boolean;
	items?: SchemaNode;
	pattern?: string;
	minLength?: number;
	uniqueItems?: boolean;
	enum?: unknown[];
	[keyword: string]: unknown;
}

/** Keywords this checker understands. Anything else in a schema is an error. */
const SUPPORTED_KEYWORDS = new Set([
	"$schema",
	"$id",
	"title",
	"description",
	"type",
	"properties",
	"required",
	"additionalProperties",
	"items",
	"pattern",
	"minLength",
	"uniqueItems",
	"enum",
]);

/**
 * Validate `value` against `schema`, returning every violation.
 *
 * Fails closed on an unrecognised keyword: a schema that grows a constraint
 * this checker cannot apply must surface as an error in the first test that
 * reaches it, never as a silently weaker check.
 */
export function validate(
	value: unknown,
	schema: SchemaNode,
	path = "$",
): string[] {
	const errors: string[] = [];

	for (const keyword of Object.keys(schema)) {
		if (!SUPPORTED_KEYWORDS.has(keyword)) {
			errors.push(
				`${path}: schema uses unsupported keyword '${keyword}'; extend conformance/lib/frontmatter.ts rather than skipping it`,
			);
		}
	}

	if (schema.type === "object") {
		if (typeof value !== "object" || value === null || Array.isArray(value)) {
			return [...errors, `${path}: expected an object`];
		}
		const record = value as Record<string, unknown>;
		for (const key of schema.required ?? []) {
			if (!(key in record)) errors.push(`${path}: missing required property '${key}'`);
		}
		if (schema.additionalProperties === false) {
			for (const key of Object.keys(record)) {
				if (!(schema.properties && key in schema.properties)) {
					errors.push(`${path}: unexpected property '${key}'`);
				}
			}
		}
		for (const [key, subschema] of Object.entries(schema.properties ?? {})) {
			if (key in record) {
				errors.push(...validate(record[key], subschema, `${path}.${key}`));
			}
		}
		return errors;
	}

	if (schema.type === "array") {
		if (!Array.isArray(value)) return [...errors, `${path}: expected an array`];
		if (schema.uniqueItems === true) {
			const seen = new Set(value.map((item) => JSON.stringify(item)));
			if (seen.size !== value.length) errors.push(`${path}: contains duplicate entries`);
		}
		if (schema.items) {
			value.forEach((item, i) => {
				errors.push(...validate(item, schema.items as SchemaNode, `${path}[${i}]`));
			});
		}
		return errors;
	}

	if (schema.type === "string") {
		if (typeof value !== "string") return [...errors, `${path}: expected a string`];
		if (schema.minLength !== undefined && value.length < schema.minLength) {
			errors.push(`${path}: shorter than minLength ${schema.minLength}`);
		}
		if (schema.pattern !== undefined && !new RegExp(schema.pattern).test(value)) {
			errors.push(`${path}: does not match ${schema.pattern} (value: ${JSON.stringify(value)})`);
		}
		if (schema.enum !== undefined && !schema.enum.includes(value)) {
			errors.push(`${path}: not one of ${JSON.stringify(schema.enum)}`);
		}
		return errors;
	}

	if (schema.type !== undefined) {
		errors.push(`${path}: schema uses unsupported type '${schema.type}'`);
	}
	return errors;
}
