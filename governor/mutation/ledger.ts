/**
 * The Governor's durable mutation ledger (Issue #17, D2).
 *
 * One record per Governor-issued command id. The record is written in state
 * DISPATCHED **before** the envelope is written to the socket -- durable
 * intent before external I/O -- so that a Governor process that dies between
 * the two leaves a record that says "may have been sent", which is the truth.
 *
 * States and the only legal transitions:
 *
 *   DISPATCHED -> COMPLETED   (success response)
 *   DISPATCHED -> FAILED      (typed pre-effect rejection)
 *   DISPATCHED -> UNCERTAIN   (anything else)
 *   UNCERTAIN  -> COMPLETED   (resolveUncertain with effect_observed)
 *   UNCERTAIN  -> FAILED      (resolveUncertain with effect_absent_proven)
 *
 * There is no transition out of UNCERTAIN without evidence, no transition
 * back to DISPATCHED, and no operation that mints a replacement command id
 * for an existing record. Re-dispatch of an uncertain mutation is a human
 * decision expressed as a NEW command with its own record and an explicit
 * `supersedes` link, never an automatic one.
 */

import { closeSync, fsyncSync, mkdirSync, openSync, readdirSync, readFileSync, renameSync, writeSync } from "node:fs";
import { join } from "node:path";

import type { DaemonResponse } from "../prime/protocol.ts";
import type { PreEffectProof, UncertainReason } from "./classify.ts";

export type MutationState = "DISPATCHED" | "COMPLETED" | "FAILED" | "UNCERTAIN";

export type ResolutionEvidence =
	| { readonly kind: "effect_observed"; readonly by: string; readonly detail: string; readonly observedAt: string }
	| { readonly kind: "effect_absent_proven"; readonly by: string; readonly detail: string; readonly observedAt: string };

export interface MutationTransition {
	readonly at: string;
	readonly to: MutationState;
	readonly reason: string;
	readonly proof?: PreEffectProof;
	readonly uncertainReason?: UncertainReason;
	readonly evidence?: ResolutionEvidence;
	readonly response?: DaemonResponse;
}

export interface MutationRecord {
	readonly schemaVersion: 1;
	readonly commandId: string;
	readonly clientId: string;
	readonly commandType: string;
	readonly sessionId: string;
	readonly activeSessionId: string;
	readonly incarnationIndex: number;
	readonly state: MutationState;
	readonly dispatchedAt: string;
	readonly transitions: readonly MutationTransition[];
	/** A human-issued replacement names the uncertain record it supersedes. */
	readonly supersedes?: string;
	/** Attempts to fetch the substrate's stored result for this id, for the record. */
	readonly probes?: readonly { readonly at: string; readonly response?: DaemonResponse; readonly detail?: string }[];
}

export type MutationLedgerErrorCode = "duplicate_command_id" | "illegal_transition" | "unknown_command" | "supersedes_not_uncertain";

export class MutationLedgerError extends Error {
	readonly code: MutationLedgerErrorCode;
	constructor(code: MutationLedgerErrorCode, message: string) {
		super(message);
		this.name = "MutationLedgerError";
		this.code = code;
	}
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

export class MutationLedger {
	readonly dir: string;

	constructor(stateDir: string) {
		this.dir = join(stateDir, "mutations");
		mkdirSync(this.dir, { recursive: true, mode: 0o700 });
	}

	#path(commandId: string): string {
		if (!/^[A-Za-z0-9._:-]+$/.test(commandId)) throw new Error(`refusing to use ${JSON.stringify(commandId)} as a file name`);
		return join(this.dir, `${commandId}.json`);
	}

