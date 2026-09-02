/**
 * The Governor's Prime client identity, persisted per state directory.
 *
 * Prime keys its mutation journal by `clientId + commandId`. The Governor
 * therefore has exactly one client identity per state directory, for the
 * lifetime of that directory: a Governor that minted a fresh id on restart
 * would turn every probe of an UNCERTAIN command into new work under a new
 * journal identity, and a second Governor that raced the first to create the
 * file would leave two identities with one directory.
 *
 * Creation is atomic and durable (`createFileExclusiveDurable`): the
 * candidate is written and fsynced to a temp file, published with `link(2)`,
 * which cannot replace an existing name, and the parent directory is fsynced.
 * A loser of the race reads the winner. No reader can observe a partial
 * file, and no writer can ever overwrite an existing identity: a missing,
 * unreadable or malformed file is an error, never a reason to mint again.
 *
 * The identity file is a JSON record rather than a bare string so a later
 * reader can tell an identity from a stray file, and so the process that
 * created it is on record.
 */

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { createFileExclusiveDurable, fsyncDirectory } from "../fs/durable.ts";
import { currentProcessIdentity, type ProcessIdentity } from "../process/identity.ts";

export const CLIENT_IDENTITY_FILE = "client-identity.json";

/** `cg:` followed by a v4 UUID; nothing else is a Governor client id. */
export const CLIENT_ID_PATTERN = /^cg:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export interface ClientIdentityRecord {
	readonly schemaVersion: 1;
	readonly clientId: string;
	readonly createdAt: string;
	readonly createdBy: ProcessIdentity;
}

export type ClientIdentityErrorCode = "identity_missing" | "identity_unreadable" | "identity_malformed";

export class ClientIdentityError extends Error {
	readonly code: ClientIdentityErrorCode;
	readonly path: string;
	constructor(code: ClientIdentityErrorCode, path: string, detail: string) {
		super(`client identity at ${path}: ${detail}`);
		this.name = "ClientIdentityError";
		this.code = code;
		this.path = path;
	}
}

export function clientIdentityPath(stateDir: string): string {
	return join(stateDir, CLIENT_IDENTITY_FILE);
}

export function isClientId(value: unknown): value is string {
	return typeof value === "string" && CLIENT_ID_PATTERN.test(value);
}

export function newClientId(): string {
	return `cg:${randomUUID()}`;
}

function parseRecord(path: string, contents: string): ClientIdentityRecord {
	let parsed: unknown;
	try {
		parsed = JSON.parse(contents);
	} catch {
		throw new ClientIdentityError("identity_malformed", path, "not JSON");
	}
	if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
		throw new ClientIdentityError("identity_malformed", path, "not an object");
	}
	const record = parsed as Record<string, unknown>;
	if (record.schemaVersion !== 1) throw new ClientIdentityError("identity_malformed", path, `schemaVersion ${String(record.schemaVersion)}`);
	if (!isClientId(record.clientId)) throw new ClientIdentityError("identity_malformed", path, "clientId is not a Governor client id");
	if (typeof record.createdAt !== "string") throw new ClientIdentityError("identity_malformed", path, "createdAt missing");
	const createdBy = record.createdBy;
	if (typeof createdBy !== "object" || createdBy === null || typeof (createdBy as { pid?: unknown }).pid !== "number") {
		throw new ClientIdentityError("identity_malformed", path, "createdBy missing");
	}
	return parsed as ClientIdentityRecord;
}

/**
 * Read the identity on disk. Missing, unreadable and malformed files are
 * distinct typed errors; none of them is a reason to create a new identity.
 */
export function readClientIdentity(stateDir: string): ClientIdentityRecord {
	const path = clientIdentityPath(stateDir);
	let contents: string;
	try {
		contents = readFileSync(path, "utf8");
	} catch (error) {
		const code = (error as NodeJS.ErrnoException).code;
		if (code === "ENOENT") throw new ClientIdentityError("identity_missing", path, "no identity file");
		throw new ClientIdentityError("identity_unreadable", path, `${String(code)}: ${(error as Error).message}`);
	}
	return parseRecord(path, contents);
}

export interface LoadedClientIdentity {
	readonly record: ClientIdentityRecord;
	/** True when this call created the identity; false when it read an existing one (including a race it lost). */
	readonly created: boolean;
}

/**
 * Test seams. `candidate` replaces the random id; `beforeCommit` runs after
 * the candidate is chosen and before it is published, which is where a
 * concurrent creator can win. Production callers pass neither.
 */
export interface LoadClientIdentityHooks {
	readonly candidate?: () => string;
	readonly beforeCommit?: () => void;
}

/**
 * The identity for `stateDir`: the existing one if the file exists, else
 * exactly one newly created one, whoever else is creating at the same time.
 */
export function loadOrCreateClientIdentity(stateDir: string, hooks: LoadClientIdentityHooks = {}): LoadedClientIdentity {
	const path = clientIdentityPath(stateDir);
	try {
		const record = readClientIdentity(stateDir);
		// The name was linked by some process at some time, and a link is visible
		// to other processes before its creator's directory fsync completes. This
		// identity is about to be stamped on every envelope, so confirm the name
		// durable here rather than assume the creator finished.
		fsyncDirectory(stateDir);
		return { record, created: false };
	} catch (error) {
		if (!(error instanceof ClientIdentityError) || error.code !== "identity_missing") throw error;
	}
	const clientId = hooks.candidate ? hooks.candidate() : newClientId();
	if (!isClientId(clientId)) throw new Error(`candidate ${JSON.stringify(clientId)} is not a Governor client id`);
	const record: ClientIdentityRecord = {
		schemaVersion: 1,
		clientId,
		createdAt: new Date().toISOString(),
		createdBy: currentProcessIdentity(),
	};
	hooks.beforeCommit?.();
	const contents = `${JSON.stringify(record, null, 2)}\n`;
	for (let attempt = 0; attempt < 8; attempt += 1) {
		const result = createFileExclusiveDurable(path, contents, { mode: 0o600 });
		if (result.outcome === "created") return { record, created: true };
		// Lost the race: the winner's record is the identity. It is validated like
		// any other read, so a foreign file that appeared under the name is an
		// error rather than an identity.
		if (result.outcome === "exists") return { record: parseRecord(path, result.contents), created: false };
		// "vanished": something removed the winner between our link and our read.
		// Nothing in the Governor does that to an identity; whatever did, the
		// name is free again and this candidate is still valid.
	}
	throw new ClientIdentityError("identity_unreadable", path, "the identity file kept appearing and vanishing; refusing to guess");
}
