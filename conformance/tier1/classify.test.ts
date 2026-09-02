/**
 * D2 (pure) — the outcome classifier, exercised over fabricated responses.
 *
 * The runtime tier proves the invariant against the real daemon; this file
 * proves the decision procedure over every shape it can be handed, including
 * the shapes that would tempt a wording-based guard.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { assertProductionPolicy, classifyMutationOutcome, DEFAULT_POLICY, NAIVE_POLICY } from "../../governor/mutation/classify.ts";
import type { DaemonResponse } from "../../governor/prime/protocol.ts";

const failure = (error: string, errorInfo?: DaemonResponse extends infer R ? (R extends { errorInfo?: infer E } ? E : never) : never): DaemonResponse =>
	({ type: "response", command: "execute_bash_and_wait", success: false, error, ...(errorInfo ? { errorInfo } : {}) }) as DaemonResponse;

describe("classifyMutationOutcome (D2)", () => {
	it("success is completed", () => {
		const verdict = classifyMutationOutcome({ kind: "response", response: { type: "response", command: "x", success: true, data: 1 } });
		assert.equal(verdict.verdict, "completed");
	});

	it("the bake-off's exact stored failure is uncertain, whatever it says", () => {
		for (const text of ["Daemon worker socket closed", "daemon worker socket closed", "Worker connection lost", "Session worker is not connected", "", "something entirely new"]) {
			const verdict = classifyMutationOutcome({ kind: "response", response: failure(text) });
			assert.equal(verdict.verdict, "uncertain", text);
			assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "untyped_failure");
		}
	});

	it("the substrate's own uncertain code is uncertain", () => {
		const verdict = classifyMutationOutcome({ kind: "response", response: failure("The previous command result is uncertain and was not replayed", { code: "command_result_uncertain", clientId: "c", commandId: "k" }) });
		assert.equal(verdict.verdict, "uncertain");
		assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "substrate_reported_uncertain");
	});

	it("a typed pre-effect rejection is the only failed verdict, and it carries its proof", () => {
		const verdict = classifyMutationOutcome({ kind: "response", response: failure("already active", { code: "session_already_active", sessionPath: "/x.jsonl", activeSessionId: "a" }) });
		assert.equal(verdict.verdict, "failed");
		assert.deepEqual(verdict.verdict === "failed" ? verdict.proof : undefined, { kind: "typed_pre_effect_rejection", code: "session_already_active" });
		for (const code of ["missing_session_cwd", "session_import_file_not_found"] as const) {
			const v = classifyMutationOutcome({ kind: "response", response: failure("pre", code === "missing_session_cwd" ? { code, issue: {} } : { code, filePath: "/f" }) });
			assert.equal(v.verdict, "failed", code);
		}
	});

	it("an errorInfo with an unknown code is not proof (a fabricated record must not pass as pre-effect)", () => {
		const forged = { type: "response", command: "x", success: false, error: "nope", errorInfo: { code: "worker_definitely_did_nothing" } } as unknown as DaemonResponse;
		const verdict = classifyMutationOutcome({ kind: "response", response: forged });
		assert.equal(verdict.verdict, "uncertain");
	});

	it("transport loss and timeout are uncertain", () => {
		assert.equal(classifyMutationOutcome({ kind: "transport_lost", detail: "socket closed" }).verdict, "uncertain");
		assert.equal(classifyMutationOutcome({ kind: "timeout", timeoutMs: 5 }).verdict, "uncertain");
	});

	it("negative control: the naive policy calls the same inputs failed, so the tests above can fail", () => {
		assert.equal(classifyMutationOutcome({ kind: "response", response: failure("Daemon worker socket closed") }, NAIVE_POLICY).verdict, "failed");
		assert.equal(classifyMutationOutcome({ kind: "transport_lost", detail: "x" }, NAIVE_POLICY).verdict, "failed");
		assert.throws(() => assertProductionPolicy(NAIVE_POLICY), /refused/);
		assert.doesNotThrow(() => assertProductionPolicy(DEFAULT_POLICY));
		assert.throws(() => assertProductionPolicy({ ...DEFAULT_POLICY, preEffectCodes: new Set(["command_result_uncertain"]) }), /refused/);
	});
});
