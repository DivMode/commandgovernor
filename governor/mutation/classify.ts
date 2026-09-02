/**
 * Outcome classification for a mutating daemon command (Issue #17, D2).
 *
 * The invariant this file exists to hold:
 *
 *     worker transport lost + outcome not proven  =>  UNCERTAIN, never FAILED
 *
 * and, more generally, a mutation is FAILED only on POSITIVE PROOF that the
 * external effect did not happen.
 *
 * The pinned Prime supervisor records a worker's death mid-command as a
 * definite failure ("Daemon worker socket closed") in its command journal and
 * replays that failure on retry, even when the effect is already on disk
 * (Issue #15 D2). The Governor therefore does NOT ask "does the error text say
 * the socket closed?". It asks the structural question: does this failure
 * carry a typed error code that the supervisor can only emit before the
 * worker receives the command? Prime's error vocabulary is closed and tiny
 * (`governor/prime/protocol.ts`), and an untyped failure -- any `success:
 * false` without such a code -- proves nothing. It classifies as uncertain,
 * whatever its message says, and a future Prime that renames the message
 * cannot slip past this by wording alone.
 *
 * The policy is a value so the guard is falsifiable: the conformance suite
 * runs the D2 reproducer's captured response through {@link DEFAULT_POLICY}
 * and asserts UNCERTAIN, then through {@link NAIVE_POLICY} and asserts FAILED,
 * proving the test can tell the two apart.
 */

import { type DaemonErrorCode, type DaemonResponse, type DaemonSuccessResponse, PRE_EFFECT_ERROR_CODES } from "../prime/protocol.ts";

/** What the dispatcher saw after sending a mutating command. */
export type Observation =
	| { readonly kind: "response"; readonly response: DaemonResponse }
	| { readonly kind: "transport_lost"; readonly detail: string }
	| { readonly kind: "timeout"; readonly timeoutMs: number };

export type PreEffectProof = {
	readonly kind: "typed_pre_effect_rejection";
	readonly code: DaemonErrorCode;
};

export type UncertainReason =
	| "substrate_reported_uncertain"
	| "untyped_failure"
	| "transport_lost"
	| "timeout";

export type Verdict =
	| { readonly verdict: "completed"; readonly response: DaemonSuccessResponse }
	| { readonly verdict: "failed"; readonly proof: PreEffectProof; readonly response: DaemonResponse }
	| { readonly verdict: "uncertain"; readonly reason: UncertainReason; readonly response?: DaemonResponse; readonly detail?: string };

export interface ClassificationPolicy {
	/** Error codes that prove the worker never received the command. */
	readonly preEffectCodes: ReadonlySet<string>;
	/** What an untyped failure means. The only admissible value in production is "uncertain". */
	readonly untypedFailure: "uncertain" | "failed";
	/** What a lost transport means. The only admissible value in production is "uncertain". */
	readonly transportLoss: "uncertain" | "failed";
}

export const DEFAULT_POLICY: ClassificationPolicy = {
	preEffectCodes: PRE_EFFECT_ERROR_CODES,
	untypedFailure: "uncertain",
	transportLoss: "uncertain",
};

/**
 * The classifier a naive client would use: any failure is a failure. This is
 * exactly the behaviour that duplicated the external effect in the S1 bake-off
 * and it exists here ONLY as the negative control for the conformance suite.
 */
export const NAIVE_POLICY: ClassificationPolicy = {
	preEffectCodes: PRE_EFFECT_ERROR_CODES,
	untypedFailure: "failed",
	transportLoss: "failed",
};

export function classifyMutationOutcome(observation: Observation, policy: ClassificationPolicy = DEFAULT_POLICY): Verdict {
	switch (observation.kind) {
		case "transport_lost":
			return policy.transportLoss === "uncertain"
				? { verdict: "uncertain", reason: "transport_lost", detail: observation.detail }
				: { verdict: "failed", proof: { kind: "typed_pre_effect_rejection", code: "session_already_active" }, response: syntheticFailure(observation.detail) };
		case "timeout":
			return policy.transportLoss === "uncertain"
				? { verdict: "uncertain", reason: "timeout", detail: `${observation.timeoutMs} ms` }
				: { verdict: "failed", proof: { kind: "typed_pre_effect_rejection", code: "session_already_active" }, response: syntheticFailure("timeout") };
		case "response": {
			const { response } = observation;
			if (response.success) return { verdict: "completed", response };
			const code = response.errorInfo?.code;
			if (code === "command_result_uncertain") {
				return { verdict: "uncertain", reason: "substrate_reported_uncertain", response };
			}
			if (code !== undefined && policy.preEffectCodes.has(code)) {
				return { verdict: "failed", proof: { kind: "typed_pre_effect_rejection", code }, response };
			}
			if (policy.untypedFailure === "uncertain") {
				return { verdict: "uncertain", reason: "untyped_failure", response };
			}
			// Naive policy: manufacture the "proof" a naive client implicitly assumes.
			return { verdict: "failed", proof: { kind: "typed_pre_effect_rejection", code: "session_already_active" }, response };
		}
	}
}

function syntheticFailure(detail: string): DaemonResponse {
	return { type: "response", command: "unknown", success: false, error: detail };
}

/** Sanity check a policy before it is allowed near a ledger. */
export function assertProductionPolicy(policy: ClassificationPolicy): void {
	if (policy.untypedFailure !== "uncertain" || policy.transportLoss !== "uncertain") {
		throw new Error("classification policy would convert an unproven outcome into a definite failure; refused");
	}
	for (const code of policy.preEffectCodes) {
		if (code === "command_result_uncertain") {
			throw new Error("classification policy lists command_result_uncertain as pre-effect proof; refused");
		}
	}
}
