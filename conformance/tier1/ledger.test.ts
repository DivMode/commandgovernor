/**
 * D2 (pure) — the mutation ledger's legal transitions, over a temp dir.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { MutationLedger, MutationLedgerError } from "../../governor/mutation/ledger.ts";
import type { DaemonResponse } from "../../governor/prime/protocol.ts";

const ok: DaemonResponse = { type: "response", command: "x", success: true };
const bad: DaemonResponse = { type: "response", command: "x", success: false, error: "Daemon worker socket closed" };
const identity = (commandId: string) => ({ commandId, clientId: "cg:test", commandType: "execute_bash_and_wait", sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });

describe("MutationLedger (D2)", () => {
	it("records intent as DISPATCHED and refuses to reuse a command id", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		const record = ledger.recordDispatch(identity("cg-1"));
		assert.equal(record.state, "DISPATCHED");
		assert.throws(() => ledger.recordDispatch(identity("cg-1")), (e: unknown) => e instanceof MutationLedgerError && e.code === "duplicate_command_id");
	});

	it("UNCERTAIN leaves only through evidence, and never back to DISPATCHED", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		ledger.recordDispatch(identity("cg-2"));
		const uncertain = ledger.markUncertain("cg-2", "untyped_failure", bad);
		assert.equal(uncertain.state, "UNCERTAIN");
		assert.throws(() => ledger.markCompleted("cg-2", ok), /not a legal transition/);
		assert.throws(() => ledger.markFailed("cg-2", { kind: "typed_pre_effect_rejection", commandType: "create", code: "session_already_active" }, bad), /not a legal transition/);
		assert.throws(() => ledger.markUncertain("cg-2", "timeout"), /not a legal transition/);
		const resolved = ledger.resolveUncertain("cg-2", { kind: "effect_observed", by: "t", detail: "d", observedAt: "now" });
		assert.equal(resolved.state, "COMPLETED");
		assert.throws(() => ledger.resolveUncertain("cg-2", { kind: "effect_absent_proven", by: "t", detail: "d", observedAt: "now" }), /not a legal transition/);
		assert.equal(ledger.require("cg-2").transitions.map((t) => t.to).join(">"), "DISPATCHED>UNCERTAIN>COMPLETED");
	});

	it("effect_absent_proven resolves to FAILED", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		ledger.recordDispatch(identity("cg-3"));
		ledger.markUncertain("cg-3", "transport_lost");
		assert.equal(ledger.resolveUncertain("cg-3", { kind: "effect_absent_proven", by: "t", detail: "d", observedAt: "now" }).state, "FAILED");
	});

	it("a superseding command must name an UNCERTAIN record", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		ledger.recordDispatch(identity("cg-4"));
		ledger.markCompleted("cg-4", ok);
		assert.throws(() => ledger.recordDispatch({ ...identity("cg-5"), supersedes: "cg-4" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "supersedes_not_uncertain");
		assert.throws(() => ledger.recordDispatch({ ...identity("cg-5"), supersedes: "cg-404" }), (e: unknown) => e instanceof MutationLedgerError && e.code === "unknown_command");
		ledger.recordDispatch(identity("cg-6"));
		ledger.markUncertain("cg-6", "timeout");
		assert.equal(ledger.recordDispatch({ ...identity("cg-7"), supersedes: "cg-6" }).supersedes, "cg-6");
	});

	it("awaitingReconciliation lists exactly the UNCERTAIN records", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		ledger.recordDispatch(identity("cg-9"));
		ledger.markUncertain("cg-9", "transport_lost");
		ledger.recordDispatch(identity("cg-10"));
		ledger.markCompleted("cg-10", ok);
		ledger.recordDispatch(identity("cg-11"));
		assert.deepEqual(ledger.awaitingReconciliation().map((r) => r.commandId), ["cg-9"]);
	});

	it("survives a re-open: a second ledger over the same dir reads the same states", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-ledger-"));
		const first = new MutationLedger(dir);
		first.recordDispatch(identity("cg-8"));
		first.markUncertain("cg-8", "untyped_failure", bad);
		const second = new MutationLedger(dir);
		assert.equal(second.require("cg-8").state, "UNCERTAIN");
		assert.equal(second.list().length, 1);
	});
});
