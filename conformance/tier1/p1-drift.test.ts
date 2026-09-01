/**
 * P1-DRIFT — drift fails closed, and mutates nothing.
 *
 * Gate P1's literal words are "version drift detected". The Rust store's
 * equivalent (DB-003 plus the A5/A6 taxonomy) adds two properties that are
 * easy to lose and expensive to retrofit: the refusal must be *typed*, so that
 * "you are behind", "you are ahead" and "I cannot tell what you are" are
 * distinguishable, and it must be *non-mutating*, so that a refused start
 * leaves nothing behind for the next start to trip over.
 *
 * Both are asserted here against a fabricated pin record. Nothing in this file
 * touches the real one.
 */

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import {
	evaluateVersion,
	readPinnedVersion,
} from "../../harness/extensions/cg-version-guard.ts";
import { PINS_JSON, readPins } from "../lib/repo.ts";

const PINNED = readPins().pi.version;

/** Write a fabricated pins.json into a throwaway directory. */
function fabricatePins(patch: Record<string, unknown>): {
	path: string;
	dispose: () => void;
} {
	const dir = mkdtempSync(join(tmpdir(), "cg-conformance-pins-"));
	const path = join(dir, "pins.json");
	const doc = JSON.parse(readFileSync(PINS_JSON, "utf8")) as {
		pi: Record<string, unknown>;
	};
	doc.pi = { ...doc.pi, ...patch };
	writeFileSync(path, JSON.stringify(doc, null, 2));
	return { path, dispose: () => rmSync(dir, { recursive: true, force: true }) };
}

function fingerprintRealPins(): string {
	return createHash("sha256").update(readFileSync(PINS_JSON)).digest("hex");
}

describe("P1-DRIFT: a fabricated pin makes the guard refuse", () => {
	it("refuses when the pin record names a different version", () => {
		const { path, dispose } = fabricatePins({ version: "0.99.99" });
		try {
			const required = readPinnedVersion(path);
			assert.equal(required, "0.99.99");

			const verdict = evaluateVersion(PINNED, required);
			assert.equal(verdict.ok, false, "a drifted pin must not be accepted");
			assert.equal(verdict.ok === false && verdict.code, "runtime-version-drift");
		} finally {
			dispose();
		}
	});

	it("refuses in both directions", () => {
		// "Older than the pin" and "newer than the pin" are both drift. A guard
		// that only rejected older versions would let an unreviewed upgrade
		// through, which is the direction that actually happens.
		for (const fabricated of ["0.83.0", "0.85.0"]) {
			const { path, dispose } = fabricatePins({ version: fabricated });
			try {
				const verdict = evaluateVersion(PINNED, readPinnedVersion(path));
				assert.equal(verdict.ok, false, `${fabricated} should be refused`);
			} finally {
				dispose();
			}
		}
	});

	it("refuses a pin record it cannot interpret, rather than guessing", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-conformance-pins-"));
		try {
			const broken = join(dir, "pins.json");

			writeFileSync(broken, "{ this is not json");
			assert.throws(() => readPinnedVersion(broken), /not valid JSON/);

			writeFileSync(broken, JSON.stringify({ pi: {} }));
			assert.throws(() => readPinnedVersion(broken), /no string field pi\.version/);

			writeFileSync(broken, JSON.stringify({ pi: { version: 84 } }));
			assert.throws(() => readPinnedVersion(broken), /no string field pi\.version/);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	it("changes nothing while refusing", () => {
		// Narrow in scope but exact in kind: a refusal must not write. The
		// broader whole-state fingerprint this stands in for arrives with the
		// durable sidecar in Phase B; the shape of the assertion is the part
		// worth establishing now.
		const before = fingerprintRealPins();

		const { path, dispose } = fabricatePins({ version: "0.99.99" });
		try {
			evaluateVersion(PINNED, readPinnedVersion(path));
		} finally {
			dispose();
		}

		assert.equal(fingerprintRealPins(), before, "a refusal mutated the real pin record");
	});
});
