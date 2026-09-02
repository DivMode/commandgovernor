/**
 * D2 (real crash) — a Governor process killed after it wrote DISPATCHED and
 * before it recorded a result leaves an obligation, and the next Governor
 * over the same state directory surfaces it.
 *
 * The child is a real Governor in a real child process, talking to a fake
 * daemon that never answers, so the DISPATCHED record is written by the
 * production path and the crash is a SIGKILL, not a simulated one. While
 * the child is alive, a second Governor in this process must NOT adopt the
 * record (live-owner fencing); once the child is dead, it must. The adopted
 * record is then probed under the exact original command (the socket sees
 * it) and refused under a different body (the socket does not).
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:net";
import { join } from "node:path";

import { CommandMismatch, Governor } from "../../governor/governor.ts";
import { expectedSubstrate } from "../../governor/prime/substrate.ts";
import { isProcessAlive } from "../../governor/prime/substrate.ts";
import { REPO_ROOT } from "../lib/repo.ts";
import { waitUntil } from "../lib/prime-fixture.ts";

const root = mkdtempSync("/tmp/cg-crash-"); // short: sun_path
const socketPath = join(root, "fake.sock");
const stateDir = join(root, "state");
const received: string[] = [];
let server: Server;
let child: ChildProcess | undefined;

before(async () => {
	const expected = expectedSubstrate();
	server = createServer((socket) => {
		socket.write(`${JSON.stringify({ type: "daemon_hello", socketPath, protocol: expected.protocol, appVersion: expected.appVersion, schemaRevision: expected.schemaRevision })}\n`);
		socket.on("data", (chunk: Buffer) => received.push(chunk.toString("utf8")));
	});
	await new Promise<void>((resolve) => server.listen(socketPath, resolve));
	mkdirSync(join(root, "sessions"), { recursive: true });
});

after(async () => {
	if (child && child.exitCode === null) child.kill("SIGKILL");
	await new Promise<void>((resolve) => server.close(() => resolve()));
	rmSync(root, { recursive: true, force: true });
});

function governorHere(): Governor {
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

describe("D2: a Governor killed inside its crash window", () => {
	it("leaves DISPATCHED; a live owner is fenced; a dead owner's record is adopted and probed under the exact command only", async () => {
		// Spawn the child Governor and wait for its DISPATCHED record.
		child = spawn(process.execPath, [join(REPO_ROOT, "conformance", "lib", "governor-crash-child.ts"), stateDir, socketPath, root], { stdio: ["ignore", "pipe", "inherit"] });
		let out = "";
		child.stdout!.on("data", (chunk: Buffer) => (out += chunk.toString("utf8")));
		const announced = await waitUntil(() => {
			const line = out.split("\n").find((candidate) => candidate.startsWith("{"));
			return line ? (JSON.parse(line) as { pid: number; clientId: string; ownerToken: string }) : undefined;
		}, 15_000, 50);
		const mutations = join(stateDir, "mutations");
		const files = await waitUntil(() => {
			const names = readdirSync(mutations, { withFileTypes: true })
				.filter((entry) => entry.isDirectory() && readdirSync(join(mutations, entry.name)).includes("v1.json"))
				.map((entry) => entry.name);
			return names.length > 0 ? names : undefined;
		}, 5_000, 25);
		assert.equal(files.length, 1);
		const commandId = files[0]!;
		const envelopesBefore = received.join("").split("\n").filter((line) => line.includes(commandId)).length;
		assert.equal(envelopesBefore, 1, "the child's envelope did reach the socket (the crash window here is 'sent, no result')");

		// Live-owner fencing: a second Governor over the same state dir, while the child is alive.
		const bystander = governorHere();
		assert.equal(bystander.clientId, announced.clientId, "same state dir, same journal identity");
		assert.deepEqual(bystander.startupAdoption.adopted, [], "nothing was adopted while the owner lives");
		assert.deepEqual(bystander.startupAdoption.inFlight.map((r) => r.commandId), [commandId]);
		assert.equal(bystander.ledger.require(commandId).state, "DISPATCHED");
		assert.equal(bystander.ledger.require(commandId).dispatchedBy.pid, announced.pid);
		assert.deepEqual(bystander.awaitingReconciliation(), [], "and it is not an obligation yet");
		assert.equal(bystander.ledger.require(commandId).state, "DISPATCHED", "listing did not adopt it either");

		// The crash.
		const exited = new Promise<void>((resolve) => child!.once("exit", () => resolve()));
		child.kill("SIGKILL");
		await exited;
		await waitUntil(() => !isProcessAlive(announced.pid), 10_000, 25);

		// The obligation surfaces: on the existing instance's attention surface ...
		const surfaced = bystander.awaitingReconciliation();
		assert.deepEqual(surfaced.map((r) => r.commandId), [commandId]);
		const adopted = bystander.ledger.require(commandId);
		assert.equal(adopted.state, "UNCERTAIN");
		const last = adopted.transitions[adopted.transitions.length - 1]!;
		assert.equal(last.uncertainReason, "dispatcher_lost");
		assert.equal(last.adoption?.dispatcher.pid, announced.pid);
		assert.equal(last.adoption?.dispatcher.ownerToken, announced.ownerToken);
		assert.ok(last.adoption?.verdict === "gone" || last.adoption?.verdict === "replaced", String(last.adoption?.verdict));
		// ... and at construction of a fresh Governor (a restart), which finds it already adopted.
		const restarted = governorHere();
		assert.deepEqual(restarted.startupAdoption.adopted, []);
		assert.deepEqual(restarted.awaitingReconciliation().map((r) => r.commandId), [commandId]);

		// Probing: the record holds the complete command, so no caller has to remember it across the restart.
		await restarted.connect(5000);
		let before = received.length;
		await assert.rejects(
			restarted.probeStoredResult(commandId, { type: "execute_bash_and_wait", activeSessionId: "active-0", command: "echo something-else" }),
			(e: unknown) => e instanceof CommandMismatch && e.reason === "digest_differs",
		);
		assert.equal(received.length, before, "a different body under the old id never reached the socket");
		before = received.length;
		const probe = await restarted.probeStoredResult(commandId, undefined, 300);
		assert.equal(probe.verdict.verdict, "uncertain", "the fake daemon never answers; the probe times out UNCERTAIN");
		const sent = received.slice(before).join("");
		const envelope = JSON.parse(sent.trim().split("\n")[0]!) as { id: string; clientId: string; command: Record<string, unknown> };
		assert.equal(envelope.id, commandId, "the ORIGINAL command id was re-presented");
		assert.equal(envelope.clientId, announced.clientId, "under the ORIGINAL journal identity");
		assert.deepEqual(envelope.command, { type: "execute_bash_and_wait", command: "echo effect >> /nowhere", activeSessionId: "active-0" }, "with the exact original command");
		bystander.close();
		restarted.close();
	});
});
