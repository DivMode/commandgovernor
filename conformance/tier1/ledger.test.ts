/**
 * D2 (pure) — the mutation ledger's legal transitions, over a temp dir.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { COMMAND_DIGEST_PATTERN, commandDigest } from "../../governor/mutation/digest.ts";
import { MutationLedger, MutationLedgerError } from "../../governor/mutation/ledger.ts";
import type { DaemonResponse } from "../../governor/prime/protocol.ts";

const ok: DaemonResponse = { type: "response", command: "x", success: true };
const bad: DaemonResponse = { type: "response", command: "x", success: false, error: "Daemon worker socket closed" };
const identity = (commandId: string) => ({ commandId, clientId: "cg:test", command: { type: "execute_bash_and_wait", activeSessionId: "a", command: "true" }, sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });

describe("MutationLedger (D2)", () => {
	it("records intent as DISPATCHED and refuses to reuse a command id", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		const record = ledger.recordDispatch(identity("cg-1"));
		assert.equal(record.state, "DISPATCHED");
		assert.equal(record.commandType, "execute_bash_and_wait", "the type is taken from the command, not declared separately");
		assert.match(record.commandDigest, COMMAND_DIGEST_PATTERN);
		assert.deepEqual(record.command, identity("cg-1").command, "the complete command is stored when it carries no environment");
		assert.deepEqual(record.withheld, []);
		assert.equal(typeof record.dispatchedBy.pid, "number");
		assert.equal(typeof record.dispatchedBy.ownerToken, "string");
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
		assert.equal(ledger.require("cg-2").version, 3, "one immutable version per write");
		assert.deepEqual(ledger.history("cg-2").map((v) => [v.version, v.state]), [[1, "DISPATCHED"], [2, "UNCERTAIN"], [3, "COMPLETED"]], "the history is every version, kept");
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

	it("withholds environment-bearing fields from the stored command but digests the complete one", () => {
		const ledger = new MutationLedger(mkdtempSync(join(tmpdir(), "cg-ledger-")));
		const command = { type: "create", sessionPath: "/s/root.jsonl", launchEnv: { HOME: "/h", SECRET_TOKEN: "hunter2" }, config: { cwd: "/w" } };
		const record = ledger.recordDispatch({ ...identity("cg-env"), command });
		assert.deepEqual(record.withheld, ["launchEnv"]);
		assert.equal("launchEnv" in record.command, false, "no environment value reaches the ledger");
		assert.deepEqual(record.command, { type: "create", sessionPath: "/s/root.jsonl", config: { cwd: "/w" } });
		assert.equal(record.commandDigest, commandDigest(command), "the digest covers the withheld field too");
		const onDisk = JSON.stringify(ledger.require("cg-env"));
		assert.ok(!onDisk.includes("hunter2"), "the secret is not on disk");
		// At any depth, not only the top level: dispatchMutation takes an arbitrary command.
		const nested = { type: "prompt", message: "x", options: { deep: { env: { NESTED: "nested-secret" } }, list: [{ launchEnv: { A: "listed-secret" } }] } };
		const deep = ledger.recordDispatch({ ...identity("cg-nested"), command: nested });
		assert.deepEqual(deep.withheld, ["options.deep.env", "options.list[0].launchEnv"]);
		const deepOnDisk = JSON.stringify(ledger.require("cg-nested"));
		assert.ok(!deepOnDisk.includes("nested-secret") && !deepOnDisk.includes("listed-secret"), "no nested environment value reaches the ledger");
		assert.equal(deep.commandDigest, commandDigest(nested));
	});

	it("the default owner token is not derived from the pid alone", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-ledger-"));
		const a = new MutationLedger(dir, { self: { pid: 5000, processStartId: "old" } });
		const b = new MutationLedger(dir, { self: { pid: 5000, processStartId: "new" } });
		assert.notEqual(a.self.ownerToken, b.self.ownerToken, "a recycled pid must not inherit the dead dispatcher's token");
	});

	it("a stray entry under mutations/ is reported, and does not hide the records around it", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-ledger-"));
		const ledger = new MutationLedger(dir);
		ledger.recordDispatch(identity("cg-real"));
		ledger.markUncertain("cg-real", "timeout");
		mkdirSync(join(ledger.dir, "stray dir"));
		writeFileSync(join(ledger.dir, "notes.txt"), "x");
		assert.deepEqual(ledger.list().map((r) => r.commandId), ["cg-real"]);
		assert.deepEqual(ledger.awaitingReconciliation().map((r) => r.commandId), ["cg-real"], "the obligation is still visible");
		assert.deepEqual([...ledger.adoptAbandoned().strays].sort(), ["notes.txt", "stray dir"]);
	});

	it("refuses to start over records in the pre-version layout, and reports empty record directories", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-ledger-"));
		const ledger = new MutationLedger(dir);
		ledger.recordDispatch(identity("cg-real"));
		writeFileSync(join(ledger.dir, "cg-old.json"), "{}");
		assert.throws(() => new MutationLedger(dir), (e: unknown) => e instanceof MutationLedgerError && e.code === "unreadable_layout" && /cg-old\.json/.test(e.message));
		rmSync(join(ledger.dir, "cg-old.json"));
		mkdirSync(join(ledger.dir, "cg-half"));
		const again = new MutationLedger(dir);
		assert.deepEqual(again.list().map((r) => r.commandId), ["cg-real"]);
		assert.deepEqual(again.empty(), ["cg-half"]);
		assert.deepEqual(again.adoptAbandoned().empty, ["cg-half"]);
		assert.equal(again.recordDispatch({ ...identity("cg-half") }).version, 1, "a later create of that id heals it");
		assert.deepEqual(again.empty(), []);
	});

	it("a version file with a non-canonical name is not a version, and a gap in the history is a typed error", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-ledger-"));
		const ledger = new MutationLedger(dir);
		ledger.recordDispatch(identity("cg-v"));
		ledger.markUncertain("cg-v", "timeout");
		const recordDir = join(ledger.dir, "cg-v");
		writeFileSync(join(recordDir, "v03.json"), "{}");
		assert.equal(ledger.require("cg-v").version, 2, "v03.json is ignored");
		rmSync(join(recordDir, "v1.json"));
		assert.equal(ledger.require("cg-v").state, "UNCERTAIN", "the current version is still readable");
		assert.throws(() => ledger.history("cg-v"), (e: unknown) => e instanceof MutationLedgerError && e.code === "corrupt_history");
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
