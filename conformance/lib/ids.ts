/**
 * Two domain-separated seeded streams: identities, and randomness.
 *
 * They are separate on purpose, and the reason is the only interesting thing in
 * this file. A harness whose id source and CSPRNG drew from one counter would
 * make them agree by construction, and an implementation that derived a
 * correlation secret from an identity -- exactly the bug the possession-fence
 * assertions exist to catch -- would pass every test. Independence is asserted,
 * not assumed; see conformance/tier1/p1-scaffolding.test.ts.
 *
 * SplitMix64 is hand-rolled here rather than taken from a dependency: ten
 * lines, and identical bytes on every machine, runtime and toolchain, which is
 * the entire point of a seeded stream.
 *
 * Production must use OS entropy. Nothing in this file may be imported outside
 * conformance/.
 */

import { DELIVERY_ID_PREFIX } from "../../harness/extensions/cg-foreman/transport.ts";

/** Domain separators. Two streams from one seed must not coincide. */
const IDENTITY_SALT = 0x9e3779b97f4a7c15n;
const RANDOMNESS_SALT = 0xbf58476d1ce4e5b9n;

/**
 * Restart stride.
 *
 * Each new process lifetime derives `seed + generation * SEED_STRIDE`, because
 * a real CSPRNG never repeats across restarts and a harness that did would hide
 * precisely the bug the durability suites hunt: two "different" correlation ids
 * that are in fact equal.
 */
export const SEED_STRIDE = 1_000_003;

const MASK64 = 0xffffffffffffffffn;

class SplitMix64 {
	#state: bigint;

	constructor(seed: bigint) {
		this.#state = seed & MASK64;
	}

	next(): bigint {
		this.#state = (this.#state + 0x9e3779b97f4a7c15n) & MASK64;
		let z = this.#state;
		z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & MASK64;
		z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & MASK64;
		return (z ^ (z >> 31n)) & MASK64;
	}
}

/** Crockford base32: 32 symbols, 22 of which are letters. */
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** Bound on the redraw loop in {@link SeededStreams.nextDeliveryId}. */
const MAX_DELIVERY_ID_DRAWS = 64;

function encodeCrockford(value: bigint, digits: number): string {
	let out = "";
	let remaining = value;
	for (let i = 0; i < digits; i += 1) {
		out = CROCKFORD[Number(remaining & 31n)] + out;
		remaining >>= 5n;
	}
	return out;
}

/**
 * A seeded source of identities and of randomness, kept apart.
 */
export class SeededStreams {
	readonly seed: number;
	readonly generation: number;
	readonly #identity: SplitMix64;
	readonly #randomness: SplitMix64;

	constructor(seed: number, generation = 0) {
		this.seed = seed;
		this.generation = generation;
		const base = BigInt(seed + generation * SEED_STRIDE);
		this.#identity = new SplitMix64(base ^ IDENTITY_SALT);
		this.#randomness = new SplitMix64(base ^ RANDOMNESS_SALT);
	}

	/** The same seed at the next process lifetime. Never the same bytes. */
	nextGeneration(): SeededStreams {
		return new SeededStreams(this.seed, this.generation + 1);
	}

	/** An opaque identity. Ordering and uniqueness only; carries no entropy. */
	nextIdentity(): string {
		return encodeCrockford(this.#identity.next(), 13);
	}

	/** Bits intended to be unguessable in production. */
	nextRandomBits(): bigint {
		return this.#randomness.next();
	}

	/**
	 * A delivery id honouring the transport encoding rule.
	 *
	 * The rule is not "contains a letter somewhere". One candidate transport
	 * replaces any run of ten or more digits, spaces, parentheses and hyphens
	 * with a `<PHONE>` placeholder when it reads a conversation back, and the
	 * `CG-D-` prefix ends in a hyphen -- so an id whose random part merely
	 * *starts* with nine digits already forms a qualifying run and is destroyed,
	 * even though the rest is full of letters. Crockford base32 makes that
	 * unlikely, not impossible: measured at roughly 1 in 5,700 draws.
	 *
	 * "Unlikely" is not a property. Draw until the id actually satisfies
	 * {@link isRedactionSafeDeliveryId}, so the generator cannot emit an id its
	 * own validator rejects. The loop is bounded because a generator that could
	 * spin forever is a worse failure than the one it is fixing.
	 */
	nextDeliveryId(): string {
		for (let attempt = 0; attempt < MAX_DELIVERY_ID_DRAWS; attempt += 1) {
			const high = this.nextRandomBits();
			const low = this.nextRandomBits();
			const candidate =
				DELIVERY_ID_PREFIX + encodeCrockford((high << 64n) | low, 26);
			if (isRedactionSafeDeliveryId(candidate)) return candidate;
		}
		throw new Error(
			`SeededStreams.nextDeliveryId: ${MAX_DELIVERY_ID_DRAWS} consecutive draws ` +
				"were all redaction-unsafe, which is far beyond chance -- the encoding " +
				"or the predicate has changed and one of them is wrong.",
		);
	}
}

/**
 * The delivery-id constraint from the transport research, as a predicate.
 *
 * Rejects an id with no ASCII letter, and one containing a run of ten or more
 * characters drawn only from digits, spaces, parentheses and hyphens -- the
 * shape one transport replaces with a `<PHONE>` placeholder when reading a
 * conversation back.
 */
export function isRedactionSafeDeliveryId(id: string): boolean {
	if (!/[A-Za-z]/.test(id)) return false;
	if (/[0-9 ()-]{10,}/.test(id)) return false;
	return true;
}
