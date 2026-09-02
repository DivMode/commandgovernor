/**
 * The Governor's durable session registry (Issue #17, D1).
 *
 * One record per logical session, keyed by Prime's stable `sessionId`, with
 * the ordered list of INCARNATIONS that session has had. Prime's
 * active-session id changes every time a resident root is reopened after its
 * worker dies (Issue #15 D1), so it is recorded as an attachment identity and
 * never used as a key. The Rust oracle called the same thing a "session
 * incarnation" (WRK-018, `stale_session_incarnation`) and this is its port.
 *
 * Storage is a directory of JSON files written atomically (temp file, fsync,
 * rename). No database: the record set is small, and a file per session means
 * two Governor processes on one state dir contend per session, not globally.
 *
 * The recovery lease is the fence that makes "reopen exactly once" true under
 * concurrency. It is an `O_EXCL` create of `<sessionId>.recovery.lock`; a
 * holder whose pid is gone is reclaimed with a report, a holder whose pid is
 * alive is never overridden. "Cannot tell" (EPERM) counts as alive.
 */

import { closeSync, fsyncSync, mkdirSync, openSync, readdirSync, readFileSync, renameSync, unlinkSync, writeSync } from "node:fs";
import { join } from "node:path";

import { isProcessAlive } from "../prime/substrate.ts";
import type { CanonicalSessionPath } from "./paths.ts";

export type IncarnationCause = "create" | "reopen" | "converged";

export interface Incarnation {
	readonly index: number;
	readonly activeSessionId: string;
	readonly workerPid?: number;
	/** Event-cursor generation observed on attach, when known. */
	readonly generation?: string;
	readonly openedAt: string;
	readonly cause: IncarnationCause;
	/** Owner token of the Governor instance that recorded it. */
	readonly openedBy: string;
}

export interface SessionRecord {
	readonly schemaVersion: 1;
	/** Prime's stable `sessionId`. The logical identity. */
	readonly sessionId: string;
	readonly sessionPath: CanonicalSessionPath;
	readonly lifecycle: "resident" | "client_owned";
	readonly createdAt: string;
	readonly incarnations: readonly Incarnation[];
}

export class StaleIncarnationError extends Error {
	readonly code = "stale_incarnation" as const;
	readonly sessionId: string;
	readonly presented: string;
	readonly current: string;
	constructor(sessionId: string, presented: string, current: string) {
		super(`active-session id ${presented} is a stale incarnation of ${sessionId}; current is ${current}`);
		this.name = "StaleIncarnationError";
		this.sessionId = sessionId;
		this.presented = presented;
		this.current = current;
	}
}

export class UnknownSessionError extends Error {
	readonly code = "unknown_session" as const;
	readonly sessionId: string;
	constructor(sessionId: string) {
		super(`no registry record for session ${sessionId}`);
		this.name = "UnknownSessionError";
		this.sessionId = sessionId;
	}
}

export class RecoveryLeaseHeld extends Error {
	readonly code = "recovery_lease_held" as const;
	readonly sessionId: string;
	readonly holder: RecoveryLeaseRecord;
	constructor(sessionId: string, holder: RecoveryLeaseRecord) {
		super(`recovery of ${sessionId} is owned by ${holder.ownerToken} (pid ${holder.pid})`);
		this.name = "RecoveryLeaseHeld";
		this.sessionId = sessionId;
		this.holder = holder;
	}
}

export interface RecoveryLeaseRecord {
	readonly sessionId: string;
	readonly ownerToken: string;
	readonly pid: number;
	readonly acquiredAt: string;
}

export interface RecoveryLease {
	readonly record: RecoveryLeaseRecord;
	/** True when a dead holder's lease was taken over; reported, never silent. */
	readonly reclaimedFrom?: RecoveryLeaseRecord;
	release(): void;
}

function writeAtomic(path: string, contents: string): void {
	const temp = `${path}.${process.pid}.${Date.now()}.tmp`;
	const fd = openSync(temp, "w", 0o600);
	try {
		writeSync(fd, contents);
		fsyncSync(fd);
	} finally {
		closeSync(fd);
	}
	renameSync(temp, path);
}

function readJson<T>(path: string): T | undefined {
	try {
		return JSON.parse(readFileSync(path, "utf8")) as T;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw error;
	}
}

export class SessionRegistry {
	readonly dir: string;

	constructor(stateDir: string) {
		this.dir = join(stateDir, "sessions");
		mkdirSync(this.dir, { recursive: true, mode: 0o700 });
	}

