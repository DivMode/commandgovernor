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

import { createFileExclusiveDurable, type DurableFs, fsyncDirectory, mkdirDurable, NODE_FS, ShortWrite, unlinkDurable, writeAllSync, writeFileDurable } from "../../governor/fs/durable.ts";

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
		writeSync: (fd, data, offset, length) => {
			maybeFail("write", fdPaths.get(fd));
			calls.push({ op: "write", fd, path: fdPaths.get(fd) });
			return NODE_FS.writeSync(fd, data, offset, length);
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
		mkdirSync: (path, mode) => {
			maybeFail("mkdir", path);
			calls.push({ op: "mkdir", path });
			NODE_FS.mkdirSync(path, mode);
		},
		existsSync: NODE_FS.existsSync,
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
		// The loser's own order: after the failed link it fsyncs the PARENT before it reads the winner,
		// because the winner may have died between its link and its own directory fsync.
		assert.deepEqual(ops(calls), ["open", "write", "fsync", "close", "link", "open", "fsync", "close", "read", "unlink"]);
		assert.equal(calls[5]!.path, dir, "the parent is fsynced by the loser");
		assert.equal(calls[8]!.path, target, "and only then read");
	});

	it("a name that vanishes between the link's EEXIST and the read is reported as vanished, not thrown", () => {
		// A concurrent lease release does exactly this; the caller retries rather than crashing.
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "lease.lock");
		writeFileSync(target, "holder\n");
		const { fs, calls } = recorder();
		const racing: DurableFs = {
			...fs,
			linkSync: (from, to) => {
				try {
					fs.linkSync(from, to);
				} finally {
					NODE_FS.unlinkSync(target); // the holder releases right after our link fails
				}
			},
		};
		const result = createFileExclusiveDurable(target, "mine\n", { fs: racing });
		assert.equal(result.outcome, "vanished");
		assert.ok(ops(calls).indexOf("fsync", ops(calls).indexOf("link")) > 0, "the loser still fsyncs the parent before discovering the name is gone");
		assert.deepEqual(readdirSync(dir), [], "nothing under the name, no temp left");
		assert.equal(createFileExclusiveDurable(target, "mine\n", { fs }).outcome, "created", "the retry succeeds");
	});

	it("a failure before the link leaves no file under the name", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "identity.json");
		const { fs } = recorder({ op: "fsync", path: /\.tmp$/ });
		assert.throws(() => createFileExclusiveDurable(target, "x", { fs }), /injected fsync failure/);
		assert.deepEqual(readdirSync(dir), []);
	});
});

describe("DUR: short writes", () => {
	/** A kernel that accepts at most `chunk` bytes per write, or nothing at all after `stallAfter` bytes. */
	function shortWriter(chunk: number, stallAfter = Number.POSITIVE_INFINITY): { fs: DurableFs; writes: number[] } {
		const writes: number[] = [];
		let total = 0;
		const fs: DurableFs = {
			...NODE_FS,
			writeSync: (fd, data, offset, length) => {
				if (total >= stallAfter) {
					writes.push(0);
					return 0;
				}
				const n = Math.min(chunk, length);
				const accepted = NODE_FS.writeSync(fd, data, offset, n);
				writes.push(accepted);
				total += accepted;
				return accepted;
			},
		};
		return { fs, writes };
	}

	it("writeAllSync keeps writing until every byte is accepted", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const contents = JSON.stringify({ padding: "x".repeat(1000) });
		const { fs, writes } = shortWriter(7);
		writeFileDurable(target, contents, { fs });
		assert.equal(readFileSync(target, "utf8"), contents, "the record is complete despite the kernel accepting 7 bytes at a time");
		assert.ok(writes.length >= Math.ceil(contents.length / 7), `many short writes were needed (${writes.length})`);
		assert.equal(writes.reduce((a, b) => a + b, 0), Buffer.byteLength(contents), "exactly the byte length was written, no more");
	});

	it("a write that makes no progress throws ShortWrite and nothing is published under the name", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const contents = "0123456789".repeat(20);
		const { fs } = shortWriter(16, 48);
		assert.throws(() => writeFileDurable(target, contents, { fs }), (e: unknown) => e instanceof ShortWrite && e.written === 48 && e.total === 200);
		assert.equal(existsSync(target), false, "a truncated record was never renamed into place");
		assert.deepEqual(readdirSync(dir), [], "and the truncated temp was removed");
		assert.throws(() => createFileExclusiveDurable(target, contents, { fs }), ShortWrite);
		assert.deepEqual(readdirSync(dir), []);
	});

	it("the byte count is measured in bytes, not characters", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const contents = "héllo wörld ✓"; // multi-byte
		const { fs, writes } = shortWriter(3);
		writeFileDurable(target, contents, { fs });
		assert.equal(readFileSync(target, "utf8"), contents);
		assert.equal(writes.reduce((a, b) => a + b, 0), Buffer.byteLength(contents, "utf8"));
		// The negative control: the pre-review helper issued one write and ignored its return.
		const fd = NODE_FS.openSync(join(dir, "naive"), "w", 0o600);
		const accepted = fs.writeSync(fd, Buffer.from(contents, "utf8"), 0, Buffer.byteLength(contents, "utf8"));
		NODE_FS.closeSync(fd);
		assert.ok(accepted < Buffer.byteLength(contents, "utf8"), "one write under this kernel would have truncated the record");
		assert.notEqual(readFileSync(join(dir, "naive"), "utf8"), contents);
	});

	it("a write that reports MORE bytes than were offered is refused, not trusted", () => {
		// write(2) never does this; the helper exists for kernels that do not behave as assumed.
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(dir, "record.json");
		const fs: DurableFs = {
			...NODE_FS,
			writeSync: (fd, data, offset, length) => {
				NODE_FS.writeSync(fd, data, offset, Math.min(3, length));
				return 1000;
			},
		};
		assert.throws(() => writeFileDurable(target, "0123456789", { fs }), (e: unknown) => e instanceof ShortWrite && e.written === 0);
		assert.equal(existsSync(target), false, "the 3-byte record was never published");
		assert.deepEqual(readdirSync(dir), []);
	});

	it("writeAllSync writes a zero-length buffer without calling write", () => {
		const dir = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const { fs, writes } = shortWriter(1);
		const fd = NODE_FS.openSync(join(dir, "empty"), "w", 0o600);
		writeAllSync(fd, Buffer.alloc(0), fs);
		NODE_FS.closeSync(fd);
		assert.deepEqual(writes, []);
	});
});

