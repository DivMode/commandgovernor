/**
 * Repository layout helpers shared by every conformance test, plus the typed
 * reader for `pins/pins.json`.
 *
 * One source for the paths and one source for the pin record. A test that
 * recomputes "where is pins.json", or that hardcodes a version string, is a
 * second place for the answer to be wrong. Nothing here imports a runtime
 * module: the suite is black-box against the pinned substrate, so the only
 * thing it may know about Prime is what the pin record says.
 */

import { readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

/** Absolute path to the repository root. */
export const REPO_ROOT: string = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

export const PINS_JSON = join(REPO_ROOT, "pins", "pins.json");
export const SHA256SUMS = join(REPO_ROOT, "pins", "SHA256SUMS");
/** The Command Governor package: skills, prompts, roles and project settings. */
export const HARNESS_DIR = join(REPO_ROOT, "harness");

/** Directories that are derived, vendored, or none of our business. */
const SKIP_DIRECTORIES = new Set([".git", "node_modules", "target", "dist", "build", "coverage", ".claude", "vendor"]);

/**
 * Every regular file under the repository, excluding derived trees.
 *
 * Deliberately a walk rather than a hand-written list: a JSON file added next
 * month joins the parse check without anyone remembering to add it.
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

// ---------------------------------------------------------------------------
// pins/pins.json
// ---------------------------------------------------------------------------

export interface PinnedAsset {
	readonly name: string;
	readonly role: "wrapper" | "sibling";
	readonly npmName: string;
	readonly upstreamSpec?: string;
	readonly sha256: string;
	readonly sha512: string;
	readonly bytes: number;
}

export interface DaemonProtocolPin {
	readonly name: string;
	readonly version: number;
	readonly schemaRevision: number;
}

export interface SubstratePin {
	readonly name: string;
	readonly displayName: string;
	readonly version: string;
	readonly tag: string;
	readonly repository: string;
	readonly commit: string;
	readonly license: string;
	readonly releaseBaseUrl: string;
	/** Repo-relative install root, e.g. `pins/prime-0.9.1`. */
	readonly installRoot: string;
	/** Repo-relative vendor directory holding the verified tarballs. */
	readonly vendorDir: string;
	/** Repo-relative path to the installed `prime-agent` binary. */
	readonly binary: string;
	readonly daemonProtocol: DaemonProtocolPin;
	readonly engines: { readonly node: string };
	readonly assets: readonly PinnedAsset[];
}

export interface FallbackPin {
	readonly package: string;
	readonly version: string;
	readonly coInstall: string;
	readonly commit?: string;
}

/**
 * A third-party package pin, as recorded. Every field is optional here on
 * purpose: `conformance/lib/policy.ts` is the authority on what a valid pin
 * looks like, and it can only reject a bad record if the reader is willing to
 * hand it one.
 */
export interface PinnedPackage {
	readonly source?: unknown;
	readonly exactVersion?: unknown;
	readonly resolvedSha?: unknown;
	readonly license?: unknown;
	readonly reviewedAt?: unknown;
	readonly authority?: unknown;
}

/**
 * One concern and the single authority that owns it, as recorded. Permissive
 * for the same reason as `PinnedPackage`: `conformance/lib/policy.ts` decides
 * what a valid record is, and it can only reject one if the reader hands it
 * over unjudged.
 */
export interface PinnedConcern {
	readonly concern?: unknown;
	readonly status?: unknown;
	readonly disposition?: unknown;
	readonly owner?: unknown;
	readonly plannedOwner?: unknown;
	readonly phase?: unknown;
	readonly removalCondition?: unknown;
	readonly note?: unknown;
}

export interface PinRecord {
	readonly schemaVersion: number;
	readonly substrate: SubstratePin;
	readonly fallback: FallbackPin;
	readonly packages: readonly PinnedPackage[];
	/** One authority per concern. Empty if the manifest does not carry them. */
	readonly concerns: readonly PinnedConcern[];
}

function fail(message: string): never {
	throw new Error(`pins/pins.json: ${message}`);
}

function asRecord(value: unknown, where: string): Record<string, unknown> {
	if (typeof value !== "object" || value === null || Array.isArray(value)) fail(`${where} must be an object`);
	return value as Record<string, unknown>;
}

function asString(value: unknown, where: string): string {
	if (typeof value !== "string" || value.length === 0) fail(`${where} must be a non-empty string`);
	return value;
}

function asInteger(value: unknown, where: string): number {
	if (typeof value !== "number" || !Number.isInteger(value)) fail(`${where} must be an integer`);
	return value;
}

function asArray(value: unknown, where: string): unknown[] {
	if (!Array.isArray(value)) fail(`${where} must be an array`);
	return value;
}

