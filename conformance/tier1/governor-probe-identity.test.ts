/**
 * IDENT (Governor) — probing an UNCERTAIN command can never send the old
 * commandId under a new Prime journal identity.
 *
 * The MutationRecord's `clientId` is the authority. Before any socket I/O the
 * Governor re-reads the identity file, compares it and its own id and the
 * live connection's id with the record, and refuses on any disagreement.
 * The daemon here is a fake: a Unix socket that answers the pin's hello and
 * records every byte it receives. "Nothing was sent" is then a fact about
 * the wire, not a claim.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:net";
import { join } from "node:path";

import { ClientIdentityMismatch, Governor } from "../../governor/governor.ts";
import { clientIdentityPath, loadOrCreateClientIdentity, readClientIdentity } from "../../governor/prime/client-identity.ts";
import { expectedSubstrate } from "../../governor/prime/substrate.ts";

const root = mkdtempSync("/tmp/cg-probe-"); // short: sun_path
const socketPath = join(root, "fake.sock");
const received: string[] = [];
let server: Server;

before(async () => {
	const expected = expectedSubstrate();
	server = createServer((socket) => {
		socket.write(`${JSON.stringify({ type: "daemon_hello", socketPath, protocol: expected.protocol, appVersion: expected.appVersion, schemaRevision: expected.schemaRevision })}\n`);
		socket.on("data", (chunk: Buffer) => received.push(chunk.toString("utf8")));
	});
	await new Promise<void>((resolve) => server.listen(socketPath, resolve));
});

after(async () => {
	await new Promise<void>((resolve) => server.close(() => resolve()));
	rmSync(root, { recursive: true, force: true });
});

function governorOver(stateDir: string): Governor {
	mkdirSync(stateDir, { recursive: true });
	return new Governor({
		stateDir,
		socketPath,
		agentDir: join(root, "agent"),
		home: join(root, "home"),
		tmpDir: root,
		sessionDir: join(root, "sessions"),
		cwd: root,
		provider: "mock",
		model: "mock-1",
		sourceEnv: { PATH: process.env.PATH },
	});
}

/** An UNCERTAIN record under `clientId`, written through the ledger so it is a real record, not a hand-made file. */
function uncertainRecord(governor: Governor, clientId: string, commandId: string) {
	governor.ledger.recordDispatch({ commandId, clientId, commandType: "execute_bash_and_wait", sessionId: "s", activeSessionId: "a", incarnationIndex: 0 });
	return governor.ledger.markUncertain(commandId, "transport_lost", undefined, "fabricated for the fence test");
}

const command = { type: "execute_bash_and_wait", activeSessionId: "a", command: "true" };

describe("IDENT: probeStoredResult fails closed on any client-identity doubt, before socket I/O", () => {
	it("a record whose clientId is not this Governor's is refused, even with a live connection", async () => {
		const governor = governorOver(join(root, "g1"));
		await governor.connect(5000);
		const foreign = "cg:00000000-0000-4000-8000-000000000001";
		assert.notEqual(foreign, governor.clientId);
		uncertainRecord(governor, foreign, "cg-foreign");
		const before = received.length;
		await assert.rejects(governor.probeStoredResult("cg-foreign", command), (e: unknown) => e instanceof ClientIdentityMismatch && e.recorded === foreign && e.current === governor.clientId);
		assert.equal(received.length, before, "nothing reached the socket");
		assert.equal(governor.ledger.require("cg-foreign").probes, undefined, "no probe was recorded either");
		governor.close();
	});

	it("a missing identity file is refused (the Governor will not trust its in-memory id alone)", async () => {
		const governor = governorOver(join(root, "g2"));
		await governor.connect(5000);
		uncertainRecord(governor, governor.clientId, "cg-missing");
		rmSync(clientIdentityPath(governor.stateDir));
		const before = received.length;
		await assert.rejects(governor.probeStoredResult("cg-missing", command), (e: unknown) => e instanceof ClientIdentityMismatch && e.reason === "identity_file_unavailable" && e.identityCode === "identity_missing");
		assert.equal(received.length, before);
		governor.close();
	});

	it("a corrupted or replaced identity file is refused", async () => {
		const governor = governorOver(join(root, "g3"));
		await governor.connect(5000);
		uncertainRecord(governor, governor.clientId, "cg-corrupt");
		const path = clientIdentityPath(governor.stateDir);
		writeFileSync(path, "{broken");
		let before = received.length;
		await assert.rejects(governor.probeStoredResult("cg-corrupt", command), (e: unknown) => e instanceof ClientIdentityMismatch && e.reason === "identity_file_unavailable" && e.identityCode === "identity_malformed");
		assert.equal(received.length, before);
		// Replaced: a valid identity file that names a different Governor. This is what "someone re-initialised the state dir" looks like.
		rmSync(path);
		const other = loadOrCreateClientIdentity(governor.stateDir).record.clientId;
		assert.notEqual(other, governor.clientId);
		before = received.length;
		await assert.rejects(governor.probeStoredResult("cg-corrupt", command), (e: unknown) => e instanceof ClientIdentityMismatch && e.current === other && e.reason === "identity_file_differs");
		assert.equal(received.length, before);
		governor.close();
	});

	it("a Governor that restarted over a re-initialised state dir cannot probe the old records", async () => {
		const stateDir = join(root, "g4");
		const first = governorOver(stateDir);
		uncertainRecord(first, first.clientId, "cg-old");
		first.close();
		// The identity is lost (deleted, restored from a different backup, ...) and a new Governor starts.
		rmSync(clientIdentityPath(stateDir));
		const second = governorOver(stateDir);
		await second.connect(5000);
		assert.notEqual(second.clientId, first.clientId, "the new Governor has a new journal identity");
		assert.equal(readClientIdentity(stateDir).clientId, second.clientId);
		const before = received.length;
		await assert.rejects(second.probeStoredResult("cg-old", command), (e: unknown) => e instanceof ClientIdentityMismatch && e.recorded === first.clientId && e.current === second.clientId);
		assert.equal(received.length, before, "the old commandId was not sent under the new clientId");
		assert.equal(second.ledger.require("cg-old").state, "UNCERTAIN", "the record stays where it was, for a human");
		second.close();
	});

	it("a probe with a different command type than the record is refused", async () => {
		const governor = governorOver(join(root, "g5"));
		await governor.connect(5000);
		uncertainRecord(governor, governor.clientId, "cg-type");
		const before = received.length;
		await assert.rejects(governor.probeStoredResult("cg-type", { type: "prompt", message: "x" }), /a probe re-presents the same command/);
		assert.equal(received.length, before);
		governor.close();
	});

	it("positive control: with the identity intact the probe does reach the socket under the recorded clientId", async () => {
		const governor = governorOver(join(root, "g6"));
		await governor.connect(5000);
		uncertainRecord(governor, governor.clientId, "cg-ok");
		const before = received.length;
		// The fake daemon never answers a command; a short timeout ends the probe as UNCERTAIN (timeout).
		const probe = await governor.probeStoredResult("cg-ok", command, 300);
		assert.equal(probe.verdict.verdict, "uncertain");
		assert.equal(probe.verdict.verdict === "uncertain" ? probe.verdict.reason : undefined, "timeout");
		const sent = received.slice(before).join("");
		assert.ok(sent.length > 0, "the envelope was written");
		const envelope = JSON.parse(sent.trim().split("\n")[0]!) as { id: string; clientId: string };
		assert.equal(envelope.id, "cg-ok");
		assert.equal(envelope.clientId, governor.clientId);
		governor.close();
	});
});
