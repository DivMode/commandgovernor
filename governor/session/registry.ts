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
 * Storage is a directory of JSON files written durably (`governor/fs/durable.ts`:
 * temp file, fsync, rename, fsync of the directory). No database: the record
 * set is small, and a file per session means two Governor processes on one
 * state dir contend per session, not globally.
 *
 * The recovery lease is the fence that makes "reopen exactly once" true under
 * concurrency. It is an exclusive, durable create of
 * `<sessionId>.json.recovery.lock` carrying the holder's `(pid, processStartId)`.
 * A holder whose process identity proves it over (`gone` or `replaced`) is
 * reclaimed with a report; a holder that is `current`, or whose identity
 * cannot be established (`unknown`), is never overridden. A recycled pid
 * therefore cannot keep a dead Governor's lease alive, and a lease the
 * Governor cannot inspect cannot be stolen.
 *
 * Reclaim itself is a compare-and-swap under a per-session reclaim mutex
 * (`<sessionId>.json.recovery.reclaim`, exclusive create): the dead lease is
 * replaced only if its bytes are still exactly the bytes that were
 * classified dead. Two reclaimers cannot both take over, and a reclaimer
 * can never delete a lease it did not inspect. The mutex is never stolen.
 */

import { closeSync, mkdirSync, openSync, readdirSync, readFileSync, unlinkSync, writeSync } from "node:fs";
import { join } from "node:path";

import { createFileExclusiveDurable, unlinkDurable, writeFileDurable } from "../fs/durable.ts";
import { classifyProcessIdentity, currentProcessIdentity, identityProvesProcessOver, LIVE_PROBE, type ProcessIdentity, type ProcessIdentityVerdict, type ProcessProbe } from "../process/identity.ts";
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

export class StaleCursorError extends Error {
	readonly code = "stale_cursor" as const;
	readonly sessionId: string;
	readonly presented: string;
	readonly current: string | undefined;
	constructor(sessionId: string, presented: string, current: string | undefined) {
		super(`event-cursor generation ${presented} belongs to a previous incarnation of ${sessionId}; current is ${current ?? "not yet observed"}`);
		this.name = "StaleCursorError";
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
	/** Why the holder was honoured: its process is `current`, or its identity is `unknown`. */
	readonly holderIdentity: ProcessIdentityVerdict;
	constructor(sessionId: string, holder: RecoveryLeaseRecord, holderIdentity: ProcessIdentityVerdict) {
		super(`recovery of ${sessionId} is owned by ${holder.ownerToken} (pid ${holder.pid}, process ${holderIdentity})`);
		this.name = "RecoveryLeaseHeld";
		this.sessionId = sessionId;
		this.holder = holder;
		this.holderIdentity = holderIdentity;
	}
}

/**
 * A reclaim could not enter its critical section because the per-session
 * reclaim mutex exists. `holderIdentity: current` is contention with a live
 * reclaimer; `gone`/`replaced` means a Governor died inside the (microsecond,
 * subprocess-free) critical section and left the mutex behind; `unknown`
 * means it cannot be told. None of these is reclaimed automatically: a mutex
 * that could be stolen would reintroduce the race it exists to close. An
 * operator who has confirmed the holder is gone removes the `.recovery.reclaim`
 * file.
 */
export class RecoveryReclaimBlocked extends Error {
	readonly code = "recovery_reclaim_blocked" as const;
	readonly sessionId: string;
	readonly holder: RecoveryLeaseRecord;
	readonly holderIdentity: ProcessIdentityVerdict;
	constructor(sessionId: string, holder: RecoveryLeaseRecord, holderIdentity: ProcessIdentityVerdict) {
		super(`reclaim of the recovery lease for ${sessionId} is blocked: the reclaim mutex is held by ${holder.ownerToken} (pid ${holder.pid}, process ${holderIdentity}); never taken over automatically`);
		this.name = "RecoveryReclaimBlocked";
		this.sessionId = sessionId;
		this.holder = holder;
		this.holderIdentity = holderIdentity;
	}
}

export interface RecoveryLeaseRecord {
	readonly sessionId: string;
	readonly ownerToken: string;
	readonly pid: number;
	/** The holder process's start identity, when it could be read at acquisition. */
	readonly processStartId?: string;
	readonly acquiredAt: string;
}

export interface RecoveryLease {
	readonly record: RecoveryLeaseRecord;
	/** The dead or replaced holder whose lease was taken over; reported, never silent. */
	readonly reclaimedFrom?: RecoveryLeaseRecord;
	/** The identity verdict that justified the reclaim. */
	readonly reclaimedBecause?: ProcessIdentityVerdict;
	release(): void;
}

export interface SessionRegistryOptions {
	/** How holder processes are inspected. Injectable so the suite can fabricate pid reuse. */
	readonly processProbe?: ProcessProbe;
	/** This process's identity as written into leases it takes. */
	readonly self?: ProcessIdentity;
}

function readRaw(path: string): string | undefined {
	try {
		return readFileSync(path, "utf8");
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw error;
	}
}

function readJson<T>(path: string): T | undefined {
	const raw = readRaw(path);
	return raw === undefined ? undefined : (JSON.parse(raw) as T);
}

function isLeaseRecord(value: unknown): value is RecoveryLeaseRecord {
	if (typeof value !== "object" || value === null) return false;
	const record = value as Record<string, unknown>;
	return (
		typeof record.sessionId === "string" &&
		typeof record.ownerToken === "string" &&
		typeof record.pid === "number" &&
		Number.isInteger(record.pid) &&
		(record.processStartId === undefined || typeof record.processStartId === "string") &&
		typeof record.acquiredAt === "string"
	);
}

export class SessionRegistry {
	readonly dir: string;
	readonly #probe: ProcessProbe;
	readonly #self: ProcessIdentity | undefined;

