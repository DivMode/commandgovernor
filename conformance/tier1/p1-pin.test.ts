/**
 * P1-PIN — the pinned Pi is the Pi that was pinned.
 *
 * Coverage note, in the discipline the Rust testkit used: every assertion here
 * is fully proven except the last, which needs the bootstrap to have run. When
 * it has not, the test skips with a stated reason rather than passing.
 */

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { pinnedPiAvailable, runPinnedPi } from "../lib/pi-runtime.ts";
import { exists, readPins, REPO_ROOT, SHA256SUMS } from "../lib/repo.ts";

const pins = readPins();

function sha256(path: string): string {
	return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/** `<sha>  <name>` lines, exactly as the release publishes them. */
function upstreamChecksums(): Map<string, string> {
	const map = new Map<string, string>();
	for (const line of readFileSync(SHA256SUMS, "utf8").split("\n")) {
		const match = /^([0-9a-f]{64})\s+(\S+)$/.exec(line.trim());
		if (match !== null) map.set(match[2], match[1]);
	}
	return map;
}

describe("P1-PIN: pinned runtime provenance", () => {
	it("every committed pin asset matches the checksum upstream published", () => {
		const upstream = upstreamChecksums();
		assert.ok(upstream.size > 0, "pins/SHA256SUMS parsed to nothing");

		for (const asset of pins.pi.assets) {
			const local = join(REPO_ROOT, asset.path);
			assert.ok(exists(local), `pinned asset missing: ${asset.path}`);

			const published = upstream.get(asset.releaseName);
			assert.ok(
				published !== undefined,
				`pins/SHA256SUMS has no line for ${asset.releaseName}`,
			);

			// The two records must agree with each other, and the bytes must agree
			// with both. Checking only "file matches pins.json" would pass happily
			// after someone edited pins.json to match a file they had changed.
			assert.equal(
				asset.sha256,
				published,
				`pins.json sha256 for ${asset.releaseName} disagrees with pins/SHA256SUMS`,
			);
			assert.equal(
				sha256(local),
				published,
				`${asset.path} does not match its published checksum`,
			);
		}
	});

	it("the pin record is internally consistent", () => {
		assert.match(pins.pi.version, /^\d+\.\d+\.\d+$/, "pi.version is not a semver triple");
		assert.equal(
			pins.pi.tag,
			`v${pins.pi.version}`,
			"pi.tag and pi.version describe different releases",
		);
		assert.match(
			pins.pi.commit,
			/^[0-9a-f]{40}$/,
			"pi.commit is not a 40-character commit sha; a tag or branch is not a pin",
		);
		assert.match(pins.pi.npmIntegrity, /^sha512-[A-Za-z0-9+/]+={0,2}$/);
		assert.equal(
			pins.pi.installRoot,
			`pins/pi-${pins.pi.version}`,
			"installRoot does not name the pinned version",
		);

		// The vendored lockfile root declares the same version it claims to pin.
		const lockRoot = JSON.parse(
			readFileSync(join(REPO_ROOT, pins.pi.installRoot, "package.json"), "utf8"),
		) as { version?: string; dependencies?: Record<string, string> };
		assert.equal(lockRoot.version, pins.pi.version);
		assert.equal(
			lockRoot.dependencies?.["@earendil-works/pi-coding-agent"],
			pins.pi.version,
			"the vendored install root does not depend on the pinned pi version exactly",
		);
	});

	it("third-party package pins are immutable, or absent", () => {
		// Pi keeps no lockfile for the packages it installs and treats any git
		// ref as `pinned`, mutable tags included. So the policy has to be checked
		// here or it is not enforced anywhere.
		for (const pkg of pins.packages) {
			assert.ok(
				typeof pkg.source === "string" && pkg.source.length > 0,
				"a pinned package has no source",
			);
			const immutable =
				(pkg.exactVersion !== undefined && /^\d+\.\d+\.\d+/.test(pkg.exactVersion)) ||
				(pkg.resolvedSha !== undefined && /^[0-9a-f]{40}$/.test(pkg.resolvedSha));
			assert.ok(
				immutable,
				`${pkg.source}: needs an exact npm version or a 40-character commit sha; a bare name, branch or tag is not a pin`,
			);
			assert.ok(
				typeof pkg.authority === "string" && pkg.authority.length > 0,
				`${pkg.source}: must name the authority it owns (see harness/authorities.json)`,
			);
		}
	});

	it("the installed binary reports the pinned version", { skip: skipReason() }, async () => {
		const result = await runPinnedPi(["--version"]);
		assert.equal(result.code, 0, `pi --version exited ${result.code}: ${result.stderr}`);
		assert.equal(result.stdout.trim(), pins.pi.version);
	});
});

function skipReason(): string | false {
	return pinnedPiAvailable()
		? false
		: "pinned pi is not installed; run scripts/bootstrap.sh";
}
