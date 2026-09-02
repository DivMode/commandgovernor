/**
 * IDENT — the Governor's Prime journal identity is one per state directory,
 * created once, never overwritten, and identical across restarts and races.
 *
 * Prime's idempotency key is `clientId + commandId`. A Governor that lost or
 * re-minted its clientId would present every UNCERTAIN command as new work
 * under a new journal identity. These tests prove the identity file cannot
 * drift: restart reads the exact same bytes; a simultaneous first start by
 * several processes converges on one id; a deterministic lost race (the
 * winner publishes between "check" and "commit") returns the winner; and a
 * missing, corrupt or malformed file is a typed error, never a new id.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { spawn } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { CLIENT_ID_PATTERN, ClientIdentityError, clientIdentityPath, loadOrCreateClientIdentity, readClientIdentity } from "../../governor/prime/client-identity.ts";
import { REPO_ROOT } from "../lib/repo.ts";

const fresh = () => mkdtempSync(join(tmpdir(), "cg-ident-"));

describe("IDENT: client identity per state directory", () => {
	it("a restart retains the exact identity, byte for byte", () => {
		const dir = fresh();
		const first = loadOrCreateClientIdentity(dir);
		assert.equal(first.created, true);
		assert.match(first.record.clientId, CLIENT_ID_PATTERN);
		assert.equal(typeof first.record.createdBy.pid, "number");
		const bytes = readFileSync(clientIdentityPath(dir));
		for (let restart = 0; restart < 5; restart += 1) {
			const again = loadOrCreateClientIdentity(dir);
			assert.equal(again.created, false);
			assert.equal(again.record.clientId, first.record.clientId);
			assert.deepEqual(readFileSync(clientIdentityPath(dir)), bytes, "the file was not rewritten");
		}
	});

	it("a lost race returns the winner's identity, and the loser's candidate never reaches disk", () => {
		const dir = fresh();
		let winner: string | undefined;
		const loser = loadOrCreateClientIdentity(dir, {
			beforeCommit: () => {
				// Another Governor completes first initialisation between our "missing" check and our commit.
				winner = loadOrCreateClientIdentity(dir).record.clientId;
			},
		});
		assert.ok(winner);
		assert.equal(loser.created, false, "the loser reports that it did not create the identity");
		assert.equal(loser.record.clientId, winner, "the loser adopts the winner");
		assert.equal(readClientIdentity(dir).clientId, winner);
	});

	it("N processes performing first initialisation simultaneously converge on one clientId", async () => {
		const dir = fresh();
		const goFile = join(dir, "go");
		const child = join(REPO_ROOT, "conformance", "lib", "client-identity-race-child.ts");
		const contenders = 8;
		const outputs = await new Promise<string[]>((resolve, reject) => {
			const results: string[] = [];
			let running = contenders;
			const procs = Array.from({ length: contenders }, () => spawn(process.execPath, [child, dir, goFile], { stdio: ["ignore", "pipe", "inherit"] }));
			for (const proc of procs) {
				let out = "";
				proc.stdout.on("data", (chunk: Buffer) => (out += chunk.toString("utf8")));
				proc.once("exit", (code) => {
					if (code !== 0) reject(new Error(`race child exited ${String(code)}`));
					results.push(out.trim());
					running -= 1;
					if (running === 0) resolve(results);
				});
			}
			// Release all contenders at once, after they have all started spinning.
			setTimeout(() => writeFileSync(goFile, "go"), 300);
		});
		const parsed = outputs.map((line) => JSON.parse(line) as { clientId: string; created: boolean });
		const ids = new Set(parsed.map((p) => p.clientId));
		assert.equal(ids.size, 1, `every contender got the same id: ${JSON.stringify(parsed)}`);
		assert.equal(parsed.filter((p) => p.created).length, 1, `exactly one contender created it: ${JSON.stringify(parsed)}`);
		assert.equal(readClientIdentity(dir).clientId, [...ids][0]);
	});

	it("a missing, unreadable, corrupt or malformed file is a typed error, not a new identity", () => {
		const dir = fresh();
		assert.throws(() => readClientIdentity(dir), (e: unknown) => e instanceof ClientIdentityError && e.code === "identity_missing");
		const created = loadOrCreateClientIdentity(dir).record.clientId;
		const path = clientIdentityPath(dir);

		writeFileSync(path, "not json");
		assert.throws(() => loadOrCreateClientIdentity(dir), (e: unknown) => e instanceof ClientIdentityError && e.code === "identity_malformed");
		assert.equal(readFileSync(path, "utf8"), "not json", "nothing overwrote the corrupt file");

		writeFileSync(path, JSON.stringify({ schemaVersion: 1, clientId: "cg:not-a-uuid", createdAt: "x", createdBy: { pid: 1 } }));
		assert.throws(() => loadOrCreateClientIdentity(dir), (e: unknown) => e instanceof ClientIdentityError && e.code === "identity_malformed");

		writeFileSync(path, `${created}\n`); // the pre-review bare-string format is not an identity either
		assert.throws(() => loadOrCreateClientIdentity(dir), (e: unknown) => e instanceof ClientIdentityError && e.code === "identity_malformed");

		if (process.getuid?.() !== 0) {
			writeFileSync(path, JSON.stringify({ schemaVersion: 1, clientId: created, createdAt: "x", createdBy: { pid: 1 } }));
			chmodSync(path, 0o000);
			try {
				assert.throws(() => loadOrCreateClientIdentity(dir), (e: unknown) => e instanceof ClientIdentityError && e.code === "identity_unreadable");
			} finally {
				chmodSync(path, 0o600);
			}
		}
		rmSync(path);
		const reminted = loadOrCreateClientIdentity(dir);
		assert.equal(reminted.created, true, "only a genuinely absent file is created; and that is a NEW identity, which is why the probe fence re-reads the file");
		assert.notEqual(reminted.record.clientId, created);
	});
});
