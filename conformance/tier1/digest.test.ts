/**
 * DIGEST — the canonical command digest is insensitive to spelling and
 * sensitive to meaning.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { COMMAND_DIGEST_PATTERN, canonicalJson, commandDigest } from "../../governor/mutation/digest.ts";

describe("DIGEST: canonical JSON", () => {
	it("sorts object keys recursively and keeps array order", () => {
		assert.equal(canonicalJson({ b: 1, a: { d: [3, { z: 1, y: 2 }], c: null } }), '{"a":{"c":null,"d":[3,{"y":2,"z":1}]},"b":1}');
		assert.notEqual(canonicalJson([1, 2]), canonicalJson([2, 1]));
	});

	it("omits undefined members like JSON.stringify, and refuses what JSON cannot carry", () => {
		assert.equal(canonicalJson({ a: undefined, b: 1 }), '{"b":1}');
		assert.equal(canonicalJson([undefined, 1]), "[null,1]");
		assert.throws(() => canonicalJson(undefined), TypeError);
		assert.throws(() => canonicalJson({ a: Number.NaN }), TypeError);
		assert.throws(() => canonicalJson({ a: 1n }), TypeError);
	});

	it("honours toJSON, so a Date digests as its ISO string", () => {
		const at = new Date("2026-09-01T00:00:00.000Z");
		assert.equal(canonicalJson({ at }), '{"at":"2026-09-01T00:00:00.000Z"}');
	});

	it("escapes strings exactly as JSON does", () => {
		assert.equal(canonicalJson({ s: 'a"b\\c\n ' }), JSON.stringify({ s: 'a"b\\c\n ' }));
	});
});

describe("DIGEST: commandDigest", () => {
	const command = { type: "execute_bash_and_wait", activeSessionId: "a", command: "echo effect >> /tmp/x" };

	it("is a sha256 and equal for every spelling of the same command", () => {
		const digest = commandDigest(command);
		assert.match(digest, COMMAND_DIGEST_PATTERN);
		assert.equal(commandDigest({ command: command.command, type: command.type, activeSessionId: "a" }), digest);
		assert.equal(commandDigest(JSON.parse(JSON.stringify(command)) as typeof command), digest, "a round trip through the wire format is the same command");
	});

	it("differs for any change to any field, including the ones a type check ignores", () => {
		const digest = commandDigest(command);
		assert.notEqual(commandDigest({ ...command, command: "echo effect >> /tmp/y" }), digest, "the body");
		assert.notEqual(commandDigest({ ...command, activeSessionId: "b" }), digest, "the incarnation");
		assert.notEqual(commandDigest({ ...command, extra: 1 }), digest, "an added field");
		assert.notEqual(commandDigest({ type: command.type, command: command.command }), digest, "a removed field");
		assert.notEqual(commandDigest({ ...command, launchEnv: { HOME: "/h" } }), commandDigest({ ...command, launchEnv: { HOME: "/other" } }), "an environment value");
	});
});
