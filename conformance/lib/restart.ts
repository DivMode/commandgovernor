/**
 * The restart primitive — a documented placeholder, deliberately not faked.
 *
 * Every durability invariant in Gates P2-P4 is expressed across a restart:
 * drive a child to terminal or blocked, kill the parent, start a new process
 * lifetime, and assert the owed work and the least-authority loadout both
 * survived. Without this primitive none of those tests can be written at all,
 * which is why it is named now rather than when the first test needs it.
 *
 * It cannot be implemented yet, and it is important to say why rather than to
 * ship something that looks implemented. A restart primitive restarts a process
 * *against durable state*, and Command Governor has no durable state yet: the
 * foreman ledger and the binding store are Phase B, and
 * harness/authorities.json records both concerns as unowned. A stub that
 * "restarted" nothing would pass, and a passing test over an absent mechanism
 * is worse than a missing one -- it is a false claim about coverage.
 *
 * So {@link restartLoop} throws. The contract below is the specification the
 * Phase B implementation must satisfy, and the shape the P2+ suites should be
 * written against.
 */

import { SeededStreams } from "./ids.ts";

/**
 * What a real restart must provide, recorded so it is implemented once rather
 * than re-derived per suite:
 *
 * 1. **The same durable bytes.** A new process lifetime opens the same state
 *    root and reads what the previous lifetime committed. Not a copy, not an
 *    in-memory handoff -- reopening the same bytes is itself under test.
 *
 * 2. **Seeded streams advance by a stride, never repeat.** The new lifetime
 *    uses `SeededStreams.nextGeneration()`. A harness that replayed the same
 *    stream would manufacture two identical correlation ids and hide exactly
 *    the collision the suites hunt for.
 *
 * 3. **Repeatable N times.** The Rust suites run 100 restarts for the
 *    obligation, store and session-lineage cases. The primitive takes a count.
 *
 * 4. **An independent reader.** Assertions after a restart must read through a
 *    connection the writer never held, or "the store shows X" means "the writer
 *    wrote X inside a transaction it still holds", which is a different and
 *    much weaker claim.
 *
 * And the ordering that makes a crash test an oracle rather than a hopeful
 * assertion -- five steps, in this order:
 *
 *   1. arm the fault at a named point *before* building the prefix;
 *   2. fingerprint the whole state through an independent reader;
 *   3. run the operation under test;
 *   4. **fingerprint again before reopening** -- recovery on open would
 *      otherwise mask a half-applied transition, and a refused operation must
 *      have changed nothing at all;
 *   5. reopen, and require replay verification to succeed.
 *
 * Step 4 is the one that gets dropped by someone reimplementing this from
 * memory, and dropping it silently converts the oracle into a smoke test.
 */
export interface RestartContract {
	readonly stateRoot: string;
	readonly streams: SeededStreams;
	readonly generation: number;
}

export const RESTART_NOT_IMPLEMENTED =
	"conformance restart primitive is not implemented: Command Governor has no " +
	"durable state yet (foreman ledger and binding store are Phase B, and " +
	"harness/authorities.json records both concerns as unassigned). Implement " +
	"this against the real sidecar rather than stubbing it -- a restart test " +
	"that restarts nothing reports coverage it does not have.";

/**
 * Run `body` across `times` process lifetimes.
 *
 * @throws Always, until Phase B gives it durable state to restart against.
 */
export function restartLoop(
	_times: number,
	_body: (context: RestartContract) => Promise<void> | void,
): never {
	throw new Error(RESTART_NOT_IMPLEMENTED);
}
