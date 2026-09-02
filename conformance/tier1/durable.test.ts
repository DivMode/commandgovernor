/**
 * DUR — the durable-write helper does exactly the sequence the durability
 * contract relies on, and fails loudly when any step fails.
 *
 * Power loss cannot be simulated here, so the test does the next best thing:
 * it records every filesystem call through an instrumented `DurableFs` and
 * asserts the ORDER -- open temp, write, fsync temp, close, rename (or link),
 * open parent, fsync parent, close -- because that order is the whole
 * argument. Then it checks the real filesystem behaviour that the helpers
 * depend on: `link` refuses an existing name, a failed step leaves no
 * half-written record under the final name, and the loser of an exclusive
 * create reads the winner's complete contents.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

import { createFileExclusiveDurable, type DurableFs, fsyncDirectory, NODE_FS, unlinkDurable, writeFileDurable } from "../../governor/fs/durable.ts";

type Call = { readonly op: string; readonly path?: string; readonly fd?: number; readonly to?: string };

/** Wrap the real fs, recording each call in order; optionally fail a named step. */
function recorder(failAt?: { op: string; path?: RegExp }): { fs: DurableFs; calls: Call[] } {
	const calls: Call[] = [];
	const fdPaths = new Map<number, string>();
	const maybeFail = (op: string, path?: string) => {
		if (failAt && failAt.op === op && (!failAt.path || (path !== undefined && failAt.path.test(path)))) {
			throw Object.assign(new Error(`injected ${op} failure`), { code: "EIO" });
		}
	};
	const fs: DurableFs = {
		openSync: ((path: string, flags: string, mode?: number) => {
			maybeFail("open", path);
			const fd = NODE_FS.openSync(path, flags as never, mode);
			fdPaths.set(fd, path);
			calls.push({ op: "open", path, fd });
			return fd;
		}) as DurableFs["openSync"],
		writeSync: (fd, data) => {
			maybeFail("write", fdPaths.get(fd));
			calls.push({ op: "write", fd, path: fdPaths.get(fd) });
			return NODE_FS.writeSync(fd, data);
		},
		fsyncSync: (fd) => {
			maybeFail("fsync", fdPaths.get(fd));
			calls.push({ op: "fsync", fd, path: fdPaths.get(fd) });
			NODE_FS.fsyncSync(fd);
		},
		closeSync: (fd) => {
			calls.push({ op: "close", fd, path: fdPaths.get(fd) });
			NODE_FS.closeSync(fd);
		},
		renameSync: (from, to) => {
			maybeFail("rename", String(from));
			calls.push({ op: "rename", path: String(from), to: String(to) });
			NODE_FS.renameSync(from, to);
		},
		linkSync: (from, to) => {
			maybeFail("link", String(from));
			calls.push({ op: "link", path: String(from), to: String(to) });
			NODE_FS.linkSync(from, to);
		},
		unlinkSync: (path) => {
			calls.push({ op: "unlink", path: String(path) });
			NODE_FS.unlinkSync(path);
		},
		readFileSync: (path, encoding) => {
			calls.push({ op: "read", path });
			return NODE_FS.readFileSync(path, encoding);
		},
	};
	return { fs, calls };
}

const ops = (calls: Call[]) => calls.map((c) => c.op);

