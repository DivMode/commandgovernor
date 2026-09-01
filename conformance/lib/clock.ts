/**
 * An injected clock.
 *
 * Scaffolding for Gates P2-P4, built now because retrofitting a clock seam into
 * an existing suite costs far more than designing for one. Nothing in Tier 1
 * needs deterministic time; claim expiry, retention grace and orphan grace all
 * do, and none of those tests can be written deterministically without this.
 *
 * Two design points carried over from the Rust harness, both of which have a
 * reason that is not obvious:
 *
 * **The default start is 1000 ms.** Small, and nowhere near a real epoch value,
 * so a timestamp that leaked in from a real clock is obvious on sight rather
 * than plausible.
 *
 * **Stepping mode advances per *reading*, not per wall-clock tick.** Total
 * elapsed time becomes a function of how many instants the code under test
 * asked for, which is a property of the code, not of how loaded the machine
 * was. A test that is slow must not become a test that is different.
 */

export interface Clock {
	/** Milliseconds since the fixture epoch. */
	now(): number;
}

export type ClockMode = "frozen" | "stepping";

export const DEFAULT_CLOCK_START_MS = 1_000;

/**
 * A clock the test holds and the code under test also holds.
 *
 * Both hold the same object, so time can move while a component is open. A
 * clock handed out by value would freeze at construction and quietly disable
 * every expiry test.
 */
export class TestClock implements Clock {
	#current: number;
	readonly #mode: ClockMode;

	constructor(mode: ClockMode = "frozen", startMs: number = DEFAULT_CLOCK_START_MS) {
		this.#mode = mode;
		this.#current = startMs;
	}

	now(): number {
		if (this.#mode === "stepping") {
			this.#current += 1;
			return this.#current - 1;
		}
		return this.#current;
	}

	/** Move time forward explicitly. Negative advances are a test bug. */
	advance(ms: number): void {
		if (ms < 0) throw new Error("TestClock.advance: time does not run backwards");
		this.#current += ms;
	}

	/** Read the current instant without consuming a step. */
	peek(): number {
		return this.#current;
	}
}