	#recordPath(sessionId: string): string {
		if (!/^[A-Za-z0-9._-]+$/.test(sessionId)) throw new Error(`refusing to use ${JSON.stringify(sessionId)} as a file name`);
		return join(this.dir, `${sessionId}.json`);
	}

	#leasePath(sessionId: string): string {
		return `${this.#recordPath(sessionId)}.recovery.lock`;
	}

	get(sessionId: string): SessionRecord | undefined {
		return readJson<SessionRecord>(this.#recordPath(sessionId));
	}

	require(sessionId: string): SessionRecord {
		const record = this.get(sessionId);
		if (!record) throw new UnknownSessionError(sessionId);
		return record;
	}

	list(): SessionRecord[] {
		return readdirSync(this.dir)
			.filter((name) => name.endsWith(".json"))
			.map((name) => readJson<SessionRecord>(join(this.dir, name)))
			.filter((record): record is SessionRecord => record !== undefined)
			.sort((a, b) => a.createdAt.localeCompare(b.createdAt));
	}

	/** The record for a canonical path, if one logical session owns it. */
	findByPath(sessionPath: CanonicalSessionPath): SessionRecord | undefined {
		return this.list().find((record) => record.sessionPath === sessionPath);
	}

	current(sessionId: string): Incarnation {
		const record = this.require(sessionId);
		const current = record.incarnations[record.incarnations.length - 1];
		if (!current) throw new Error(`session ${sessionId} has no incarnation`);
		return current;
	}

	/** The fence every mutation passes: the presented id must be the current incarnation. */
	assertCurrent(sessionId: string, activeSessionId: string): Incarnation {
		const current = this.current(sessionId);
		if (current.activeSessionId !== activeSessionId) {
			throw new StaleIncarnationError(sessionId, activeSessionId, current.activeSessionId);
		}
		return current;
	}

	/** Record a brand-new logical session with its first incarnation. */
	create(input: {
		sessionId: string;
		sessionPath: CanonicalSessionPath;
		lifecycle: "resident" | "client_owned";
		activeSessionId: string;
		workerPid?: number;
		generation?: string;
		openedBy: string;
	}): SessionRecord {
		const existing = this.get(input.sessionId);
		if (existing) {
			throw new Error(`session ${input.sessionId} already has a registry record; use recordIncarnation`);
		}
		const owner = this.findByPath(input.sessionPath);
		if (owner) {
			throw new Error(`session path ${input.sessionPath} is already bound to session ${owner.sessionId}`);
		}
		const now = new Date().toISOString();
		const record: SessionRecord = {
			schemaVersion: 1,
			sessionId: input.sessionId,
			sessionPath: input.sessionPath,
			lifecycle: input.lifecycle,
			createdAt: now,
			incarnations: [
				{
					index: 0,
					activeSessionId: input.activeSessionId,
					...(input.workerPid !== undefined ? { workerPid: input.workerPid } : {}),
					...(input.generation !== undefined ? { generation: input.generation } : {}),
					openedAt: now,
					cause: "create",
					openedBy: input.openedBy,
				},
			],
		};
		writeAtomic(this.#recordPath(input.sessionId), `${JSON.stringify(record, null, 2)}\n`);
		return record;
	}

	/**
	 * Append an incarnation. Idempotent for the same active-session id: a
	 * second observer of the same reopen converges instead of duplicating.
	 */
	recordIncarnation(input: {
		sessionId: string;
		activeSessionId: string;
		workerPid?: number;
		generation?: string;
		cause: Exclude<IncarnationCause, "create">;
		openedBy: string;
	}): { record: SessionRecord; incarnation: Incarnation; appended: boolean } {
		const record = this.require(input.sessionId);
		const known = record.incarnations.find((inc) => inc.activeSessionId === input.activeSessionId);
		if (known) return { record, incarnation: known, appended: false };
		const incarnation: Incarnation = {
			index: record.incarnations.length,
			activeSessionId: input.activeSessionId,
			...(input.workerPid !== undefined ? { workerPid: input.workerPid } : {}),
			...(input.generation !== undefined ? { generation: input.generation } : {}),
			openedAt: new Date().toISOString(),
			cause: input.cause,
			openedBy: input.openedBy,
		};
		const updated: SessionRecord = { ...record, incarnations: [...record.incarnations, incarnation] };
		writeAtomic(this.#recordPath(input.sessionId), `${JSON.stringify(updated, null, 2)}\n`);
		return { record: updated, incarnation, appended: true };
	}

	/**
	 * Take the recovery lease for `sessionId`, or throw {@link RecoveryLeaseHeld}.
	 *
	 * Atomic by `O_EXCL`. A lease whose holder pid no longer exists is stale
	 * and is reclaimed exactly once, with the dead holder reported; a lease
	 * whose holder is alive -- or cannot be inspected -- is honoured.
	 */
	acquireRecoveryLease(sessionId: string, ownerToken: string): RecoveryLease {
		this.require(sessionId);
		const path = this.#leasePath(sessionId);
		const record: RecoveryLeaseRecord = {
			sessionId,
			ownerToken,
			pid: process.pid,
			acquiredAt: new Date().toISOString(),
		};
		let reclaimedFrom: RecoveryLeaseRecord | undefined;
		for (let attempt = 0; attempt < 2; attempt += 1) {
			try {
				const fd = openSync(path, "wx", 0o600);
				try {
					writeSync(fd, `${JSON.stringify(record)}\n`);
					fsyncSync(fd);
				} finally {
					closeSync(fd);
				}
				return {
					record,
					...(reclaimedFrom ? { reclaimedFrom } : {}),
					release: () => {
						const onDisk = readJson<RecoveryLeaseRecord>(path);
						if (onDisk && onDisk.ownerToken === ownerToken) unlinkSync(path);
					},
				};
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
				const holder = readJson<RecoveryLeaseRecord>(path);
				if (!holder) continue; // released between our EEXIST and our read
				if (holder.ownerToken === ownerToken || isProcessAlive(holder.pid)) {
					throw new RecoveryLeaseHeld(sessionId, holder);
				}
				reclaimedFrom = holder;
				try {
					unlinkSync(path);
				} catch (unlinkError) {
					if ((unlinkError as NodeJS.ErrnoException).code !== "ENOENT") throw unlinkError;
				}
			}
		}
		const holder = readJson<RecoveryLeaseRecord>(path);
		throw new RecoveryLeaseHeld(sessionId, holder ?? record);
	}
}