	constructor(stateDir: string, options: SessionRegistryOptions = {}) {
		this.dir = join(stateDir, "sessions");
		mkdirSync(this.dir, { recursive: true, mode: 0o700 });
		this.#probe = options.processProbe ?? LIVE_PROBE;
		this.#self = options.self;
	}

	#recordPath(sessionId: string): string {
		if (!/^[A-Za-z0-9._-]+$/.test(sessionId)) throw new Error(`refusing to use ${JSON.stringify(sessionId)} as a file name`);
		return join(this.dir, `${sessionId}.json`);
	}

	#leasePath(sessionId: string): string {
		return `${this.#recordPath(sessionId)}.recovery.lock`;
	}

	#write(sessionId: string, record: SessionRecord): void {
		writeFileDurable(this.#recordPath(sessionId), `${JSON.stringify(record, null, 2)}\n`, { mode: 0o600 });
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

	/**
	 * Bind the event-cursor generation observed on attach to an incarnation.
	 * Idempotent for the same value; a different generation for the same
	 * incarnation is refused, because a generation belongs to one worker
	 * process and an incarnation is one worker process.
	 */
	recordGeneration(sessionId: string, activeSessionId: string, generation: string): Incarnation {
		const record = this.require(sessionId);
		const index = record.incarnations.findIndex((inc) => inc.activeSessionId === activeSessionId);
		const known = record.incarnations[index];
		if (!known) throw new StaleIncarnationError(sessionId, activeSessionId, this.current(sessionId).activeSessionId);
		if (known.generation === generation) return known;
		if (known.generation !== undefined) {
			throw new Error(`incarnation ${activeSessionId} of ${sessionId} already has generation ${known.generation}; refusing to rebind it to ${generation}`);
		}
		const bound: Incarnation = { ...known, generation };
		const incarnations = [...record.incarnations];
		incarnations[index] = bound;
		this.#write(sessionId, { ...record, incarnations });
		return bound;
	}

	/** The cursor fence: the presented generation must be the current incarnation's. */
	assertCurrentGeneration(sessionId: string, generation: string): Incarnation {
		const current = this.current(sessionId);
		if (current.generation !== generation) {
			throw new StaleCursorError(sessionId, generation, current.generation);
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
		this.#write(input.sessionId, record);
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
		this.#write(input.sessionId, updated);
		return { record: updated, incarnation, appended: true };
	}

	/** The lease on disk for `sessionId`, if any, with the verdict on its holder's process and the exact bytes inspected. */
	inspectRecoveryLease(sessionId: string): { holder: RecoveryLeaseRecord; identity: ProcessIdentityVerdict; raw: string } | undefined {
		const raw = readRaw(this.#leasePath(sessionId));
		if (raw === undefined) return undefined;
		let parsed: unknown;
		try {
			parsed = JSON.parse(raw);
		} catch {
			parsed = null;
		}
		if (!isLeaseRecord(parsed)) {
			// A lease file that is not a lease record has no inspectable holder. It
			// is honoured, because "unknown" is never a licence to reclaim.
			return { holder: { sessionId, ownerToken: "<unreadable>", pid: -1, acquiredAt: "" }, identity: "unknown", raw };
		}
		return { holder: parsed, identity: classifyProcessIdentity(parsed, this.#probe), raw };
	}

	/**
	 * Take the recovery lease for `sessionId`, or throw {@link RecoveryLeaseHeld}
	 * (or {@link RecoveryReclaimBlocked}).
	 *
	 * Atomic and durable by `createFileExclusiveDurable`. A lease whose holder
	 * process is proven over (`gone` or `replaced` by pid reuse) is reclaimed
	 * exactly once, with the dead holder reported; a lease whose holder is
	 * `current`, or whose identity is `unknown`, is honoured.
	 *
	 * Reclaim is a compare-and-swap, not an unlink by name: the holder is
	 * classified OUTSIDE the critical section (that step may spawn `ps`), and
	 * then, under the per-session reclaim mutex, the lease bytes are re-read
	 * and compared with the bytes that were classified. Only an unchanged dead
	 * lease is replaced. Anything else means another process acted first, and
	 * this one re-inspects rather than deleting whatever is there now.
	 */
	acquireRecoveryLease(sessionId: string, ownerToken: string): RecoveryLease {
		this.require(sessionId);
		const path = this.#leasePath(sessionId);
		const self = this.#self ?? currentProcessIdentity();
		const record: RecoveryLeaseRecord = {
			sessionId,
			ownerToken,
			pid: self.pid,
			...(self.processStartId !== undefined ? { processStartId: self.processStartId } : {}),
			acquiredAt: new Date().toISOString(),
		};
		const contents = `${JSON.stringify(record)}\n`;
		const lease = (reclaimedFrom?: RecoveryLeaseRecord, reclaimedBecause?: ProcessIdentityVerdict): RecoveryLease => ({
			record,
			...(reclaimedFrom ? { reclaimedFrom, reclaimedBecause } : {}),
			release: () => {
				const onDisk = readRaw(path);
				if (onDisk === contents) unlinkDurable(path);
			},
		});
		for (let attempt = 0; attempt < 4; attempt += 1) {
			const created = createFileExclusiveDurable(path, contents, { mode: 0o600 });
			if (created.outcome === "created") return lease();
			if (created.outcome === "vanished") continue; // released between our EEXIST and our read
			const inspected = this.inspectRecoveryLease(sessionId);
			if (!inspected) continue; // released between our EEXIST and our inspection
			const { holder, identity, raw } = inspected;
			if (holder.ownerToken === ownerToken || !identityProvesProcessOver(identity)) {
				throw new RecoveryLeaseHeld(sessionId, holder, identity);
			}
			const replaced = this.#replaceDeadLease(sessionId, raw, contents, record);
			if (replaced === "reclaimed") return lease(holder, identity);
			if (replaced === "created") return lease(); // the dead holder released it itself first; nothing was reclaimed
			// Someone else acted between our inspection and our critical section: look again.
		}
		const inspected = this.inspectRecoveryLease(sessionId);
		throw new RecoveryLeaseHeld(sessionId, inspected?.holder ?? record, inspected?.identity ?? "unknown");
	}

	/**
	 * The critical section of a reclaim. Exclusive by the reclaim mutex; no
	 * subprocess and no classification inside it. Returns true when the dead
	 * lease whose bytes were `expected` was replaced by `contents`, false when
	 * the file had changed or vanished-and-been-retaken, in which case nothing
	 * was deleted.
	 */
	#replaceDeadLease(sessionId: string, expected: string, contents: string, self: RecoveryLeaseRecord): "reclaimed" | "created" | "changed" {
		const path = this.#leasePath(sessionId);
		const mutex = this.#reclaimMutexPath(sessionId);
		let fd: number;
		try {
			fd = openSync(mutex, "wx", 0o600);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
			throw this.#reclaimBlocked(sessionId);
		}
		try {
			try {
				writeSync(fd, `${JSON.stringify({ ...self, stage: "reclaim" })}\n`);
			} finally {
				closeSync(fd);
			}
			const now = readRaw(path);
			if (now !== undefined && now !== expected) return "changed";
			const wasDead = now !== undefined;
			if (wasDead) unlinkDurable(path);
			// A fresh acquirer (no mutex needed for a plain create) may have taken
			// the name while it was absent; then it holds, legitimately, and we do not.
			const created = createFileExclusiveDurable(path, contents, { mode: 0o600 });
			if (created.outcome !== "created") return "changed";
			return wasDead ? "reclaimed" : "created";
		} finally {
			try {
				unlinkSync(mutex);
			} catch {
				// Already gone; nothing to hold open.
			}
		}
	}

	#reclaimMutexPath(sessionId: string): string {
		return `${this.#recordPath(sessionId)}.recovery.reclaim`;
	}

	#reclaimBlocked(sessionId: string): RecoveryReclaimBlocked {
		const raw = readRaw(this.#reclaimMutexPath(sessionId));
		let parsed: unknown = null;
		try {
			parsed = raw === undefined ? undefined : JSON.parse(raw);
		} catch {
			parsed = null;
		}
		if (parsed === undefined) {
			// The mutex was released between our EEXIST and our read: contention, not a stale mutex.
			return new RecoveryReclaimBlocked(sessionId, { sessionId, ownerToken: "<released>", pid: -1, acquiredAt: "" }, "current");
		}
		if (!isLeaseRecord(parsed)) {
			return new RecoveryReclaimBlocked(sessionId, { sessionId, ownerToken: "<unreadable>", pid: -1, acquiredAt: "" }, "unknown");
		}
		return new RecoveryReclaimBlocked(sessionId, parsed, classifyProcessIdentity(parsed, this.#probe));
	}
}
