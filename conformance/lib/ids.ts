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

/** Crockford base32, which is why a generated id always contains letters. */
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

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
	 * A delivery id honouring the transport encoding rule: it must contain
	 * letters, because one candidate transport's readback redaction destroys a
	 * long run of digits and hyphens. Crockford base32 over 128 bits makes a
	 * letter-free result astronomically unlikely, and
	 * {@link isRedactionSafeDeliveryId} is what actually checks it.
	 */
	nextDeliveryId(): string {
		const high = this.nextRandomBits();
		const low = this.nextRandomBits();
		return DELIVERY_ID_PREFIX + encodeCrockford((high << 64n) | low, 26);
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
