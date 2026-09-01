/**
 * Tier 2 — the credentialed suite.
 *
 * These need a real provider credential and a real agent turn, so they are
 * skipped unless `CG_CONFORMANCE_LIVE=1`. Every one of them is currently a
 * placeholder, and each is written as a `skip` with the reason attached rather
 * than as a passing assertion over a mock.
 *
 * That distinction is the point. Each test below stands in for a claim this
 * distribution has inherited from documentation and has NOT verified. Recording
 * them as skipped keeps the claims visible; implementing them against a fake
 * would convert "we have not checked this" into "this passed", which is the
 * failure mode the whole suite exists to avoid.
 *
 * A note on the first one especially. ADR 0008 and the Pi review both assert
 * that `agent_settled` is non-vetoable and is the correct completion signal.
 * That is an inherited claim. The Rust workspace carries WRK-003 and WRK-004
 * precisely because an analogous claim about a different harness's stop hook
 * turned out to be wrong, and the architecture review recorded a second
 * provider-semantics assumption that had already gone stale once. Verify it
 * empirically before anything depends on it.
 */

import { describe, it } from "node:test";

const LIVE = process.env.CG_CONFORMANCE_LIVE === "1";

const NOT_LIVE =
	"credentialed tier: set CG_CONFORMANCE_LIVE=1 and configure a provider to run";

/** Placeholders carry their own reason, so a skip is never mistaken for a pass. */
function pending(reason: string): string {
	return LIVE ? `NOT IMPLEMENTED: ${reason}` : NOT_LIVE;
}

describe("Tier 2 (credentialed): lifecycle claims that must be verified, not inherited", () => {
	it(
		"agent_settled arrives on a real turn, after any auto-retry and auto-compaction",
		{
			skip: pending(
				"needs a real provider turn plus an induced retry and compaction; " +
					"agent_end can fire while Pi still intends to continue, so the " +
					"assertion is about ordering under load, not about the event existing",
			),
		},
		() => {},
	);

	it(
		"agent_settled cannot be vetoed by an extension",
		{
			skip: pending(
				"the non-vetoability claim is inherited from documentation and has " +
					"a precedent for being wrong (WRK-003/WRK-004). Drive a real turn " +
					"with a handler that attempts to cancel and observe what happens",
			),
		},
		() => {},
	);

	it(
		"a vetoed RPC fork returns success with data.cancelled true, and is handled as such",
		{
			skip: pending(
				"a session_before_fork handler can veto, and Pi answers success:true " +
					"with data.cancelled:true rather than an error. A client that only " +
					"checks `success` silently mis-reports a vetoed fork",
			),
		},
		() => {},
	);

	it(
		"compaction ownership is ours, and is observable as ours",
		{
			skip: pending(
				"Pi resolves competing session_before_compact handlers by load order " +
					"with no error, so ownership must be asserted from the produced " +
					"compaction entry (fromHook) rather than from the settings file. " +
					"Blocked until a compaction owner is chosen -- see " +
					"harness/authorities.json, concern compaction-summary",
			),
		},
		() => {},
	);

	it(
		"a declared tool restriction actually blocks the tool it names",
		{
			skip: pending(
				"harness/agents/*.md tools lists are enforced by whatever extension " +
					"reads them, and today nothing does. Verifying the restriction " +
					"before an enforcement point exists would prove only that the file " +
					"parses. Blocked on the Phase C capability ceiling",
			),
		},
		() => {},
	);
});
