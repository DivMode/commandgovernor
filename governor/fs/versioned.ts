/**
 * A store of records that are updated by compare-and-swap on a version
 * number, shared safely by any number of processes, with no locks.
 *
 * Why: several Governors may share one state directory, and any authority
 * record written by "read the file, change it, rename over it" can be
 * regressed by a stale writer that read before someone else wrote. That
 * lost update is not a corruption a reader can detect; it is a record that
 * quietly says something older than the truth. The mutation ledger and the
 * session registry are both such authorities.
 *
 * Layout: `<dir>/<id>/v1.json, v2.json, ...`. Every version is immutable
 * and complete: it is published with an exclusive `link(2)` of an already
 * fsynced temp file (`createFileExclusiveDurable`), so no reader can see a
 * partial version and no two writers can publish the same version number.
 * A write is:
 *
 *   read the highest version N  ->  derive the next record from it  ->
 *   publish v(N+1) exclusively  ->  EEXIST? someone else won: retry from
 *   the read, against THEIR record.
 *
 * The derivation runs against the current record on every attempt, so a
 * precondition ("must be UNCERTAIN", "must not already be claimed") is
 * re-checked on the record that actually exists when the write lands. A
 * derivation that throws refuses the write; nothing is published. A
 * derivation may also answer {@link NO_CHANGE}, in which case nothing is
 * written and the current record is returned (idempotent writes cost no
 * version).
 *
 * Nothing here ever renames over, unlinks, or locks. There is no stale lock
 * to reclaim after a crash: a writer that dies mid-publish leaves at most a
 * temp file the next writer's `createFileExclusiveDurable` ignores. The
 * highest version is the record; the rest is its history, kept.
 *
 * Every lost attempt means another writer landed a version, so a bounded
 * retry count is exhausted only under sustained contention on ONE record;
 * a jittered pause between attempts keeps that far away, and exhaustion is
 * reported as `contended` with nothing written.
 */

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { createFileExclusiveDurable, mkdirDurable } from "./durable.ts";

export interface Versioned {
	/** The version this snapshot is; the file name `v<version>.json` is the authority. */
	readonly version: number;
}

export type VersionStoreErrorCode = "unknown_record" | "duplicate_record" | "contended" | "corrupt_history" | "bad_id";

export class VersionStoreError extends Error {
	readonly code: VersionStoreErrorCode;
	readonly id: string;
	constructor(code: VersionStoreErrorCode, id: string, message: string) {
		super(message);
		this.name = "VersionStoreError";
		this.code = code;
		this.id = id;
	}
}

/** Answer from a derivation meaning "the record already says this; write nothing". */
export const NO_CHANGE: unique symbol = Symbol("no_change");

export interface VersionStoreHooks {
	/**
	 * Runs after a write has read the current version and derived the next,
	 * before it tries to publish: exactly where a concurrent writer can win.
	 * A test seam; production passes none.
	 */
	readonly beforeCommit?: (id: string, fromVersion: number) => void;
}

export interface VersionStoreOptions {
	/** Attempts a write makes before reporting `contended`; defaults to {@link MAX_CAS_ATTEMPTS}. */
	readonly maxAttempts?: number;
	readonly hooks?: VersionStoreHooks;
	/** Which names are record ids; anything else under the store is a stray. */
	readonly idPattern?: RegExp;
	/** Names under the store that are known not to be records and are not reported as strays (e.g. lease files). */
	readonly ignore?: (name: string) => boolean;
}

/**
 * Attempts a write makes against concurrent writers before it reports
 * contention. Exhaustion needs this many OTHER writes to land on one
 * record while this one keeps losing; an independent review measured a
 * worst case of 54 attempts with 32 processes hammering one record before
 * the backoff existed.
 */
export const MAX_CAS_ATTEMPTS = 1024;

/** Upper bound of the jittered pause after a lost attempt, in milliseconds. */
const CAS_BACKOFF_MAX_MS = 25;

const VERSION_FILE = /^v([1-9]\d*)\.json$/;
const DEFAULT_ID = /^[A-Za-z0-9._:-]+$/;

const sleeper = new Int32Array(new SharedArrayBuffer(4));

/** Synchronous, jittered pause; the write path is synchronous on purpose (see durable.ts). */
function backoff(attempt: number): void {
	const cap = Math.min(CAS_BACKOFF_MAX_MS, 1 + attempt);
	const ms = Math.random() * cap;
	if (ms >= 0.5) Atomics.wait(sleeper, 0, 0, ms);
}

export class VersionStore<T extends Versioned> {
	readonly dir: string;
	readonly #maxAttempts: number;
	readonly #hooks: VersionStoreHooks;
	readonly #idPattern: RegExp;
	readonly #ignore: (name: string) => boolean;

	constructor(dir: string, options: VersionStoreOptions = {}) {
		this.dir = dir;
		mkdirDurable(dir, { mode: 0o700 });
		this.#maxAttempts = options.maxAttempts ?? MAX_CAS_ATTEMPTS;
		this.#hooks = options.hooks ?? {};
		this.#idPattern = options.idPattern ?? DEFAULT_ID;
		this.#ignore = options.ignore ?? (() => false);
	}