describe("DUR: mkdirDurable", () => {
	it("creates each missing directory and fsyncs its parent, top-down; existing directories are untouched", () => {
		const root = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(root, "state", "mutations");
		const { fs, calls } = recorder();
		const created = mkdirDurable(target, { fs });
		assert.deepEqual(created, [join(root, "state"), target]);
		assert.deepEqual(ops(calls), ["mkdir", "open", "fsync", "close", "mkdir", "open", "fsync", "close"]);
		assert.equal(calls[0]!.path, join(root, "state"));
		assert.equal(calls[1]!.path, root, "the state dir's entry is made durable in ITS parent");
		assert.equal(calls[4]!.path, target);
		assert.equal(calls[5]!.path, join(root, "state"), "and mutations/ in the state dir");
		// Idempotent: no mkdir attempt, but the leaf's parent is fsynced once, because
		// a directory another process created is not known durable until this one confirms it.
		const again = recorder();
		assert.deepEqual(mkdirDurable(target, { fs: again.fs }), []);
		assert.deepEqual(ops(again.calls), ["open", "fsync", "close"]);
		assert.equal(again.calls[0]!.path, join(root, "state"), "the existing leaf's parent");
	});

	it("the loser of a concurrent mkdir still fsyncs the parent before returning", () => {
		const root = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(root, "state");
		const { fs, calls } = recorder();
		const racing: DurableFs = {
			...fs,
			mkdirSync: (path, mode) => {
				// Another Governor creates the directory between our existence check and our mkdir.
				NODE_FS.mkdirSync(path, mode);
				fs.mkdirSync(path, mode); // records the call, then throws EEXIST
			},
		};
		assert.deepEqual(mkdirDurable(target, { fs: racing }), [], "not reported as created by this caller");
		assert.deepEqual(ops(calls), ["mkdir", "open", "fsync", "close"], "but the parent was fsynced anyway");
		assert.equal(calls[1]!.path, root);
	});

	it("a failed parent fsync is an error: the directory exists but was not reported durable", () => {
		const root = mkdtempSync(join(tmpdir(), "cg-durable-"));
		const target = join(root, "state");
		const { fs } = recorder({ op: "fsync", path: new RegExp(`${basename(root).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) });
		assert.throws(() => mkdirDurable(target, { fs }), /injected fsync failure/);
		assert.equal(existsSync(target), true);
	});

	it("a component that exists as a file is an error, and nothing is removed", () => {
		const root = mkdtempSync(join(tmpdir(), "cg-durable-"));
		writeFileSync(join(root, "state"), "not a directory");
		assert.throws(() => mkdirDurable(join(root, "state", "mutations")));
		assert.equal(readFileSync(join(root, "state"), "utf8"), "not a directory");
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
