/**
 * Session path policy (Issue #17, D8).
 *
 * Every Prime session the Governor creates carries an explicit, canonical,
 * persistent `sessionPath`. Issue #15 D8 measured why: a client-owned worker
 * created without one relaunched with an EMPTY live transcript even though its
 * JSONL on disk held every pre-crash turn, while the same worker created with
 * an explicit path resumed all of them. Recovery of a resident root (D1) is a
 * `create` on the same path, so the path IS the durable handle to the session.
 *
 * "Canonical" is defined here once. Prime canonicalises with `realpath` of the
 * file or, if the file does not exist yet, of its directory (session-lease.ts
 * `canonicalSessionPath`); the Governor applies the same rule, so both sides
 * agree on the lease identity, and then adds a fence Prime does not have: the
 * path must lie inside the session directory the Governor was configured
 * with. There is no fallback. A spec that omits the path fails preflight.
 */

import { realpathSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

export type SessionPathIssue =
	| "missing"
	| "not_a_string"
	| "empty"
	| "relative"
	| "not_jsonl"
	| "outside_session_dir"
	| "parent_missing"
	| "contains_nul";

export class SessionPathError extends Error {
	readonly code = "session_path_invalid" as const;
	readonly issue: SessionPathIssue;
	constructor(issue: SessionPathIssue, message: string) {
		super(message);
		this.name = "SessionPathError";
		this.issue = issue;
	}
}

/** A path that has passed {@link canonicalSessionPath}. The brand is the proof. */
export type CanonicalSessionPath = string & { readonly __brand: "CanonicalSessionPath" };

function realpathOrParent(path: string): string {
	try {
		return realpathSync(path);
	} catch {
		try {
			return join(realpathSync(dirname(path)), basename(path));
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") {
				throw new SessionPathError("parent_missing", `the directory that would hold ${path} does not exist; the Governor never creates transcript directories implicitly`);
			}
			throw error;
		}
	}
}

/**
 * Canonicalise a session path and fence it to `sessionDir`.
 *
 * Throws {@link SessionPathError}; never substitutes a default. Two spellings
 * of the same file (a relative segment, a symlinked parent, a trailing `./`)
 * canonicalise to one string, which is what makes the registry's by-path
 * index and Prime's lease agree.
 */
export function canonicalSessionPath(candidate: unknown, sessionDir: string): CanonicalSessionPath {
	if (candidate === undefined || candidate === null) {
		throw new SessionPathError("missing", "sessionPath is required: every Governor-created session names its transcript explicitly");
	}
	if (typeof candidate !== "string") {
		throw new SessionPathError("not_a_string", "sessionPath must be a string");
	}
	if (candidate.length === 0) {
		throw new SessionPathError("empty", "sessionPath must not be empty");
	}
	if (candidate.includes("\0")) {
		throw new SessionPathError("contains_nul", "sessionPath must not contain NUL");
	}
	if (!isAbsolute(candidate)) {
		throw new SessionPathError("relative", `sessionPath must be absolute, got ${JSON.stringify(candidate)}`);
	}
	if (!candidate.endsWith(".jsonl")) {
		throw new SessionPathError("not_jsonl", `sessionPath must name a .jsonl transcript, got ${JSON.stringify(candidate)}`);
	}
	const canonical = realpathOrParent(resolve(candidate));
	const canonicalDir = realpathOrParent(resolve(sessionDir));
	const rel = relative(canonicalDir, canonical);
	if (rel === "" || rel.startsWith(`..${sep}`) || rel === ".." || isAbsolute(rel)) {
		throw new SessionPathError(
			"outside_session_dir",
			`sessionPath ${canonical} is outside the configured session directory ${canonicalDir}`,
		);
	}
	return canonical as CanonicalSessionPath;
}

/** True when `candidate` would be accepted. Convenience over {@link canonicalSessionPath}. */
export function isAcceptableSessionPath(candidate: unknown, sessionDir: string): boolean {
	try {
		canonicalSessionPath(candidate, sessionDir);
		return true;
	} catch (error) {
		if (error instanceof SessionPathError) return false;
		throw error;
	}
}