function readAsset(raw: unknown, index: number): PinnedAsset {
	const where = `substrate.assets[${index}]`;
	const doc = asRecord(raw, where);
	const role = asString(doc.role, `${where}.role`);
	if (role !== "wrapper" && role !== "sibling") fail(`${where}.role must be "wrapper" or "sibling"`);
	const asset: PinnedAsset = {
		name: asString(doc.name, `${where}.name`),
		role,
		npmName: asString(doc.npmName, `${where}.npmName`),
		sha256: asString(doc.sha256, `${where}.sha256`),
		sha512: asString(doc.sha512, `${where}.sha512`),
		bytes: asInteger(doc.bytes, `${where}.bytes`),
		...(doc.upstreamSpec === undefined ? {} : { upstreamSpec: asString(doc.upstreamSpec, `${where}.upstreamSpec`) }),
	};
	return asset;
}

function readPackage(raw: unknown, index: number): PinnedPackage {
	const where = `packages[${index}]`;
	const doc = asRecord(raw, where);
	// Deliberately permissive: policy.ts is the authority on what a valid
	// package pin looks like, and it must be able to see an invalid one.
	return doc as PinnedPackage;
}

/**
 * Parse and validate `pins/pins.json`.
 *
 * The shape is checked here so every consumer can rely on it, and so a
 * malformed manifest fails with one clear message rather than as an
 * `undefined` deep inside an assertion.
 */
export function readPins(path: string = PINS_JSON): PinRecord {
	const doc = asRecord(readJson(path), "document");
	const substrateDoc = asRecord(doc.substrate, "substrate");
	const protocolDoc = asRecord(substrateDoc.daemonProtocol, "substrate.daemonProtocol");
	const enginesDoc = asRecord(substrateDoc.engines, "substrate.engines");
	const fallbackDoc = asRecord(doc.fallback, "fallback");

	return {
		schemaVersion: asInteger(doc.schemaVersion, "schemaVersion"),
		substrate: {
			name: asString(substrateDoc.name, "substrate.name"),
			displayName: asString(substrateDoc.displayName, "substrate.displayName"),
			version: asString(substrateDoc.version, "substrate.version"),
			tag: asString(substrateDoc.tag, "substrate.tag"),
			repository: asString(substrateDoc.repository, "substrate.repository"),
			commit: asString(substrateDoc.commit, "substrate.commit"),
			license: asString(substrateDoc.license, "substrate.license"),
			releaseBaseUrl: asString(substrateDoc.releaseBaseUrl, "substrate.releaseBaseUrl"),
			installRoot: asString(substrateDoc.installRoot, "substrate.installRoot"),
			vendorDir: asString(substrateDoc.vendorDir, "substrate.vendorDir"),
			binary: asString(substrateDoc.binary, "substrate.binary"),
			daemonProtocol: {
				name: asString(protocolDoc.name, "substrate.daemonProtocol.name"),
				version: asInteger(protocolDoc.version, "substrate.daemonProtocol.version"),
				schemaRevision: asInteger(protocolDoc.schemaRevision, "substrate.daemonProtocol.schemaRevision"),
			},
			engines: { node: asString(enginesDoc.node, "substrate.engines.node") },
			assets: asArray(substrateDoc.assets, "substrate.assets").map(readAsset),
		},
		fallback: {
			package: asString(fallbackDoc.package, "fallback.package"),
			version: asString(fallbackDoc.version, "fallback.version"),
			coInstall: asString(fallbackDoc.coInstall, "fallback.coInstall"),
			...(fallbackDoc.commit === undefined ? {} : { commit: asString(fallbackDoc.commit, "fallback.commit") }),
		},
		packages: asArray(doc.packages, "packages").map(readPackage),
		concerns: (doc.concerns === undefined ? [] : asArray(doc.concerns, "concerns")).map((raw, index) => asRecord(raw, `concerns[${index}]`) as PinnedConcern),
	};
}

/** Absolute path for a repo-relative pin field. */
export function pinPath(relativePath: string): string {
	return isAbsolute(relativePath) ? relativePath : join(REPO_ROOT, relativePath);
}

/**
 * The Node entry point of the pinned `prime-agent`.
 *
 * `substrate.binary` is npm's `.bin` symlink; the conformance harness spawns
 * the real script with this process's own `node`, so the pinned CLI never
 * depends on a `prime-agent` being on PATH or on a shebang resolution.
 */
export function primeCliEntry(pins: PinRecord = readPins()): string {
	const binary = pinPath(pins.substrate.binary);
	if (!exists(binary)) {
		throw new Error(`the pinned ${pins.substrate.name} is not installed at ${pins.substrate.binary}; run scripts/bootstrap.sh`);
	}
	return realpathSync(binary);
}
