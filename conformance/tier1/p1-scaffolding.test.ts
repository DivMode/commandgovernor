/**
 * P1-SCAFFOLDING — the seams the later gates need, proved to work now.
 *
 * None of this is Gate P1's literal scope. It is here because a harness that
 * can only express P1's assertions cannot express P2-P4's, and retrofitting
 * deterministic time, domain-separated entropy and a restart primitive into an
 * existing suite costs far more than designing for them.
 *
 * The independence assertion below is the one that matters. A harness whose id
 * source and CSPRNG drew from a single counter would make the two agree by
 * construction, and an implementation that derived a correlation secret from an
 * identity would then pass every possession-fence test ever written against it.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
	DEFAULT_CLOCK_START_MS,
	TestClock,
} from "../lib/clock.ts";
import {
	isRedactionSafeDeliveryId,
	SEED_STRIDE,
	SeededStreams,
} from "../lib/ids.ts";
import { RESTART_NOT_IMPLEMENTED, restartLoop } from "../lib/restart.ts";
import {
	DELIVERY_ID_PREFIX,
	REDACTION_HAZARD_RUN_LENGTH,
} from "../../harness/extensions/cg-foreman/transport.ts";

describe("P1-SCAFFOLDING: injected clock", () => {
	it("starts far from any real epoch value", () => {
		// So a timestamp that leaked in from a real clock is obvious rather than
		// plausible.
		assert.equal(new TestClock().now(), DEFAULT_CLOCK_START_MS);
		assert.ok(DEFAULT_CLOCK_START_MS < 1_000_000);
	});

	it("holds still until told to move", () => {
		const clock = new TestClock("frozen");
		assert.equal(clock.now(), clock.now());
		clock.advance(500);
		assert.equal(clock.now(), DEFAULT_CLOCK_START_MS + 500);
	});

	it("advances per reading in stepping mode, not per wall-clock tick", () => {
		const clock = new TestClock("stepping");
		const first = clock.now();
		const second = clock.now();
		assert.equal(second, first + 1);
	});

	it("refuses to run backwards", () => {
		assert.throws(() => new TestClock().advance(-1), /does not run backwards/);
	});

	it("is shared, so time can move while a component holds it", () => {
		const clock = new TestClock("frozen");
		const held: { now(): number } = clock;
		clock.advance(42);
		assert.equal(held.now(), DEFAULT_CLOCK_START_MS + 42);
	});
});

describe("P1-SCAFFOLDING: two domain-separated seeded streams", () => {
	it("is deterministic for a given seed", () => {
		const a = new SeededStreams(7);
		const b = new SeededStreams(7);
		assert.equal(a.nextIdentity(), b.nextIdentity());
		assert.equal(a.nextRandomBits(), b.nextRandomBits());
	});

	it("keeps identity and randomness independent across 1024 seeds", () => {
		// The whole point. If these agreed, an implementation that derived a
		// correlation secret from an identity would be untestable.
		let collisions = 0;
		for (let seed = 0; seed < 1024; seed += 1) {
			const streams = new SeededStreams(seed);
			const identity = streams.nextIdentity();
			const random = streams.nextRandomBits().toString(32).toUpperCase();
			if (identity === random) collisions += 1;
		}
		assert.equal(collisions, 0, "identity and randomness streams agreed");
	});

	it("never repeats a stream across a restart", () => {
		// A harness that replayed the same bytes after a restart would produce
		// two "different" correlation ids that are in fact equal -- the exact
		// bug the durability suites hunt.
		const first = new SeededStreams(11);
		const second = first.nextGeneration();
		assert.equal(second.generation, 1);
		assert.notEqual(first.nextIdentity(), second.nextIdentity());
		assert.equal(SEED_STRIDE, 1_000_003);
	});

	it("generates delivery ids that survive transport readback redaction", () => {
		// A property, over enough draws to cross the failure rate the naive
		// encoding had. Before the generator redrew, roughly 1 in 5,700 ids was
		// rejected by this very predicate -- the `CG-D-` prefix ends in a hyphen,
		// so a random part merely starting with nine digits already forms a
		// ten-character run that the transport's readback replaces wholesale.
		// A few hundred draws would have missed it; 10,000 across many seeds
		// would not.
		let drawn = 0;
		for (let seed = 0; seed < 500; seed += 1) {
			const streams = new SeededStreams(seed);
			for (let i = 0; i < 20; i += 1) {
				const id = streams.nextDeliveryId();
				drawn += 1;
				assert.ok(id.startsWith(DELIVERY_ID_PREFIX), id);
				assert.ok(isRedactionSafeDeliveryId(id), `${id} would be mangled on readback`);
			}
		}
		assert.equal(drawn, 10_000);
	});

	it("stays deterministic despite redrawing", () => {
		// The redraw consumes entropy, so two streams on one seed must still
		// agree -- otherwise the fix would have traded a rare invalid id for an
		// irreproducible harness.
		const a = new SeededStreams(4242);
		const b = new SeededStreams(4242);
		for (let i = 0; i < 64; i += 1) {
			assert.equal(a.nextDeliveryId(), b.nextDeliveryId());
		}
	});

	it("rejects the id shapes a transport's redaction destroys", () => {
		// A purely numeric id is replaced with a <PHONE> placeholder when one
		// candidate transport reads a conversation back, which silently breaks
		// the only correlation primitive the protocol has.
		assert.equal(isRedactionSafeDeliveryId("1234567890123456"), false);
		assert.equal(isRedactionSafeDeliveryId("555-0100-9999-1234"), false);
		assert.equal(isRedactionSafeDeliveryId("CG-D-0000000000000000"), false);
		assert.equal(isRedactionSafeDeliveryId("CG-D-7Q4KZ"), true);
		assert.equal(REDACTION_HAZARD_RUN_LENGTH, 10);
	});
});

describe("P1-SCAFFOLDING: restart primitive", () => {
	it("refuses to pretend it restarted anything", () => {
		// There is no durable Command Governor state to restart against yet, and
		// a stub that passed would report coverage this suite does not have.
		assert.throws(
			() => restartLoop(100, () => {}),
			/restart primitive is not implemented/,
		);
		assert.match(RESTART_NOT_IMPLEMENTED, /Phase B/);
	});
});
