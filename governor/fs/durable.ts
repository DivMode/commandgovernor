/**
 * Durable file writes for authority-bearing Governor records.
 *
 * The Governor's contract is stronger than process-crash atomicity: durable
 * intent MUST exist before external I/O (the mutation ledger writes
 * DISPATCHED before the socket; the registry writes an incarnation before
 * the next dispatch can be fenced by it; the recovery lease exists before a
 * reopen is sent). "Durable" here means the record survives power loss, not
 * only the death of this process.
 *
 * A temp-file + `fsync(file)` + `rename` sequence gives crash atomicity of
 * the CONTENTS but not of the NAME: on Linux ext4/xfs/btrfs (and on APFS) the
 * rename is a directory operation, and a directory entry that has not been
 * fsynced can be lost on power failure even though the file's bytes were
 * flushed. The name is what a later Governor looks up, so losing it is losing
 * the record. Every writer below therefore ends with `fsync(parent directory)`.
 *
 * The guarantee relied on, by platform:
 *
 * - Linux: `fsync(2)` on the file flushes its data and metadata; `fsync(2)`
 *   on the parent directory fd flushes the directory entry created by
 *   `rename(2)`/`link(2)`/`unlink(2)`. This is the documented ext4/xfs
 *   requirement ("Do you need to fsync the directory? Yes.") and is the
 *   sequence SQLite and PostgreSQL use.
 * - macOS: Node's `fs.fsyncSync` reaches libuv `uv__fs_fsync`, which on Apple
 *   platforms issues `fcntl(F_FULLFSYNC)` (flush through the drive cache)
 *   and falls back to `fsync(2)` if that is refused, e.g. on a directory fd.
 *   Plain `fsync(2)` on macOS only pushes to the drive; `F_FULLFSYNC` is the
 *   documented way to ask for a durable write, and libuv does so for us.
 *
 * What is NOT claimed: a `rename` over an existing name is atomic for
 * readers (they see old or new, never a mix) on both platforms; the helper
 * does not add anything to that. Nothing here is safe over NFS.
 *
 * Every function is synchronous on purpose. The callers are on the path
 * between "decide" and "send", and an `await` there would be a place for the
 * send to happen first.
 */

import * as nodeFs from "node:fs";
import { dirname } from "node:path";

/** The subset of `node:fs` the helpers use, injectable so a test can record the exact sequence. */
export interface DurableFs {
	openSync: typeof nodeFs.openSync;
	writeSync: (fd: number, data: string) => number;
	fsyncSync: typeof nodeFs.fsyncSync;
	closeSync: typeof nodeFs.closeSync;
	renameSync: typeof nodeFs.renameSync;
	linkSync: typeof nodeFs.linkSync;
	unlinkSync: typeof nodeFs.unlinkSync;
	readFileSync: (path: string, encoding: "utf8") => string;
}

export const NODE_FS: DurableFs = {
	openSync: nodeFs.openSync,
	writeSync: (fd, data) => nodeFs.writeSync(fd, data),
	fsyncSync: nodeFs.fsyncSync,
	closeSync: nodeFs.closeSync,
	renameSync: nodeFs.renameSync,
	linkSync: nodeFs.linkSync,
	unlinkSync: nodeFs.unlinkSync,
	readFileSync: (path, encoding) => nodeFs.readFileSync(path, encoding),
};

let tempCounter = 0;

/** A temp name next to `path`, unique across processes and within one. */
export function tempPathFor(path: string): string {
	tempCounter += 1;
	return `${path}.${process.pid}.${Date.now()}.${tempCounter}.tmp`;
}

/** `fsync` the directory itself so the entries created/removed in it are durable. */
export function fsyncDirectory(dir: string, fs: DurableFs = NODE_FS): void {
	const fd = fs.openSync(dir, "r");
	try {
		fs.fsyncSync(fd);
	} finally {
		fs.closeSync(fd);
	}
}

function writeTemp(temp: string, contents: string, mode: number, fs: DurableFs): void {
	const fd = fs.openSync(temp, "w", mode);
	try {
		fs.writeSync(fd, contents);
		fs.fsyncSync(fd);
	} finally {
		fs.closeSync(fd);
	}
}

function removeQuietly(path: string, fs: DurableFs): void {
	try {
		fs.unlinkSync(path);
	} catch {
		// The temp file is either already gone or unreachable; the caller is
		// already failing, and a stale `.tmp` next to a record is harmless.
	}
}

/**
 * Replace (or create) `path` with `contents`, durably:
 * write temp -> fsync temp -> close -> rename over path -> fsync parent.
 *
 * Throws on any failure, including a failed directory fsync: a record whose
 * name is not known to be durable is not reported as written.
 */
export function writeFileDurable(path: string, contents: string, options: { mode?: number; fs?: DurableFs } = {}): void {
	const fs = options.fs ?? NODE_FS;
	const temp = tempPathFor(path);
	try {
		writeTemp(temp, contents, options.mode ?? 0o600, fs);
		fs.renameSync(temp, path);
	} catch (error) {
		removeQuietly(temp, fs);
		throw error;
	}
	fsyncDirectory(dirname(path), fs);
}

export type ExclusiveCreateOutcome = { readonly outcome: "created" } | { readonly outcome: "exists"; readonly contents: string };

/**
 * Create `path` with `contents` only if it does not exist, durably and
 * atomically with respect to concurrent creators and readers:
 *
 *   write temp -> fsync temp -> close -> link(temp, path) -> fsync parent -> unlink temp.
 *
 * `link(2)` fails with `EEXIST` when the name already exists and never
 * replaces it, so exactly one creator wins, and because the link publishes
 * an already-complete, already-fsynced file, no reader can ever observe an
 * empty or partial `path` (an `O_EXCL` create followed by a write would let
 * one). The loser reads the winner's contents and returns them.
 */
export function createFileExclusiveDurable(path: string, contents: string, options: { mode?: number; fs?: DurableFs } = {}): ExclusiveCreateOutcome {
	const fs = options.fs ?? NODE_FS;
	const temp = tempPathFor(path);
	try {
		writeTemp(temp, contents, options.mode ?? 0o600, fs);
		try {
			fs.linkSync(temp, path);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
			return { outcome: "exists", contents: fs.readFileSync(path, "utf8") };
		}
		fsyncDirectory(dirname(path), fs);
		return { outcome: "created" };
	} finally {
		removeQuietly(temp, fs);
	}
}

/** Remove `path` durably: unlink -> fsync parent. `ENOENT` is not an error. */
export function unlinkDurable(path: string, fs: DurableFs = NODE_FS): boolean {
	try {
		fs.unlinkSync(path);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
		throw error;
	}
	fsyncDirectory(dirname(path), fs);
	return true;
}