	get(commandId: string): MutationRecord | undefined {
		try {
			return JSON.parse(readFileSync(this.#path(commandId), "utf8")) as MutationRecord;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
			throw error;
		}
	}

	require(commandId: string): MutationRecord {
		const record = this.get(commandId);
		if (!record) throw new MutationLedgerError("unknown_command", `no ledger record for ${commandId}`);
		return record;
	}

	list(): MutationRecord[] {
		return readdirSync(this.dir)
			.filter((name) => name.endsWith(".json"))
			.map((name) => JSON.parse(readFileSync(join(this.dir, name), "utf8")) as MutationRecord)
			.sort((a, b) => a.dispatchedAt.localeCompare(b.dispatchedAt));
	}

	/**
	 * Durably record intent. Must be called, and must return, before the
	 * envelope is written to the socket. Refuses an id that already exists:
	 * a command id is dispatched once, ever.
	 */
	recordDispatch(input: {
		commandId: string;
		clientId: string;
		commandType: string;
		sessionId: string;
		activeSessionId: string;
		incarnationIndex: number;
		supersedes?: string;
	}): MutationRecord {
		if (this.get(input.commandId)) {
			throw new MutationLedgerError("duplicate_command_id", `command id ${input.commandId} was already dispatched; a command id is never reused`);
		}
		if (input.supersedes !== undefined) {
			const prior = this.require(input.supersedes);
			if (prior.state !== "UNCERTAIN") {
				throw new MutationLedgerError("supersedes_not_uncertain", `${input.supersedes} is ${prior.state}, not UNCERTAIN; only an uncertain mutation may be superseded`);
			}
		}
		const now = new Date().toISOString();
		const record: MutationRecord = {
			schemaVersion: 1,
			commandId: input.commandId,
			clientId: input.clientId,
			commandType: input.commandType,
			sessionId: input.sessionId,
			activeSessionId: input.activeSessionId,
			incarnationIndex: input.incarnationIndex,
			state: "DISPATCHED",
			dispatchedAt: now,
			transitions: [{ at: now, to: "DISPATCHED", reason: "intent recorded before send" }],
			...(input.supersedes !== undefined ? { supersedes: input.supersedes } : {}),
		};
		writeAtomic(this.#path(input.commandId), `${JSON.stringify(record, null, 2)}\n`);
		return record;
	}

	#transition(commandId: string, from: readonly MutationState[], transition: MutationTransition): MutationRecord {
		const record = this.require(commandId);
		if (!from.includes(record.state)) {
			throw new MutationLedgerError("illegal_transition", `${commandId}: ${record.state} -> ${transition.to} is not a legal transition`);
		}
		const updated: MutationRecord = { ...record, state: transition.to, transitions: [...record.transitions, transition] };
		writeAtomic(this.#path(commandId), `${JSON.stringify(updated, null, 2)}\n`);
		return updated;
	}

	markCompleted(commandId: string, response: DaemonResponse): MutationRecord {
		return this.#transition(commandId, ["DISPATCHED"], { at: new Date().toISOString(), to: "COMPLETED", reason: "success response", response });
	}

	markFailed(commandId: string, proof: PreEffectProof, response: DaemonResponse): MutationRecord {
		return this.#transition(commandId, ["DISPATCHED"], { at: new Date().toISOString(), to: "FAILED", reason: `typed pre-effect rejection ${proof.code}`, proof, response });
	}

	markUncertain(commandId: string, uncertainReason: UncertainReason, response?: DaemonResponse, detail?: string): MutationRecord {
		return this.#transition(commandId, ["DISPATCHED"], {
			at: new Date().toISOString(),
			to: "UNCERTAIN",
			reason: detail ? `${uncertainReason}: ${detail}` : uncertainReason,
			uncertainReason,
			...(response ? { response } : {}),
		});
	}

	/** The only way out of UNCERTAIN: exact evidence about the external effect. */
	resolveUncertain(commandId: string, evidence: ResolutionEvidence): MutationRecord {
		const to: MutationState = evidence.kind === "effect_observed" ? "COMPLETED" : "FAILED";
		return this.#transition(commandId, ["UNCERTAIN"], { at: new Date().toISOString(), to, reason: `resolved by ${evidence.kind}`, evidence });
	}

	/** Record that the substrate's stored result for this id was fetched. Does not change state. */
	recordProbe(commandId: string, probe: { response?: DaemonResponse; detail?: string }): MutationRecord {
		const record = this.require(commandId);
		const updated: MutationRecord = { ...record, probes: [...(record.probes ?? []), { at: new Date().toISOString(), ...probe }] };
		writeAtomic(this.#path(commandId), `${JSON.stringify(updated, null, 2)}\n`);
		return updated;
	}
}
