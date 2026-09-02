/**
 * IDS — the seeded identity/randomness streams and the delivery-id rule,
 * transplanted from PR #16 (p1-scaffolding). Kept because the foreman
 * transport types are kept, and their delivery-id encoding constraint is a
 * property of every future ledger row.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { isRedactionSafeDeliveryId, SEED_STRIDE, SeededStreams } from "../lib/ids.ts";
import { DELIVERY_ID_PREFIX } from "../../harness/extensions/cg-foreman/transport.ts";

describe("IDS: seeded streams and delivery ids", () => {
	it("identity and randomness streams are independent and repeatable", () => {
		const a = new SeededStreams(7);
		const b = new SeededStreams(7);
		assert.equal(a.nextIdentity(), b.nextIdentity());
		assert.equal(a.nextRandomBits(), b.nextRandomBits());
		const c = new SeededStreams(7);
		const id = c.nextIdentity();
		const bits = c.nextRandomBits();
		assert.notEqual(BigInt(`0x${Buffer.from(id).toString("hex")}`), bits, "the two streams do not coincide");
		assert.notEqual(new SeededStreams(7).nextGeneration().nextIdentity(), new SeededStreams(7).nextIdentity(), "a restart advances by the stride");
		assert.ok(SEED_STRIDE > 1000);
	});

	it("every generated delivery id satisfies the redaction rule (100k draws)", () => {
		const streams = new SeededStreams(2026);
		for (let i = 0; i < 100_000; i += 1) {
			const id = streams.nextDeliveryId();
			assert.ok(id.startsWith(DELIVERY_ID_PREFIX));
			assert.ok(isRedactionSafeDeliveryId(id), id);
		}
	});

	it("the predicate rejects what the rule forbids", () => {
		assert.equal(isRedactionSafeDeliveryId("CG-D-1234567890"), false);
		assert.equal(isRedactionSafeDeliveryId("1234567890ABC"), false, "a digit run of ten anywhere");
		assert.equal(isRedactionSafeDeliveryId("CG-D-12345678"), true);
		assert.equal(isRedactionSafeDeliveryId("0000000000000"), false);
	});
});