describe("DUR: writeFileDurable", () => {
	it("writes temp, fsyncs it, closes, renames, then fsyncs the parent directory -- in that order", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const { fs, calls } = recorder();
		writeFileDurable(target, "{\"a\":1}\n", { fs });
		assert.deepEqual(ops(calls), ["open", "write", "fsync", "close", "rename", "open", "fsync", "close"]);
		const [openTemp, , fsyncTemp, closeTemp, rename, openDir, fsyncDir, closeDir] = calls;
		assert.notEqual(openTemp!.path, target, "the data is written to a temp name, not the final one");
		assert.equal(dirname(openTemp!.path!), dir, "the temp lives in the same directory (same filesystem) as the record");
		assert.equal(fsyncTemp!.fd, openTemp!.fd, "the file fsync is on the temp fd");
		assert.equal(closeTemp!.fd, openTemp!.fd);
		assert.equal(rename!.path, openTemp!.path);
		assert.equal(rename!.to, target);
		assert.equal(openDir!.path, dir, "the parent directory is opened after the rename");
		assert.equal(fsyncDir!.fd, openDir!.fd, "and fsynced on its own fd");
		assert.equal(closeDir!.fd, openDir!.fd);
		assert.equal(readFileSync(target, "utf8"), "{\"a\":1}\n");
		assert.deepEqual(readdirSync(dir), ["record.json"], "no temp file is left behind");
	});

	it("replaces an existing record atomically and fsyncs the directory again", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		writeFileDurable(target, "one\n");
		const { fs, calls } = recorder();
		writeFileDurable(target, "two\n", { fs });
		assert.equal(readFileSync(target, "utf8"), "two\n");
		assert.equal(ops(calls).filter((op) => op === "fsync").length, 2, "file fsync and directory fsync");
		assert.deepEqual(readdirSync(dir), ["record.json"]);
	});

	it("a failed file fsync throws and leaves neither the record nor the temp", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const { fs } = recorder({ op: "fsync", path: /\.tmp$/ });
		assert.throws(() => writeFileDurable(target, "x", { fs }), /injected fsync failure/);
		assert.equal(existsSync(target), false, "no record under the final name");
		assert.deepEqual(readdirSync(dir), [], "the temp was removed");
	});

	it("a failed directory fsync is an error, not a silently non-durable success", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const { fs } = recorder({ op: "fsync", path: new RegExp(`${basename(dir).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) });
		assert.throws(() => writeFileDurable(target, "x", { fs }), /injected fsync failure/);
		// The rename already happened: the bytes are there, but the caller was told the write is not durable.
		assert.equal(readFileSync(target, "utf8"), "x");
	});

	it("real directory fsync works on this platform (macOS falls back from F_FULLFSYNC on a directory fd)", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		assert.doesNotThrow(() => fsyncDirectory(dir));
	});
});

describe("DUR: createFileExclusiveDurable", () => {
	it("publishes a complete, fsynced file by link, fsyncs the parent, and removes the temp", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "identity.json");
		const { fs, calls } = recorder();
		const result = createFileExclusiveDurable(target, "id\n", { fs });
		assert.equal(result.outcome, "created");
		assert.deepEqual(ops(calls), ["open", "write", "fsync", "close", "link", "open", "fsync", "close", "unlink"]);
		const link = calls[4]!;
		assert.equal(link.to, target);
		assert.notEqual(link.path, target);
		assert.equal(calls[5]!.path, dir, "the directory fsync follows the link");
		assert.equal(calls[8]!.path, link.path, "the temp is unlinked last");
		assert.equal(readFileSync(target, "utf8"), "id\n");
		assert.deepEqual(readdirSync(dir), ["identity.json"]);
	});

	it("never replaces an existing name: the loser reads the winner's contents and its own temp is gone", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "identity.json");
		writeFileSync(target, "winner\n");
		const { fs, calls } = recorder();
		const result = createFileExclusiveDurable(target, "loser\n", { fs });
		assert.equal(result.outcome, "exists");
		assert.equal(result.outcome === "exists" ? result.contents : undefined, "winner\n");
		assert.equal(readFileSync(target, "utf8"), "winner\n");
		assert.ok(!ops(calls).includes("rename"), "an exclusive create never renames over the name");
		assert.deepEqual(readdirSync(dir), ["identity.json"], "the loser's temp was removed");
	});

	it("a failure before the link leaves no file under the name", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "identity.json");
		const { fs } = recorder({ op: "fsync", path: /\.tmp$/ });
		assert.throws(() => createFileExclusiveDurable(target, "x", { fs }), /injected fsync failure/);
		assert.deepEqual(readdirSync(dir), []);
	});
});

describe("DUR: unlinkDurable", () => {
	it("unlinks then fsyncs the parent; a missing file is reported, not thrown", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "lease.lock");
		writeFileSync(target, "x");
		const { fs, calls } = recorder();
		assert.equal(unlinkDurable(target, fs), true);
		assert.deepEqual(ops(calls), ["unlink", "open", "fsync", "close"]);
		assert.equal(calls[1]!.path, dir);
		assert.equal(unlinkDurable(target, fs), false);
	});
});
