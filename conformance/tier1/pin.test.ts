/**
 * PIN — the component manifest is internally consistent and matches what is
 * installed.
 *
 * `pins/pins.json` is the only place a version string may live, so this file
 * checks that record against its three independent authorities: the release's
 * own checksum file as committed (`pins/SHA256SUMS`), the install-root
 * lockfile npm actually enforces, and the bytes on disk after bootstrap —
 * including what the installed binary says its own version is. A manifest that
 * agrees only with itself would prove nothing.
 *
 * The daemon protocol recorded here is checked against a LIVE supervisor in
 * `conformance/runtime/d8-explicit-session-path.test.ts`; there is nothing in
 * this repository for it to be compared against locally any more.
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { checkPackagePin, checkPackageSet } from "../lib/policy.ts";
import { exists, primeCliEntry, readJson, readPins, REPO_ROOT, SHA256SUMS } from "../lib/repo.ts";

const pins = readPins();
const substrate = pins.substrate;
const installRoot = join(REPO_ROOT, substrate.installRoot);

interface LockfileEntry {
	version?: string;
	resolved?: string;
	integrity?: string;
}

describe("PIN: component manifest", () => {
	it("names a substrate by exact version, tag and 40-character commit", () => {
		assert.ok(Number.isInteger(pins.schemaVersion) && pins.schemaVersion >= 3, `schemaVersion ${pins.schemaVersion} is older than this reader understands`);
		assert.equal(substrate.name, "prime-agent");
		assert.match(substrate.version, /^\d+\.\d+\.\d+$/);
		assert.equal(substrate.tag, `v${substrate.version}`);
		assert.match(substrate.commit, /^[0-9a-f]{40}$/, "a tag is not a pin; the commit it resolved to is");
		assert.equal(substrate.license, "MIT");
		assert.ok(
			substrate.releaseBaseUrl.startsWith("https://github.com/") && substrate.releaseBaseUrl.includes(`/releases/download/${substrate.tag}/`),
			"assets come from the immutable GitHub release",
		);
	});

	it("keeps every path field pointing inside the pinned install root", () => {
		assert.ok(substrate.installRoot.startsWith("pins/"), substrate.installRoot);
		assert.ok(substrate.vendorDir.startsWith(`${substrate.installRoot}/`), substrate.vendorDir);
		assert.ok(substrate.binary.startsWith(`${substrate.installRoot}/`), substrate.binary);
		assert.ok(substrate.installRoot.includes(substrate.version), `${substrate.installRoot} must be version-scoped so two pins cannot share a tree`);
	});

	it("records every asset with sha256 AND sha512, and the sha256 matches the release checksum file", () => {
		const sums = new Map(
			readFileSync(SHA256SUMS, "utf8")
				.split("\n")
				.filter(Boolean)
				.map((line) => {
					const [sha, name] = line.trim().split(/\s+/);
					return [name, sha] as const;
				}),
		);
		assert.ok(substrate.assets.length >= 4, "wrapper plus three siblings");
		const roles = substrate.assets.map((asset) => asset.role);
		assert.equal(roles.filter((role) => role === "wrapper").length, 1);
		assert.equal(roles.filter((role) => role === "sibling").length, 3);
		for (const asset of substrate.assets) {
			assert.match(asset.sha256, /^[0-9a-f]{64}$/, asset.name);
			assert.match(asset.sha512, /^sha512-[A-Za-z0-9+/]+=*$/, asset.name);
			assert.equal(sums.get(asset.name), asset.sha256, `pins/SHA256SUMS disagrees with pins.json for ${asset.name}`);
			assert.ok(Number.isInteger(asset.bytes) && asset.bytes > 0, asset.name);
			assert.ok(asset.name.includes(substrate.version), `${asset.name} is not an asset of ${substrate.version}`);
		}
		assert.equal(sums.size, substrate.assets.length, "SHA256SUMS lists exactly the pinned assets");
	});

	it("pins each package in the install-root lockfile by the sha512 recorded here; a URL alone is never the authority", () => {
		const lock = readJson(join(installRoot, "package-lock.json")) as { packages: Record<string, LockfileEntry> };
		for (const asset of substrate.assets) {
			const entry = lock.packages[`node_modules/${asset.npmName}`];
			assert.ok(entry, `lockfile has no entry for ${asset.npmName}`);
			assert.equal(entry.version, substrate.version, asset.npmName);
			assert.equal(entry.integrity, asset.sha512, `lockfile integrity for ${asset.npmName} must equal the manifest sha512`);
			if (asset.role === "sibling") {
				assert.equal(entry.resolved, asset.upstreamSpec, "the lock resolves the sibling from the same mirror Prime's own manifest names");
			} else {
				assert.equal(entry.resolved, `file:vendor/${asset.name}`, "the wrapper installs from the verified vendor copy, not from a URL");
			}
		}
	});

	it("matches the installed package, and the installed binary says so itself", () => {
		const packageJson = join(installRoot, "node_modules", "prime-agent", "package.json");
		assert.ok(exists(packageJson), `pinned Prime is not installed at ${substrate.installRoot}; run scripts/bootstrap.sh`);
		const installed = readJson(packageJson) as { name: string; version: string; piConfig?: { configDir?: string } };
		assert.equal(installed.name, "prime-agent");
		assert.equal(installed.version, substrate.version);
		assert.equal(installed.piConfig?.configDir, ".prime/agent", "project state lives under .prime/agent, not .pi");

		// The manifest and the package.json can agree while the binary on disk is
		// something else, so ask the binary. `--version` answers on stderr.
		const reported = spawnSync(process.execPath, [primeCliEntry(pins), "--version"], { encoding: "utf8", timeout: 60_000 });
		assert.equal(reported.status, 0, reported.stderr);
		assert.equal(reported.stderr.trim(), substrate.version, "the installed prime-agent reports a different version than pins.json records");
	});

	it("installs Prime's own siblings at Prime's version and never co-installs upstream Pi", () => {
		const modules = join(installRoot, "node_modules");
		assert.ok(!exists(join(modules, "@earendil-works", "pi-coding-agent")), "the upstream Pi wrapper must not be in the Prime install root");
		for (const asset of substrate.assets) {
			if (asset.role !== "sibling") continue;
			const pkg = readJson(join(modules, ...asset.npmName.split("/"), "package.json")) as { version: string };
			assert.equal(pkg.version, substrate.version, `${asset.npmName} must be Prime's ${substrate.version}, not upstream Pi's`);
		}
		assert.ok(!exists(join(REPO_ROOT, "node_modules", "@earendil-works")), "the repository root node_modules must not carry any @earendil-works tree");
		assert.equal(pins.fallback.coInstall, "forbidden");
	});

	it("keeps the fallback as a record, not an install", () => {
		assert.equal(pins.fallback.package, "@earendil-works/pi-coding-agent");
		assert.match(pins.fallback.version, /^\d+\.\d+\.\d+$/);
		assert.ok(
			!exists(join(REPO_ROOT, "pins", `pi-${pins.fallback.version}`, "package.json")),
			"the upstream-Pi fallback is a recorded escape hatch, not a second installed tree",
		);
	});
});

describe("PIN: third-party package policy", () => {
	const knownConcerns = new Set(pins.concerns.map((entry) => String(entry.concern)));
	/** A syntactically valid npm integrity hash, so the fabricated records below fail on ONE rule each. */
	const SHA512 = "sha512-JF4bj8bSkpkeBloU3pe1yQZXov9LlyR17jeQeH8KTeOLVY7KX2Oz15pN/YQiHLmK/KdUgd/DFfE0+9fUnIq1fQ==";

	it("the checker rejects fabricated pins, so a pass over the real file means something", () => {
		// Negative controls. Each fabricated record violates exactly one rule.
		assert.ok(checkPackagePin({}).some((error) => /no source/.test(error)));
		assert.ok(checkPackagePin({ source: "x", exactVersion: "main", integrity: SHA512, authority: "a", license: "MIT", reviewedAt: "2026-09-04" }).some((error) => /not a pin/.test(error)));
		assert.ok(checkPackagePin({ source: "x", resolvedSha: "abc", integrity: SHA512, authority: "a", license: "MIT", reviewedAt: "2026-09-04" }).some((error) => /not a pin/.test(error)));
		assert.ok(checkPackagePin({ source: "x", exactVersion: "1.0.0", integrity: SHA512, license: "MIT", reviewedAt: "2026-09-04" }).some((error) => /authority/.test(error)));
		assert.ok(checkPackagePin({ source: "x", exactVersion: "1.0.0", integrity: SHA512, authority: "a", reviewedAt: "2026-09-04" }).some((error) => /license/.test(error)));
		assert.ok(checkPackagePin({ source: "x", exactVersion: "1.0.0", integrity: SHA512, authority: "a", license: "MIT" }).some((error) => /reviewed/.test(error)));

		// A version resolves to whatever the registry serves today; the integrity
		// hash is what makes it the same bytes tomorrow.
		assert.ok(
			checkPackagePin({ source: "x", exactVersion: "1.0.0", authority: "a", license: "MIT", reviewedAt: "2026-09-04" }).some((error) => /integrity/.test(error)),
			"a package entry with no integrity hash must be rejected",
		);
		assert.ok(
			checkPackagePin({ source: "x", exactVersion: "1.0.0", integrity: "sha1-deadbeef", authority: "a", license: "MIT", reviewedAt: "2026-09-04" }).some((error) => /integrity/.test(error)),
			"an integrity hash that is not sha512- must be rejected",
		);

		assert.deepEqual(
			checkPackagePin({
				source: "x",
				resolvedSha: "0123456789abcdef0123456789abcdef01234567",
				integrity: SHA512,
				authority: "a",
				license: "MIT",
				reviewedAt: "2026-09-04",
			}),
			[],
			"a fully specified pin must pass, or the checker only ever says no",
		);
	});

	it("rejects an unknown authority and two owners for one concern", () => {
		const concern = [...knownConcerns][0];
		const unknown = checkPackageSet(
			[{ source: "a", exactVersion: "1.0.0", integrity: SHA512, authority: "definitely-not-a-concern", license: "MIT", reviewedAt: "2026-09-04" }],
			knownConcerns,
		);
		assert.ok(unknown.some((error) => /not a concern/.test(error)));
		if (concern === undefined) return; // pins.json declares no concerns; the duplicate case needs one
		const twice = checkPackageSet(
			[
				{ source: "a", exactVersion: "1.0.0", integrity: SHA512, authority: concern, license: "MIT", reviewedAt: "2026-09-04" },
				{ source: "b", exactVersion: "1.0.0", integrity: SHA512, authority: concern, license: "MIT", reviewedAt: "2026-09-04" },
			],
			knownConcerns,
		);
		assert.ok(twice.some((error) => /claimed by a and b/.test(error)), JSON.stringify(twice));
	});

	it("the real packages[] passes", () => {
		assert.deepEqual(checkPackageSet(pins.packages, knownConcerns), []);
	});
});
