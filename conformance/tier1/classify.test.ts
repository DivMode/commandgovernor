/**
 * D2 (pure) — the outcome classifier, exercised over fabricated responses.
 *
 * The runtime tier proves the invariant against the real daemon; this file
 * proves the decision procedure over every shape it can be handed, including
 * the shapes that would tempt a wording-based guard and the shape that
 * falsified the pre-review classifier: a typed code that is pre-effect for
 * one command and post-effect for another.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { assertProductionPolicy, classifyMutationOutcome, DEFAULT_POLICY, LEGACY_GLOBAL_CODE_POLICY, NAIVE_POLICY } from "../../governor/mutation/classify.ts";
import { ProofMatrix, REVIEWED_PROOFS } from "../../governor/mutation/proof.ts";
import type { DaemonErrorInfo, DaemonResponse } from "../../governor/prime/protocol.ts";

const failure = (commandType: string, error: string, errorInfo?: DaemonErrorInfo): DaemonResponse =>
	({ type: "response", command: commandType, success: false, error, ...(errorInfo ? { errorInfo } : {}) }) as DaemonResponse;

const missingCwd: DaemonErrorInfo = { code: "missing_session_cwd", issue: { sessionCwd: "/nowhere", fallbackCwd: "/work" } };
const importNotFound: DaemonErrorInfo = { code: "session_import_file_not_found", filePath: "/f.jsonl" };
const alreadyActive: DaemonErrorInfo = { code: "session_already_active", sessionPath: "/x.jsonl", activeSessionId: "a" };

describe("classifyMutationOutcome (D2)", () => {
	it("success is completed", () => {
		const verdict = classifyMutationOutcome({ kind: "response", commandType: "x", response: { type: "response", command: "x", success: true, data: 1 } });
		assert.equal(verdict.verdict, "completed");
	});

	it("the bake-off's exact stored failure is uncertain, whatever it says", () => {
		for (const text of ["Daemon worker socket closed", "daemon worker socket closed", "Worker connection lost", "Session worker is not connected", "", "something entirely new"]) {
			const verdict = classifyMutationOutcome({ kind: "response", commandType: "execute_bash_and_wait", response: failure("execute_bash_and_wait", text) });
			assert.equal(verdict.verdict, "uncertain", text);
			assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "untyped_failure");
		}
	});

	it("the substrate's own uncertain code is uncertain, for any command", () => {
		for (const commandType of ["create", "import_jsonl", "execute_bash_and_wait", "never_heard_of_it"]) {
			const verdict = classifyMutationOutcome({ kind: "response", commandType, response: failure(commandType, "uncertain", { code: "command_result_uncertain", clientId: "c", commandId: "k" }) });
			assert.equal(verdict.verdict, "uncertain");
			assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "substrate_reported_uncertain");
		}
	});

	it("a reviewed pre-effect pair is the only failed verdict, and it carries the review as proof", () => {
		const verdict = classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response: failure("import_jsonl", "not found", importNotFound) });
		assert.equal(verdict.verdict, "failed");
		assert.ok(verdict.verdict === "failed");
		assert.equal(verdict.proof.kind, "typed_pre_effect_rejection");
		assert.equal(verdict.proof.commandType, "import_jsonl");
		assert.equal(verdict.proof.code, "session_import_file_not_found");
		assert.equal(verdict.proof.review?.timing, "pre_effect");
		assert.match(verdict.proof.review?.thrownAt ?? "", /importFromJsonl/);
	});

	it("a reviewed AMBIGUOUS pair is uncertain: create + session_already_active has a worker-side throw site after launchWorker", () => {
		const verdict = classifyMutationOutcome({ kind: "response", commandType: "create", response: failure("create", "already active", alreadyActive) });
		assert.equal(verdict.verdict, "uncertain");
		assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "typed_failure_ambiguous");
		assert.match(verdict.verdict === "uncertain" ? (verdict.detail ?? "") : "", /acquireSessionLease/);
	});

	it("the same code is proof for one command and not for another: import_jsonl + missing_session_cwd is post-effect", () => {
		const verdict = classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response: failure("import_jsonl", "cwd missing", missingCwd) });
		assert.equal(verdict.verdict, "uncertain");
		assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "typed_failure_post_effect");
		assert.match(verdict.verdict === "uncertain" ? (verdict.detail ?? "") : "", /copyFileSync/);
	});

	it("the classifier keys on the command the Governor SENT, not the command the response claims", () => {
		// A response that labels itself `create` but arrived for an `import_jsonl` dispatch is classified as import_jsonl.
		const mislabelled = failure("create", "cwd missing", missingCwd);
		const verdict = classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response: mislabelled });
		assert.equal(verdict.verdict, "uncertain");
		assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "typed_failure_post_effect");
	});

	it("an unreviewed (command, code) pair fails closed to uncertain, including every code for an unknown command", () => {
		for (const [commandType, info] of [
			["create", missingCwd], // reviewed nowhere: a worker-side throw after a lease was taken
			["create", importNotFound],
			["switch_session", alreadyActive],
			["execute_bash_and_wait", alreadyActive],
			["never_heard_of_it", alreadyActive],
			["never_heard_of_it", importNotFound],
			["", alreadyActive],
		] as const) {
			const verdict = classifyMutationOutcome({ kind: "response", commandType, response: failure(commandType, "typed", info) });
			assert.equal(verdict.verdict, "uncertain", `${commandType} + ${info.code}`);
			assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "typed_failure_unreviewed", `${commandType} + ${info.code}`);
		}
	});

	it("an errorInfo with an unknown code is not proof (a fabricated record must not pass as pre-effect)", () => {
		const forged = { type: "response", command: "create", success: false, error: "nope", errorInfo: { code: "worker_definitely_did_nothing" } } as unknown as DaemonResponse;
		const verdict = classifyMutationOutcome({ kind: "response", commandType: "create", response: forged });
		assert.equal(verdict.verdict, "uncertain");
		assert.equal(verdict.verdict === "uncertain" ? verdict.reason : undefined, "unknown_error_code");
	});

	it("transport loss and timeout are uncertain", () => {
		assert.equal(classifyMutationOutcome({ kind: "transport_lost", commandType: "x", detail: "socket closed" }).verdict, "uncertain");
		assert.equal(classifyMutationOutcome({ kind: "timeout", commandType: "x", timeoutMs: 5 }).verdict, "uncertain");
	});

	it("negative controls: the naive and the legacy global-code policies call the falsifying inputs failed, so the tests above can fail", () => {
		assert.equal(classifyMutationOutcome({ kind: "response", commandType: "execute_bash_and_wait", response: failure("execute_bash_and_wait", "Daemon worker socket closed") }, NAIVE_POLICY).verdict, "failed");
		assert.equal(classifyMutationOutcome({ kind: "transport_lost", commandType: "x", detail: "x" }, NAIVE_POLICY).verdict, "failed");
		// The pre-review classifier: any serialised code is proof for any command.
		const postEffect = failure("import_jsonl", "cwd missing", missingCwd);
		assert.equal(classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response: postEffect }, LEGACY_GLOBAL_CODE_POLICY).verdict, "failed");
		assert.equal(classifyMutationOutcome({ kind: "response", commandType: "import_jsonl", response: postEffect }, DEFAULT_POLICY).verdict, "uncertain");
		// ... but even that classifier never called untyped failure or transport loss failed.
		assert.equal(classifyMutationOutcome({ kind: "response", commandType: "x", response: failure("x", "Daemon worker socket closed") }, LEGACY_GLOBAL_CODE_POLICY).verdict, "uncertain");
		assert.throws(() => assertProductionPolicy(NAIVE_POLICY), /refused/);
		assert.throws(() => assertProductionPolicy(LEGACY_GLOBAL_CODE_POLICY), /regardless of command; refused/);
		assert.doesNotThrow(() => assertProductionPolicy(DEFAULT_POLICY));
	});

	it("assertProductionPolicy refuses a matrix that reviews the uncertain code, an unproducible code, or a wildcard command", () => {
		const base = REVIEWED_PROOFS[0]!;
		assert.throws(() => assertProductionPolicy({ ...DEFAULT_POLICY, proofMatrix: new ProofMatrix([{ ...base, code: "command_result_uncertain" }]) }), /refused/);
		assert.throws(() => assertProductionPolicy({ ...DEFAULT_POLICY, proofMatrix: new ProofMatrix([{ ...base, commandType: "*" }]) }), /wildcard/);
		assert.throws(() => new ProofMatrix([base, base]), /twice/);
	});
});
