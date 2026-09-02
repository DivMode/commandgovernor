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
 */

import { mkdirSync, readdirSync, readFileSync } from "node:fs";
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

function readJson<T>(path: string): T | undefined {
	try {
		return JSON.parse(readFileSync(path, "utf8")) as T;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw error;
	}
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

	/** The lease on disk for `sessionId`, if any, with the verdict on its holder's process. */
	inspectRecoveryLease(sessionId: string): { holder: RecoveryLeaseRecord; identity: ProcessIdentityVerdict } | undefined {
		let raw: unknown;
		try {
			raw = readJson<unknown>(this.#leasePath(sessionId));
		} catch (error) {
			if (!(error instanceof SyntaxError)) throw error;
			raw = null; // present but not JSON: an uninspectable holder, handled below
		}
		if (raw === undefined) return undefined;
		if (!isLeaseRecord(raw)) {
			// A lease file that is not a lease record has no inspectable holder. It
			// is honoured, because "unknown" is never a licence to reclaim.
			return { holder: { sessionId, ownerToken: "<unreadable>", pid: -1, acquiredAt: "" }, identity: "unknown" };
		}
		return { holder: raw, identity: classifyProcessIdentity(raw, this.#probe) };
	}

	/**
	 * Take the recovery lease for `sessionId`, or throw {@link RecoveryLeaseHeld}.
	 *
	 * Atomic and durable by `createFileExclusiveDurable`. A lease whose holder
	 * process is proven over (`gone` or `replaced` by pid reuse) is reclaimed
	 * exactly once, with the dead holder reported; a lease whose holder is
	 * `current`, or whose identity is `unknown`, is honoured.
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
		let reclaimedFrom: RecoveryLeaseRecord | undefined;
		let reclaimedBecause: ProcessIdentityVerdict | undefined;
		for (let attempt = 0; attempt < 2; attempt += 1) {
			const created = createFileExclusiveDurable(path, `${JSON.stringify(record)}\n`, { mode: 0o600 });
			if (created.outcome === "created") {
				return {
					record,
					...(reclaimedFrom ? { reclaimedFrom, reclaimedBecause } : {}),
					release: () => {
						const onDisk = readJson<RecoveryLeaseRecord>(path);
						if (onDisk && onDisk.ownerToken === ownerToken) unlinkDurable(path);
					},
				};
			}
			const inspected = this.inspectRecoveryLease(sessionId);
			if (!inspected) continue; // released between our EEXIST and our read
			const { holder, identity } = inspected;
			if (holder.ownerToken === ownerToken || !identityProvesProcessOver(identity)) {
				throw new RecoveryLeaseHeld(sessionId, holder, identity);
			}
			reclaimedFrom = holder;
			reclaimedBecause = identity;
			unlinkDurable(path);
		}
		const inspected = this.inspectRecoveryLease(sessionId);
		throw new RecoveryLeaseHeld(sessionId, inspected?.holder ?? record, inspected?.identity ?? "unknown");
	}
}
