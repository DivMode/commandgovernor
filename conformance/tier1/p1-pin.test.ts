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
import { checkPackagePin, checkPackageSet } from "../lib/policy.ts";
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
		const violations = pins.packages.flatMap((pkg) => checkPackagePin(pkg));
		assert.deepEqual(violations, [], violations.join("; "));
	});

	it("would reject the pin shapes Pi accepts but we do not", () => {
		// `packages[]` is empty today, so the loop above evaluates nothing. That
		// is a check which cannot come back negative -- unless the rule is also
		// run against records that break it. These are those records.
		const good = {
			source: "npm:pi-subagents@0.62.0",
			exactVersion: "0.62.0",
			license: "MIT",
			reviewedAt: "2026-09-01",
			authority: "subagent-process-lifecycle",
		};
		assert.deepEqual(checkPackagePin(good), [], "a well-formed pin must pass");

		// A git tag satisfies Pi's own `pinned` flag and is still mutable
		// upstream. This is the exact shape the policy exists to reject.
		const tagged = { ...good, exactVersion: undefined, source: "git:github.com/o/r@v1.2.3" };
		assert.ok(
			checkPackagePin(tagged).some((m) => m.includes("not a pin")),
			"a tag ref must be rejected",
		);

		const branch = { ...good, exactVersion: undefined, source: "git:github.com/o/r@main" };
		assert.ok(checkPackagePin(branch).some((m) => m.includes("not a pin")));

		const bare = { ...good, exactVersion: undefined, source: "npm:pi-subagents" };
		assert.ok(checkPackagePin(bare).some((m) => m.includes("not a pin")));

		const shortSha = { ...good, exactVersion: undefined, resolvedSha: "59d920f" };
		assert.ok(
			checkPackagePin(shortSha).some((m) => m.includes("not a pin")),
			"an abbreviated sha is not a 40-character pin",
		);

		const fullSha = {
			...good,
			exactVersion: undefined,
			resolvedSha: "59d920f935239fc8952709d0891202f16d40c821",
		};
		assert.deepEqual(checkPackagePin(fullSha), [], "a full commit sha is a pin");

		const unowned = { ...good, authority: undefined };
		assert.ok(
			checkPackagePin(unowned).some((m) => m.includes("must name the authority")),
			"a package that names no authority must be rejected",
		);

		const unreviewed = { ...good, license: undefined, reviewedAt: undefined };
		const messages = checkPackagePin(unreviewed).join("; ");
		assert.match(messages, /license/);
		assert.match(messages, /reviewed/);
	});

	it("would reject two packages claiming one concern", () => {
		const concerns = new Set(["subagent-process-lifecycle"]);
		const both = [
			{
				source: "npm:a@1.0.0",
				exactVersion: "1.0.0",
				license: "MIT",
				reviewedAt: "2026-09-01",
				authority: "subagent-process-lifecycle",
			},
			{
				source: "npm:b@2.0.0",
				exactVersion: "2.0.0",
				license: "MIT",
				reviewedAt: "2026-09-01",
				authority: "subagent-process-lifecycle",
			},
		];
		assert.ok(
			checkPackageSet(both, concerns).some((m) => m.includes("is claimed by")),
			"two owners for one concern must be rejected",
		);
		assert.deepEqual(checkPackageSet([both[0]], concerns), []);
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