	recordDir(id: string): string {
		if (!this.#idPattern.test(id)) throw new VersionStoreError("bad_id", id, `refusing to use ${JSON.stringify(id)} as a record name`);
		return join(this.dir, id);
	}

	versionPath(id: string, version: number): string {
		return join(this.recordDir(id), `v${version}.json`);
	}

	/** Record ids under the store, and the entries that are neither records nor ignored. */
	entries(): { ids: string[]; strays: string[] } {
		const ids: string[] = [];
		const strays: string[] = [];
		for (const entry of readdirSync(this.dir, { withFileTypes: true })) {
			if (entry.isDirectory() && this.#idPattern.test(entry.name)) ids.push(entry.name);
			else if (!this.#ignore(entry.name)) strays.push(entry.name);
		}
		return { ids, strays };
	}

	/** The highest version on disk for `id`, or undefined for no record. */
	current(id: string): { record: T; version: number } | undefined {
		let names: string[];
		try {
			names = readdirSync(this.recordDir(id));
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
			throw error;
		}
		let version = 0;
		for (const name of names) {
			const match = VERSION_FILE.exec(name);
			if (match) version = Math.max(version, Number(match[1]));
		}
		if (version === 0) return undefined; // a directory with no published version (a creator died before its link)
		return { record: this.#read(id, version), version };
	}

	#read(id: string, version: number): T {
		// A version is published complete (link of an fsynced file); it is never partial.
		const parsed = JSON.parse(readFileSync(this.versionPath(id, version), "utf8")) as T;
		return { ...parsed, version };
	}

	get(id: string): T | undefined {
		return this.current(id)?.record;
	}

	require(id: string): T {
		const record = this.get(id);
		if (!record) throw new VersionStoreError("unknown_record", id, `no record ${id} under ${this.dir}`);
		return record;
	}

	/** The path of the current version of `id`; for operators and tests. */
	currentVersionPath(id: string): string {
		const current = this.current(id);
		if (!current) throw new VersionStoreError("unknown_record", id, `no record ${id} under ${this.dir}`);
		return this.versionPath(id, current.version);
	}

	/** Every version of `id`, oldest first. A gap is damage and is named as such. */
	history(id: string): T[] {
		const current = this.current(id);
		if (!current) throw new VersionStoreError("unknown_record", id, `no record ${id} under ${this.dir}`);
		const versions: T[] = [];
		for (let version = 1; version <= current.version; version += 1) {
			try {
				versions.push(this.#read(id, version));
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
				throw new VersionStoreError("corrupt_history", id, `${id}: version ${version} is missing although version ${current.version} exists`);
			}
		}
		return versions;
	}

	/** Every record's current version, in no particular order. */
	list(): T[] {
		return this.entries()
			.ids.map((id) => this.get(id))
			.filter((record): record is T => record !== undefined);
	}

	/**
	 * Publish version 1 of a new record, exclusively: two creators of one id
	 * cannot both succeed, and the loser is told so.
	 */
	create(id: string, record: Omit<T, "version">): T {
		const first = { ...record, version: 1 } as T;
		mkdirDurable(this.recordDir(id), { mode: 0o700 });
		const created = createFileExclusiveDurable(this.versionPath(id, 1), serialise(first), { mode: 0o600 });
		if (created.outcome !== "created") {
			throw new VersionStoreError("duplicate_record", id, `record ${id} already exists under ${this.dir}`);
		}
		return first;
	}

	/**
	 * The compare-and-swap: read the current version, derive the next record
	 * from it, publish it as the next version exclusively. A version that
	 * appears in between means another writer won; re-read and re-derive
	 * against the new record. `derive` throwing refuses the write; answering
	 * {@link NO_CHANGE} writes nothing and returns the current record.
	 */
	update(id: string, derive: (current: T) => Omit<T, "version"> | typeof NO_CHANGE): T {
		for (let attempt = 0; attempt < this.#maxAttempts; attempt += 1) {
			const current = this.current(id);
			if (!current) throw new VersionStoreError("unknown_record", id, `no record ${id} under ${this.dir}`);
			const derived = derive(current.record);
			if (derived === NO_CHANGE) return current.record;
			const next = { ...derived, version: current.version + 1 } as T;
			this.#hooks.beforeCommit?.(id, current.version);
			const published = createFileExclusiveDurable(this.versionPath(id, next.version), serialise(next), { mode: 0o600 });
			if (published.outcome === "created") return next;
			// "exists": a concurrent writer published this version; "vanished"
			// cannot happen (versions are never removed) and is treated the same.
			backoff(attempt);
		}
		throw new VersionStoreError("contended", id, `${id}: ${this.#maxAttempts} attempts each found a newer version; giving up without writing`);
	}
}

function serialise(record: unknown): string {
	return `${JSON.stringify(record, null, 2)}\n`;
}
