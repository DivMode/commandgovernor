/**
 * PIN — the component manifest is internally consistent and matches what is
 * installed. Every value the Governor compares against a daemon comes from
 * pins/pins.json; this file checks that record against its own second
 * authority (pins/SHA256SUMS, the release's checksum file verbatim), against
 * the install-root lockfile, and against the installed package.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { PRIME_DAEMON_PROTOCOL } from "../../governor/prime/protocol.ts";
import { exists, readJson, readPins, REPO_ROOT, SHA256SUMS } from "../lib/repo.ts";

const pins = readPins();
const substrate = pins.substrate;

describe("PIN: component manifest", () => {
	it("is schema version 2 and names a Prime Agent substrate by exact version, tag and 40-char commit", () => {
		assert.equal(pins.schemaVersion, 2);
		assert.equal(substrate.name, "prime-agent");
		assert.match(substrate.version, /^\d+\.\d+\.\d+$/);
		assert.equal(substrate.tag, `v${substrate.version}`);
		assert.match(substrate.commit, /^[0-9a-f]{40}$/);
		assert.equal(substrate.license, "MIT");
		assert.ok(substrate.releaseBaseUrl.startsWith("https://github.com/") && substrate.releaseBaseUrl.includes(`/releases/download/${substrate.tag}/`), "assets come from the immutable GitHub release");
	});

	it("records every asset with sha256 AND sha512, and the sha256 matches the release checksum file", () => {
		const sums = new Map(
			readFileSync(SHA256SUMS, "utf8")
				.split("\n")
				.filter(Boolean)
				.map((line) => {
					const [sha, name] = line.trim().split(/\s+/);
					return [name!, sha!] as const;
				}),
		);
		assert.ok(substrate.assets.length >= 4, "wrapper plus three siblings");
		const roles = substrate.assets.map((a) => a.role);
		assert.equal(roles.filter((r) => r === "wrapper").length, 1);
		assert.equal(roles.filter((r) => r === "sibling").length, 3);
		for (const asset of substrate.assets) {
			assert.match(asset.sha256, /^[0-9a-f]{64}$/, asset.name);
			assert.match(asset.sha512, /^sha512-[A-Za-z0-9+/]+=*$/, asset.name);
			assert.equal(sums.get(asset.name), asset.sha256, `pins/SHA256SUMS disagrees with pins.json for ${asset.name}`);
			assert.ok(Number.isInteger(asset.bytes) && asset.bytes > 0);
		}
		assert.equal(sums.size, substrate.assets.length, "SHA256SUMS lists exactly the pinned assets");
	});

	it("pins each sibling in the install-root lockfile by the sha512 recorded here; a URL alone is never the authority", () => {
		const lock = readJson(join(REPO_ROOT, substrate.installRoot, "package-lock.json")) as { packages: Record<string, { version?: string; resolved?: string; integrity?: string }> };
		for (const asset of substrate.assets) {
			const entry = lock.packages[`node_modules/${asset.npmName}`];
			assert.ok(entry, `lockfile has no entry for ${asset.npmName}`);
			assert.equal(entry.version, substrate.version);
			assert.equal(entry.integrity, asset.sha512, `lockfile integrity for ${asset.npmName} must equal the manifest sha512`);
			if (asset.role === "sibling") {
				assert.equal(entry.resolved, asset.upstreamSpec, "the lock resolves the sibling from the same mirror Prime's own manifest names");
			} else {
				assert.equal(entry.resolved, `file:vendor/${asset.name}`, "the wrapper installs from the verified vendor copy");
			}
		}
	});

	it("declares the daemon protocol the Governor speaks", () => {
		assert.equal(substrate.daemonProtocol.name, PRIME_DAEMON_PROTOCOL.name);
		assert.equal(substrate.daemonProtocol.version, PRIME_DAEMON_PROTOCOL.version);
		assert.ok(Number.isInteger(substrate.daemonProtocol.schemaRevision));
	});

	it("matches the installed package (after bootstrap)", () => {
		const packageJson = join(REPO_ROOT, substrate.installRoot, "node_modules", "prime-agent", "package.json");
		assert.ok(exists(packageJson), `pinned Prime is not installed at ${substrate.installRoot}; run scripts/bootstrap.sh`);
		const installed = readJson(packageJson) as { name: string; version: string; piConfig?: { configDir?: string } };
		assert.equal(installed.name, "prime-agent");
		assert.equal(installed.version, substrate.version);
		assert.equal(installed.piConfig?.configDir, ".prime/agent", "project state lives under .prime/agent, not .pi");
	});

	it("never co-installs upstream Pi 0.84.4 next to Prime", () => {
		const installRoot = join(REPO_ROOT, substrate.installRoot, "node_modules");
		assert.ok(!exists(join(installRoot, "@earendil-works", "pi-coding-agent")), "the upstream Pi wrapper must not be in the Prime install root");
		for (const sibling of ["pi-agent-core", "pi-ai", "pi-tui"]) {
			const pkg = readJson(join(installRoot, "@earendil-works", sibling, "package.json")) as { version: string };
			assert.equal(pkg.version, substrate.version, `@earendil-works/${sibling} must be Prime's ${substrate.version}, not upstream Pi's`);
		}
		assert.ok(!exists(join(REPO_ROOT, "node_modules", "@earendil-works")), "the repository root node_modules must not carry any @earendil-works tree");
		assert.equal((pins.fallback as { coInstall?: string } | undefined)?.coInstall, "forbidden");
	});

	it("keeps the fallback as a record, not an install", () => {
		const fallback = pins.fallback as { package: string; version: string; commit: string };
		assert.equal(fallback.package, "@earendil-works/pi-coding-agent");
		assert.match(fallback.commit, /^[0-9a-f]{40}$/);
		assert.ok(!exists(join(REPO_ROOT, "pins", `pi-${fallback.version}`, "package.json")), "the upstream-Pi install root is not part of this tree");
	});
});
