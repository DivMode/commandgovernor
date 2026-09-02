/**
 * Process identity beyond the pid.
 *
 * A pid is recycled. A lease that records only a pid can look live because
 * an unrelated process inherited the number, and a fence that honours it
 * then blocks recovery forever; a fence that reclaims it on a bad guess
 * double-recovers. Prime's own session lease and worker registry carry a
 * `(pid, processStartId)` pair for the same reason (`core/session-lease.ts`
 * `getProcessStartId`, `daemon-supervisor.ts` `processIdentity`). This is the
 * minimum port of that pattern, with the same conservative verdicts:
 *
 *   gone      the pid is not alive: the recorded process is certainly over
 *   replaced  the pid is alive but its start identity differs: the recorded
 *             process is over and something else has its number
 *   current   the pid is alive and its start identity matches
 *   unknown   cannot tell: no recorded start identity, or none observable now
 *
 * Callers reclaim only on `gone` or `replaced`, never on `unknown`.
 *
 * The start identity is the kernel's process start time, read from
 * `/proc/<pid>/stat` field 22 (`starttime`, in clock ticks since boot) on
 * Linux, and from `ps -o lstart=` (one-second resolution) on macOS and BSD.
 * Two processes that reuse a pid within the same second on macOS are
 * indistinguishable here; Prime accepts the same limit.
 */

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

import { isProcessAlive } from "../prime/substrate.ts";

export type ProcessIdentityVerdict = "current" | "replaced" | "gone" | "unknown";

/** A pid and, when it could be read, its start identity. */
export interface ProcessIdentity {
	readonly pid: number;
	readonly processStartId?: string;
}

/** The observations the verdict is built from; injectable so tests can fabricate pid reuse. */
export interface ProcessProbe {
	alive(pid: number): boolean;
	startId(pid: number): string | undefined;
}

function readProcStartTicks(pid: number): string | undefined {
	let stat: string;
	try {
		stat = readFileSync(`/proc/${pid}/stat`, "utf8");
	} catch {
		return undefined;
	}
	// The command name in field 2 is parenthesised and may itself contain
	// spaces or parentheses; everything after the LAST ")" is fixed-format.
	const commandEnd = stat.lastIndexOf(")");
	if (commandEnd < 0) return undefined;
	const fields = stat.slice(commandEnd + 2).split(" ");
	const startTime = fields[19];
	return startTime && /^\d+$/.test(startTime) ? `proc:${startTime}` : undefined;
}

function readPsStart(pid: number): string | undefined {
	try {
		const out = execFileSync("ps", ["-p", String(pid), "-o", "lstart="], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
		return out ? `ps:${out}` : undefined;
	} catch {
		return undefined;
	}
}

/** The start identity of `pid`, or undefined when it cannot be observed. Never throws. */
export function processStartId(pid: number): string | undefined {
	if (!Number.isInteger(pid) || pid <= 0) return undefined;
	if (process.platform === "linux") {
		const fromProc = readProcStartTicks(pid);
		if (fromProc !== undefined) return fromProc;
	}
	return readPsStart(pid);
}

let ownStartId: string | undefined;
let ownStartIdRead = false;

/** This process's own start identity, read once. */
export function currentProcessIdentity(): ProcessIdentity {
	if (!ownStartIdRead) {
		ownStartId = processStartId(process.pid);
		ownStartIdRead = true;
	}
	return ownStartId === undefined ? { pid: process.pid } : { pid: process.pid, processStartId: ownStartId };
}

export const LIVE_PROBE: ProcessProbe = { alive: isProcessAlive, startId: processStartId };

/**
 * Is the recorded process still the one behind its pid?
 *
 * `alive` treats "cannot signal" (EPERM) as alive, so a process the Governor
 * is not allowed to inspect is never called `gone`.
 */
export function classifyProcessIdentity(recorded: ProcessIdentity, probe: ProcessProbe = LIVE_PROBE): ProcessIdentityVerdict {
	if (!probe.alive(recorded.pid)) return "gone";
	if (recorded.processStartId === undefined) return "unknown";
	const observed = probe.startId(recorded.pid);
	if (observed === undefined) return "unknown";
	return observed === recorded.processStartId ? "current" : "replaced";
}

/** The verdicts on which a fence may take over a holder's place. */
export function identityProvesProcessOver(verdict: ProcessIdentityVerdict): boolean {
	return verdict === "gone" || verdict === "replaced";
}
