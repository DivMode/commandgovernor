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
 * (Issue #15 D2; upstream PrimeIntellect-ai/prime-agent#1974). The Governor
 * therefore does NOT ask "does the error text say the socket closed?". It
 * asks two structural questions, in order:
 *
 *   1. Does the failure carry a typed `errorInfo.code` from the pinned
 *      daemon's closed vocabulary (`governor/prime/protocol.ts`)? An untyped
 *      failure -- any `success: false` without one -- proves nothing.
 *   2. Has the PAIR `(commandType, code)` been reviewed against the pinned
 *      source and found to be thrown before the command's external effect
 *      (`governor/mutation/proof.ts`)? A code alone is not proof: the same
 *      `missing_session_cwd` that a `create` might reject with arrives after
 *      `import_jsonl` has already copied a transcript into the session
 *      directory.
 *
 * Only "yes" to both is FAILED, and the verdict carries the review as its
 * proof. Unknown commands, unknown codes and unreviewed pairs classify
 * UNCERTAIN, whatever their message says; a future Prime that renames a
 * message, adds a code, or moves a throw cannot slip past this by wording.
 *
 * The policy is a value so the guard is falsifiable: the conformance suite
 * runs captured responses through {@link DEFAULT_POLICY} and asserts
 * UNCERTAIN, then through {@link NAIVE_POLICY} (any failure is failure) and
 * through {@link LEGACY_GLOBAL_CODE_POLICY} (any serialised code is proof,
 * for any command -- the classifier PR #18 shipped before review) and
 * asserts FAILED, proving each test can tell the policies apart.
 */

import { DAEMON_ERROR_CODES, type DaemonErrorCode, type DaemonResponse, type DaemonSuccessResponse, SERIALIZED_ERROR_CODES } from "../prime/protocol.ts";
import { PRE_EFFECT_PROOF_MATRIX, type ProofMatrix, type ProofReview } from "./proof.ts";

/** What the dispatcher saw after sending a mutating command. `commandType` is what the Governor sent, not what the response claims. */
export type Observation =
	| { readonly kind: "response"; readonly commandType: string; readonly response: DaemonResponse }
	| { readonly kind: "transport_lost"; readonly commandType: string; readonly detail: string }
	| { readonly kind: "timeout"; readonly commandType: string; readonly timeoutMs: number };

export type PreEffectProof = {
	readonly kind: "typed_pre_effect_rejection";
	readonly commandType: string;
	readonly code: DaemonErrorCode;
	/** The review that makes this pair proof; absent only under a non-production policy. */
	readonly review?: ProofReview;
};

export type UncertainReason =
	| "substrate_reported_uncertain"
	| "untyped_failure"
	| "unknown_error_code"
	| "typed_failure_post_effect"
	| "typed_failure_ambiguous"
	| "typed_failure_unreviewed"
	| "transport_lost"
	| "timeout";

export type Verdict =
	| { readonly verdict: "completed"; readonly response: DaemonSuccessResponse }
	| { readonly verdict: "failed"; readonly proof: PreEffectProof; readonly response: DaemonResponse }
	| { readonly verdict: "uncertain"; readonly reason: UncertainReason; readonly response?: DaemonResponse; readonly detail?: string };

export interface ClassificationPolicy {
	/** The reviewed `(commandType, code)` pairs. */
	readonly proofMatrix: ProofMatrix;
	/**
	 * Treat any serialised error code as pre-effect proof for ANY command,
	 * ignoring the matrix. This is the pre-review classifier and exists only
	 * as a negative control; `assertProductionPolicy` refuses it.
	 */
	readonly codeIsGlobalProof: boolean;
	/** What an untyped failure means. The only admissible value in production is "uncertain". */
	readonly untypedFailure: "uncertain" | "failed";
	/** What a lost transport means. The only admissible value in production is "uncertain". */
	readonly transportLoss: "uncertain" | "failed";
}

export const DEFAULT_POLICY: ClassificationPolicy = {
	proofMatrix: PRE_EFFECT_PROOF_MATRIX,
	codeIsGlobalProof: false,
	untypedFailure: "uncertain",
	transportLoss: "uncertain",
};

/**
 * The classifier a naive client would use: any failure is a failure. This is
 * exactly the behaviour that duplicated the external effect in the S1 bake-off
 * and it exists here ONLY as the negative control for the conformance suite.
 */
export const NAIVE_POLICY: ClassificationPolicy = {
	proofMatrix: PRE_EFFECT_PROOF_MATRIX,
	codeIsGlobalProof: true,
	untypedFailure: "failed",
	transportLoss: "failed",
};

/**
 * The classifier this PR shipped before review: untyped failure and transport
 * loss are UNCERTAIN, but every code `serializeDaemonError` can produce is
 * taken as pre-effect proof regardless of the command. Falsified by the
 * `import_jsonl` + `missing_session_cwd` runtime case; kept ONLY as that
 * test's control.
 */
export const LEGACY_GLOBAL_CODE_POLICY: ClassificationPolicy = {
	proofMatrix: PRE_EFFECT_PROOF_MATRIX,
	codeIsGlobalProof: true,
	untypedFailure: "uncertain",
	transportLoss: "uncertain",
};

function isKnownCode(code: string): code is DaemonErrorCode {
	return (DAEMON_ERROR_CODES as readonly string[]).includes(code);
}

export function classifyMutationOutcome(observation: Observation, policy: ClassificationPolicy = DEFAULT_POLICY): Verdict {
	const { commandType } = observation;
	switch (observation.kind) {
		case "transport_lost":
			return policy.transportLoss === "uncertain"
				? { verdict: "uncertain", reason: "transport_lost", detail: observation.detail }
				: { verdict: "failed", proof: assumedProof(commandType), response: syntheticFailure(commandType, observation.detail) };
		case "timeout":
			return policy.transportLoss === "uncertain"
				? { verdict: "uncertain", reason: "timeout", detail: `${observation.timeoutMs} ms` }
				: { verdict: "failed", proof: assumedProof(commandType), response: syntheticFailure(commandType, "timeout") };
		case "response": {
			const { response } = observation;
			if (response.success) return { verdict: "completed", response };
			const code: string | undefined = response.errorInfo?.code;
			if (code === undefined) {
				return policy.untypedFailure === "uncertain"
					? { verdict: "uncertain", reason: "untyped_failure", response }
					: { verdict: "failed", proof: assumedProof(commandType), response };
			}
			if (code === "command_result_uncertain") {
				return { verdict: "uncertain", reason: "substrate_reported_uncertain", response };
			}
			if (!isKnownCode(code)) {
				// The wire guard already rejects this shape; a fabricated record can still carry it.
				return policy.untypedFailure === "uncertain"
					? { verdict: "uncertain", reason: "unknown_error_code", response }
					: { verdict: "failed", proof: assumedProof(commandType), response };
			}
			if (policy.codeIsGlobalProof && SERIALIZED_ERROR_CODES.has(code)) {
				return { verdict: "failed", proof: { kind: "typed_pre_effect_rejection", commandType, code }, response };
			}
			const review = policy.proofMatrix.lookup(commandType, code);
			if (review === undefined) {
				return { verdict: "uncertain", reason: "typed_failure_unreviewed", response, detail: `${commandType} + ${code} has no reviewed proof` };
			}
			// Only a reviewed pre-effect timing is proof. Anything else -- including a
			// timing value this switch has never heard of -- fails closed.
			switch (review.timing) {
				case "pre_effect":
					return { verdict: "failed", proof: { kind: "typed_pre_effect_rejection", commandType, code, review }, response };
				case "post_effect":
					return { verdict: "uncertain", reason: "typed_failure_post_effect", response, detail: review.basis };
				case "ambiguous":
					return { verdict: "uncertain", reason: "typed_failure_ambiguous", response, detail: review.basis };
				default: {
					const unknownTiming: never = review.timing;
					return { verdict: "uncertain", reason: "typed_failure_unreviewed", response, detail: `unknown review timing ${String(unknownTiming)}` };
				}
			}
		}
	}
}

/** The "proof" a naive client implicitly assumes: none. Only reachable under a non-production policy. */
function assumedProof(commandType: string): PreEffectProof {
	return { kind: "typed_pre_effect_rejection", commandType, code: "session_already_active" };
}

function syntheticFailure(commandType: string, detail: string): DaemonResponse {
	return { type: "response", command: commandType, success: false, error: detail };
}

/** Sanity check a policy before it is allowed near a ledger. */
export function assertProductionPolicy(policy: ClassificationPolicy): void {
	if (policy.untypedFailure !== "uncertain" || policy.transportLoss !== "uncertain") {
		throw new Error("classification policy would convert an unproven outcome into a definite failure; refused");
	}
	if (policy.codeIsGlobalProof) {
		throw new Error("classification policy treats an error code as proof regardless of command; refused");
	}
	for (const entry of policy.proofMatrix.entries) {
		if (entry.code === "command_result_uncertain") {
			throw new Error("classification policy lists command_result_uncertain as reviewed proof; refused");
		}
		if (!SERIALIZED_ERROR_CODES.has(entry.code)) {
			throw new Error(`classification policy reviews ${entry.code}, which the pinned serializer cannot produce; refused`);
		}
		if (entry.commandType === "*" || entry.commandType.trim() === "") {
			throw new Error("classification policy has a wildcard command in its proof matrix; refused");
		}
	}
}
