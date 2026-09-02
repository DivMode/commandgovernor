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
 * Two more things the contract needs that a naive helper gets wrong:
 *
 * - `write(2)` may accept fewer bytes than asked. A helper that issues one
 *   `writeSync` and ignores its return value can fsync and publish a
 *   truncated record. `writeTemp` therefore writes a byte buffer in a loop
 *   until every byte has been accepted, and fails without publishing if the
 *   kernel makes no progress.
 * - `mkdir(2)` creates a directory entry in the PARENT, and that entry is no
 *   more durable than a `rename`'s until the parent is fsynced. Fsyncing
 *   `mutations/` after creating a record inside it makes the record's name
 *   durable in `mutations/`; it says nothing about whether `mutations/`
 *   itself survived in the state directory. `mkdirDurable` fsyncs the parent
 *   of every directory it creates.
 *
 * Every function is synchronous on purpose. The callers are on the path
 * between "decide" and "send", and an `await` there would be a place for the
 * send to happen first.
 */

import * as nodeFs from "node:fs";
import { dirname, resolve } from "node:path";

/** The subset of `node:fs` the helpers use, injectable so a test can record the exact sequence. */
export interface DurableFs {
	openSync: typeof nodeFs.openSync;
	/** `write(2)` semantics: returns the number of bytes accepted, which may be fewer than `length`. */
	writeSync: (fd: number, data: Uint8Array, offset: number, length: number) => number;
	fsyncSync: typeof nodeFs.fsyncSync;
	closeSync: typeof nodeFs.closeSync;
	renameSync: typeof nodeFs.renameSync;
	linkSync: typeof nodeFs.linkSync;
	unlinkSync: typeof nodeFs.unlinkSync;
	readFileSync: (path: string, encoding: "utf8") => string;
	/** Non-recursive `mkdir(2)`: throws `EEXIST` when the name exists. */
	mkdirSync: (path: string, mode: number) => void;
	existsSync: (path: string) => boolean;
}

export const NODE_FS: DurableFs = {
	openSync: nodeFs.openSync,
	writeSync: (fd, data, offset, length) => nodeFs.writeSync(fd, data, offset, length),
	fsyncSync: nodeFs.fsyncSync,
	closeSync: nodeFs.closeSync,
	renameSync: nodeFs.renameSync,
	linkSync: nodeFs.linkSync,
	unlinkSync: nodeFs.unlinkSync,
	readFileSync: (path, encoding) => nodeFs.readFileSync(path, encoding),
	mkdirSync: (path, mode) => {
		nodeFs.mkdirSync(path, { mode });
	},
	existsSync: nodeFs.existsSync,
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

/**
 * Write every byte of `data` to `fd`, honouring short writes. Throws
 * `ShortWrite` if a `write` accepts nothing, rather than spinning or
 * pretending: a record that cannot be written in full is not written.
 */
export function writeAllSync(fd: number, data: Uint8Array, fs: DurableFs = NODE_FS): void {
	let offset = 0;
	while (offset < data.length) {
		const remaining = data.length - offset;
		const accepted = fs.writeSync(fd, data, offset, remaining);
		// No progress, or more progress than was asked for: neither is a write
		// this loop can account for, and a record it cannot account for is not
		// published.
		if (!Number.isInteger(accepted) || accepted <= 0 || accepted > remaining) {
			throw new ShortWrite(offset, data.length, accepted);
		}
		offset += accepted;
	}
}

export class ShortWrite extends Error {
	readonly code = "short_write" as const;
	readonly written: number;
	readonly total: number;
	constructor(written: number, total: number, accepted: number) {
		super(`write accepted ${String(accepted)} bytes at offset ${written} of ${total}; the record cannot be completed`);
		this.name = "ShortWrite";
		this.written = written;
		this.total = total;
	}
}

function writeTemp(temp: string, contents: string, mode: number, fs: DurableFs): void {
	const fd = fs.openSync(temp, "w", mode);
	try {
		writeAllSync(fd, Buffer.from(contents, "utf8"), fs);
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

export type ExclusiveCreateOutcome =
	| { readonly outcome: "created" }
	| { readonly outcome: "exists"; readonly contents: string }
	/** The name existed when `link` ran and was gone by the time it was read: somebody released it. The caller retries. */
	| { readonly outcome: "vanished" };

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
 * one). The loser fsyncs the parent (the winner may have died before doing
 * so), then reads the winner's contents and returns them.
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
			// The loser is about to rely on the winner's name as authority. The
			// winner may have died between its link and its directory fsync, so
			// the loser fsyncs the parent itself before reading: a name it did
			// not confirm durable is not one it may act on.
			fsyncDirectory(dirname(path), fs);
			try {
				return { outcome: "exists", contents: fs.readFileSync(path, "utf8") };
			} catch (readError) {
				if ((readError as NodeJS.ErrnoException).code !== "ENOENT") throw readError;
				return { outcome: "vanished" };
			}
		}
		fsyncDirectory(dirname(path), fs);
		return { outcome: "created" };
	} finally {
		removeQuietly(temp, fs);
	}
}

/**
 * Create `dir` and any missing ancestors, durably: each directory that this
 * call creates is followed by an `fsync` of ITS PARENT, so the new entry is
 * on disk before the caller writes anything under it. Directories that
 * already exist are left alone and not fsynced. Returns the directories
 * created, outermost first.
 *
 * A directory that another creator made between this call's existence check
 * and its `mkdir` (`EEXIST`) is not reported as created, but its parent IS
 * fsynced here: the loser must not publish records under a directory whose
 * entry it never confirmed. A component that exists but is not a directory
 * surfaces as the error the next `mkdir` under it raises
 * (`ENOTDIR`/`ENOENT`); nothing is removed.
 */
export function mkdirDurable(dir: string, options: { mode?: number; fs?: DurableFs } = {}): string[] {
	const fs = options.fs ?? NODE_FS;
	const mode = options.mode ?? 0o700;
	const absolute = resolve(dir);
	const created: string[] = [];
	// Collect the missing suffix of the path, then create it top-down so every
	// parent exists (and is fsynced) before its child is made.
	const missing: string[] = [];
	for (let current = absolute; !fs.existsSync(current) && dirname(current) !== current; current = dirname(current)) {
		missing.unshift(current);
	}
	for (const path of missing) {
		let made = true;
		try {
			fs.mkdirSync(path, mode);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
			// A concurrent creator won the name. Its fsync of the parent is not
			// something this caller synchronised with, so fsync the parent here
			// too before anything is written under the directory.
			made = false;
		}
		fsyncDirectory(dirname(path), fs);
		if (made) created.push(path);
	}
	return created;
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
