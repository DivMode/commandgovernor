/**
 * PIN (wire) — the daemon_hello guard refuses a daemon that is not the pin,
 * and refuses it before any command can be sent.
 *
 * Every other test talks to the correct pinned daemon, or to a fake that
 * builds its hello from the pin, so none of them could notice the guard
 * becoming a no-op. Here a fake daemon answers with each kind of wrong
 * hello, records every byte it receives, and the test asserts the typed
 * refusal, the closed socket, and an empty inbox.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:net";
import { join } from "node:path";

import { DaemonClient, SubstrateMismatch, connectWithRetry } from "../../governor/prime/daemon-client.ts";
import { expectedSubstrate } from "../../governor/prime/substrate.ts";

const root = mkdtempSync("/tmp/cg-pin-"); // short: sun_path
const socketPath = join(root, "fake.sock");
let server: Server;
let hello: Record<string, unknown>;
const received: string[] = [];

before(async () => {
	server = createServer((socket) => {
		socket.write(`${JSON.stringify(hello)}\n`);
		socket.on("data", (chunk: Buffer) => received.push(chunk.toString("utf8")));
	});
	await new Promise<void>((resolve) => server.listen(socketPath, resolve));
});

after(async () => {
	await new Promise<void>((resolve) => server.close(() => resolve()));
	rmSync(root, { recursive: true, force: true });
});

const expected = expectedSubstrate();
const good = () => ({ type: "daemon_hello", socketPath, protocol: { ...expected.protocol }, appVersion: expected.appVersion, schemaRevision: expected.schemaRevision });

describe("PIN (wire): an unpinned daemon never receives a command", () => {
	it("the matching hello is accepted (positive control)", async () => {
		hello = good();
		const client = new DaemonClient(socketPath, { clientId: "cg-pin-test", expected });
		const answered = await client.connect(2000);
		assert.equal(answered.appVersion, expected.appVersion);
		client.close();
	});

	for (const [label, mutate, pattern] of [
		["wrong protocol version", (h: Record<string, unknown>) => ({ ...h, protocol: { ...expected.protocol, version: 99 } }), /pin requires prime-agent\.daemon v7/],
		["wrong protocol name", (h: Record<string, unknown>) => ({ ...h, protocol: { name: "something-else", version: expected.protocol.version } }), /daemon speaks something-else/],
		["wrong appVersion", (h: Record<string, unknown>) => ({ ...h, appVersion: "9.9.9" }), /appVersion 9\.9\.9; pin requires/],
		["missing appVersion", (h: Record<string, unknown>) => ({ ...h, appVersion: undefined }), /appVersion undefined; pin requires/],
		["wrong schemaRevision", (h: Record<string, unknown>) => ({ ...h, schemaRevision: 999999 }), /schemaRevision 999999; pin requires/],
		["missing schemaRevision", (h: Record<string, unknown>) => ({ ...h, schemaRevision: undefined }), /schemaRevision undefined; pin requires/],
	] as const) {
		it(`${label}: SubstrateMismatch, socket closed, nothing sent`, async () => {
			hello = mutate(good());
			const before = received.length;
			const client = new DaemonClient(socketPath, { clientId: "cg-pin-test", expected });
			await assert.rejects(client.connect(2000), (e: unknown) => e instanceof SubstrateMismatch && pattern.test(e.message));
			await assert.rejects(client.request({ type: "list" }, "cg-pin-after-refusal"), /not connected|closed/);
			await new Promise((resolve) => setTimeout(resolve, 50));
			assert.equal(received.length, before, "the fake daemon received nothing");
			// connectWithRetry does not retry past a mismatch: the answer is definitive, not transient.
			await assert.rejects(connectWithRetry(socketPath, { clientId: "cg-pin-test", expected }, 5000), SubstrateMismatch);
			assert.equal(received.length, before);
		});
	}
});
