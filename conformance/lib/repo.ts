/**
 * Repository layout helpers shared by every conformance test.
 *
 * One source for the paths. A test that recomputes "where is pins.json" is a
 * second place for the answer to be wrong.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

/** Absolute path to the repository root. */
export const REPO_ROOT: string = join(
	dirname(fileURLToPath(import.meta.url)),
	"..",
	"..",
);

export const PINS_JSON = join(REPO_ROOT, "pins", "pins.json");
export const SHA256SUMS = join(REPO_ROOT, "pins", "SHA256SUMS");
export const PACKAGE_JSON = join(REPO_ROOT, "package.json");
export const PROJECT_SETTINGS = join(REPO_ROOT, ".pi", "settings.json");
export const AUTHORITIES_JSON = join(REPO_ROOT, "harness", "authorities.json");
export const AGENTS_DIR = join(REPO_ROOT, "harness", "agents");

/** Directories that are derived, vendored, or none of our business. */
const SKIP_DIRECTORIES = new Set([
	".git",
	"node_modules",
	"target",
	"dist",
	"build",
	"coverage",
	".claude",
]);

/**
 * Every regular file under the repository, excluding derived trees.
 *
 * Deliberately a walk rather than a hand-written list: a JSON file added next
 * month joins the parse check without anyone remembering to add it. The same
 * reasoning is why the Rust testkit built its sentinel surface list by walking
 * the state root.
 */
export function walkRepoFiles(root: string = REPO_ROOT): string[] {
	const found: string[] = [];
	const visit = (dir: string): void => {
		for (const entry of readdirSync(dir, { withFileTypes: true })) {
			if (entry.isDirectory()) {
				if (SKIP_DIRECTORIES.has(entry.name)) continue;
				visit(join(dir, entry.name));
			} else if (entry.isFile()) {
				found.push(join(dir, entry.name));
			}
		}
	};
	visit(root);
	return found.sort();
}

/** Path relative to the repository root, for readable assertion messages. */
export function repoRelative(absolutePath: string): string {
	return relative(REPO_ROOT, absolutePath).split(sep).join("/");
}

export function readJson(path: string): unknown {
	return JSON.parse(readFileSync(path, "utf8"));
}

export function exists(path: string): boolean {
	try {
		statSync(path);
		return true;
	} catch {
		return false;
	}
}

export function isDirectory(path: string): boolean {
	try {
		return statSync(path).isDirectory();
	} catch {
		return false;
	}
}

/** The pin record, read once per test file. */
export interface PinRecord {
	readonly pi: {
		readonly version: string;
		readonly tag: string;
		readonly commit: string;
		readonly npmIntegrity: string;
		readonly installRoot: string;
		readonly engines: { readonly node: string };
		readonly assets: readonly {
			readonly releaseName: string;
			readonly path: string;
			readonly sha256: string;
		}[];
	};
	readonly packages: readonly PinnedPackage[];
}

export interface PinnedPackage {
	readonly source: string;
	readonly exactVersion?: string;
	readonly resolvedSha?: string;
	readonly license?: string;
	readonly reviewedAt?: string;
	readonly authority?: string;
}

export function readPins(): PinRecord {
	return readJson(PINS_JSON) as PinRecord;
}
