/**
 * P1-MANIFEST — the `pi` manifest says only things Pi will read.
 *
 * Pi's manifest parser (`src/core/pi-manifest.ts`) recognises exactly four
 * resource fields and silently drops anything that is not an array of strings.
 * There is no `agents` field, no `tools` field, and no version constraint: a
 * `"pi": { "minVersion": ... }` entry would be accepted by JSON and ignored by
 * Pi. Every failure mode here is silent, which is why it is asserted.
 */

import assert from "node:assert/strict";
import { join } from "node:path";
import { describe, it } from "node:test";

import { exists, PACKAGE_JSON, readJson, REPO_ROOT } from "../lib/repo.ts";

/** The only keys Pi's manifest parser reads. Anything else is dropped. */
const LEGAL_RESOURCE_FIELDS = ["extensions", "skills", "prompts", "themes"] as const;

interface Manifest {
	name?: string;
	keywords?: string[];
	pi?: Record<string, unknown>;
	peerDependencies?: Record<string, string>;
	engines?: Record<string, string>;
}

/** The five packages Pi makes available to an extension. Nothing else. */
const PI_PROVIDED_PACKAGES = [
	"@earendil-works/pi-coding-agent",
	"@earendil-works/pi-agent-core",
	"@earendil-works/pi-ai",
	"@earendil-works/pi-tui",
	"typebox",
];

describe("P1-MANIFEST: the pi package manifest", () => {
	const manifest = readJson(PACKAGE_JSON) as Manifest;

	it("is discoverable as a pi package", () => {
		assert.ok(
			manifest.keywords?.includes("pi-package"),
			'package.json must carry the "pi-package" keyword',
		);
		assert.ok(manifest.pi, "package.json has no `pi` manifest block");
	});

	it("contains only the four resource keys Pi actually reads", () => {
		const keys = Object.keys(manifest.pi ?? {});
		for (const key of keys) {
			assert.ok(
				(LEGAL_RESOURCE_FIELDS as readonly string[]).includes(key),
				`pi.${key} is not one of ${LEGAL_RESOURCE_FIELDS.join(", ")}; Pi drops it silently`,
			);
		}
		assert.deepEqual(
			[...keys].sort(),
			[...LEGAL_RESOURCE_FIELDS].sort(),
			"every resource key should be declared explicitly, even when its directory is empty",
		);
	});

	it("declares every resource key as an array of strings", () => {
		for (const field of LEGAL_RESOURCE_FIELDS) {
			const value = manifest.pi?.[field];
			assert.ok(Array.isArray(value), `pi.${field} must be an array`);
			assert.ok(value.length > 0, `pi.${field} is empty`);
			for (const entry of value) {
				assert.equal(typeof entry, "string", `pi.${field} must contain only strings`);
			}
		}
	});

	it("points every declared path at something that exists", () => {
		for (const field of LEGAL_RESOURCE_FIELDS) {
			for (const entry of manifest.pi?.[field] as string[]) {
				// No globbing in the declared paths today, so a plain existence
				// check is exact rather than approximate. If a glob is introduced
				// later this assertion must be widened deliberately.
				assert.ok(
					!entry.includes("*") && !entry.startsWith("!"),
					`pi.${field} entry ${entry} uses a glob; this check only understands plain paths`,
				);
				assert.ok(
					exists(join(REPO_ROOT, entry)),
					`pi.${field} points at ${entry}, which does not exist`,
				);
			}
		}
	});

	it("keeps Pi-provided packages as peer dependencies and bundles none of them", () => {
		for (const name of PI_PROVIDED_PACKAGES) {
			assert.equal(
				manifest.peerDependencies?.[name],
				"*",
				`${name} must be a peerDependency at "*"; Pi provides it and bundling it forks the runtime`,
			);
		}
		const record = manifest as Record<string, unknown>;
		for (const forbidden of ["dependencies", "bundledDependencies"]) {
			const value = record[forbidden] as Record<string, unknown> | undefined;
			for (const name of PI_PROVIDED_PACKAGES) {
				assert.ok(
					value?.[name] === undefined,
					`${name} must not appear in ${forbidden}`,
				);
			}
		}
	});

	it("marks the Pi-provided peers optional, so npm does not fetch a second Pi", () => {
		// Measured, not assumed: with these peers mandatory, `npm install` at the
		// repository root resolves them from the registry and installs a second,
		// unpinned copy of the entire Pi tree beside the pinned one. Optional is
		// also the honest declaration -- the host provides them.
		const meta = (manifest as { peerDependenciesMeta?: Record<string, { optional?: boolean }> })
			.peerDependenciesMeta;
		for (const name of PI_PROVIDED_PACKAGES) {
			assert.equal(
				meta?.[name]?.optional,
				true,
				`${name} must be marked optional in peerDependenciesMeta`,
			);
		}
	});

	it("keeps tooling in devDependencies, where a consumer install skips it", () => {
		// `pi install` runs `npm install --omit=dev`, so a devDependency never
		// reaches a consumer. Anything needed at runtime would have to move.
		const dev = (manifest as { devDependencies?: Record<string, string> }).devDependencies;
		assert.ok(dev, "expected devDependencies for the typecheck tooling");
		for (const [name, range] of Object.entries(dev)) {
			assert.match(
				range,
				/^\d+\.\d+\.\d+$/,
				`${name} is pinned as "${range}"; tooling must be an exact version`,
			);
		}
	});

	it("declares the same node floor the pinned runtime requires", () => {
		const pins = readJson(join(REPO_ROOT, "pins", "pins.json")) as {
			pi: { engines: { node: string } };
		};
		assert.equal(
			manifest.engines?.node,
			pins.pi.engines.node,
			"package.json and pins.json disagree about the minimum node version",
		);
	});

	it("keeps agents and profiles out of the manifest", () => {
		// Pi has no agents concept. Declaring `harness/agents` would either be
		// dropped silently or, worse, loaded as some other resource type.
		const declared = Object.values(manifest.pi ?? {}).flat() as string[];
		for (const path of declared) {
			assert.ok(
				!path.includes("agents") && !path.includes("profiles"),
				`${path} is declared to Pi, but Pi has no concept for it`,
			);
		}
	});
});
