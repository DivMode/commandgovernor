/**
 * A canonical digest of a daemon command, so a durable mutation record can
 * prove that a later probe re-presents the SAME command and not merely one
 * of the same type.
 *
 * Why it matters: Prime keys its journal by `clientId + commandId`, and a
 * repeated id is answered from the stored result only if the supervisor
 * journaled the receipt. If the Governor died after writing DISPATCHED and
 * before the envelope reached the socket, Prime has no receipt, and whatever
 * the probe carries under that id is admitted as new work. Prime is right to
 * do so; the Governor is the one that must prove intent. A type check does
 * not: two `execute_bash_and_wait` bodies are the same type.
 *
 * "Canonical" here is JSON with object keys sorted recursively, no
 * whitespace, arrays in order, and `undefined` members omitted exactly as
 * `JSON.stringify` omits them. Two commands that serialise to the same wire
 * JSON, in any key order, have the same digest; anything else differs.
 */

import { createHash } from "node:crypto";

import type { DaemonCommand } from "../prime/protocol.ts";

/** `sha256:` followed by 64 hex digits. */
export const COMMAND_DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/;

export function canonicalJson(value: unknown): string {
	if (value === null || typeof value !== "object") {
		if (value === undefined || typeof value === "function" || typeof value === "symbol") {
			throw new TypeError(`cannot canonicalise a ${typeof value}`);
		}
		if (typeof value === "number" && !Number.isFinite(value)) {
			throw new TypeError("cannot canonicalise a non-finite number");
		}
		if (typeof value === "bigint") throw new TypeError("cannot canonicalise a bigint");
		return JSON.stringify(value);
	}
	if (Array.isArray(value)) {
		// JSON.stringify writes null for an undefined array element; so do we.
		return `[${value.map((item) => (item === undefined ? "null" : canonicalJson(item))).join(",")}]`;
	}
	if (typeof (value as { toJSON?: unknown }).toJSON === "function") {
		return canonicalJson((value as { toJSON: () => unknown }).toJSON());
	}
	const entries = Object.entries(value as Record<string, unknown>)
		.filter(([, item]) => item !== undefined && typeof item !== "function" && typeof item !== "symbol")
		.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
	return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`).join(",")}}`;
}

/** The digest of the complete wire command, every field included. */
export function commandDigest(command: DaemonCommand): string {
	return `sha256:${createHash("sha256").update(canonicalJson(command), "utf8").digest("hex")}`;
}
