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
 *   DISPATCHED -> UNCERTAIN   (anything else, including the dispatching
 *                              Governor process being proven over)
 *   UNCERTAIN  -> COMPLETED   (resolveUncertain with effect_observed)
 *   UNCERTAIN  -> FAILED      (resolveUncertain with effect_absent_proven)
 *
 * There is no transition out of UNCERTAIN without evidence, no transition
 * back to DISPATCHED, and no operation that mints a replacement command id
 * for an existing record. Re-dispatch of an uncertain mutation is a human
 * decision expressed as a NEW command with its own record and an explicit
 * `supersedes` link, never an automatic one.
 *
 * Two facts every record carries so that the crash window the ledger exists
 * for cannot swallow an obligation:
 *
 * - **Who dispatched it.** `dispatchedBy` is the Governor process's
 *   `(ownerToken, pid, processStartId)`. A record left DISPATCHED by a
 *   process that is proven over (`gone`, or its pid `replaced` by another
 *   process) is ADOPTED as UNCERTAIN by {@link MutationLedger.adoptAbandoned},
 *   which the attention surface runs first. A record whose dispatcher is
 *   `current` is in flight in a live Governor sharing this state directory
 *   and is left alone; one whose dispatcher is `unknown` is reported, never
 *   adopted, because adopting a live dispatcher's record would make its own
 *   completion an illegal transition.
 * - **What was dispatched.** `commandDigest` is the canonical digest of the
 *   complete wire command, and `command` is the command itself less any
 *   field that carries environment values (`withheld` names them; env values
 *   never touch the ledger). A probe of the record must re-present a command
 *   with the same digest, or it is refused before any I/O: if Prime never
 *   received the original, it will run whatever the probe carries.
 */

import { randomUUID } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { mkdirDurable, writeFileDurable } from "../fs/durable.ts";
import type { DaemonCommand, DaemonResponse } from "../prime/protocol.ts";
import { classifyProcessIdentity, LIVE_PROBE, type ProcessIdentity, type ProcessIdentityVerdict, type ProcessProbe, identityProvesProcessOver } from "../process/identity.ts";
import type { PreEffectProof, UncertainReason } from "./classify.ts";
import { commandDigest } from "./digest.ts";

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
	/** For an adoption: the verdict on the dispatching process and who adopted. */
	readonly adoption?: { readonly dispatcher: DispatcherIdentity; readonly verdict: ProcessIdentityVerdict; readonly adoptedBy: DispatcherIdentity };
}

/** The Governor process that wrote a record, in the same terms as a recovery lease holder. */
export interface DispatcherIdentity extends ProcessIdentity {
	readonly ownerToken: string;
}

/** Field names that carry environment values, at any depth, and are never stored in a record. */
export const WITHHELD_COMMAND_FIELDS: readonly string[] = ["launchEnv", "env"];

export interface MutationRecord {
	readonly schemaVersion: 2;
	readonly commandId: string;
	readonly clientId: string;
	readonly commandType: string;
	/** The wire command less `withheld` fields. Complete when `withheld` is empty. */
	readonly command: DaemonCommand;
	/** Dotted paths of fields removed from `command` before storage. */
	readonly withheld: readonly string[];
	/** `sha256:` digest of the canonical JSON of the COMPLETE wire command. */
	readonly commandDigest: string;
	readonly sessionId: string;
	readonly activeSessionId: string;
	readonly incarnationIndex: number;
	readonly dispatchedBy: DispatcherIdentity;
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

/** What {@link MutationLedger.adoptAbandoned} did and did not do, for the record. */
export interface AdoptionReport {
	/** DISPATCHED records whose dispatcher was proven over; now UNCERTAIN. */
	readonly adopted: readonly MutationRecord[];
	/** DISPATCHED records whose dispatcher is alive (`current`), including this process's own. Left alone. */
	readonly inFlight: readonly MutationRecord[];
	/** DISPATCHED records whose dispatcher cannot be classified. Left alone and reported: an operator decides. */
	readonly undecidable: readonly { readonly record: MutationRecord; readonly verdict: ProcessIdentityVerdict }[];
}

export interface MutationLedgerOptions {
	/** How dispatcher processes are inspected. Injectable so the suite can fabricate pid reuse. */
	readonly processProbe?: ProcessProbe;
	/** This process's identity, as written into the records it dispatches and the adoptions it makes. */
	readonly self?: ProcessIdentity;
	/** This Governor instance's owner token. */
	readonly ownerToken?: string;
}

/**
 * Every record write is durable through `writeFileDurable`: temp, fsync,
 * rename, fsync of the containing directory. The ledger is the authority a
 * later Governor consults about what may have been sent, so a directory
 * entry lost to power failure would be a lost authority, not a lost cache.
 */
function writeAtomic(path: string, contents: string): void {
	writeFileDurable(path, contents, { mode: 0o600 });
}

/**
 * `value` with every environment-bearing field removed at any depth; the
 * dotted paths of what was removed are appended to `withheld`.
 */
function withholdEnv(value: unknown, path: string, withheld: string[]): unknown {
	if (Array.isArray(value)) return value.map((item, index) => withholdEnv(item, `${path}[${index}]`, withheld));
	if (typeof value !== "object" || value === null) return value;
	const out: Record<string, unknown> = {};
	for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
		const here = path === "" ? key : `${path}.${key}`;
		if (WITHHELD_COMMAND_FIELDS.includes(key)) {
			withheld.push(here);
			continue;
		}
		out[key] = withholdEnv(item, here, withheld);
	}
	return out;
}

/** The command as stored: complete unless it carries environment values somewhere. */
function storableCommand(command: DaemonCommand): { command: DaemonCommand; withheld: string[] } {
	const withheld: string[] = [];
	const stored = withholdEnv(command, "", withheld) as DaemonCommand;
	return { command: stored, withheld };
}

export class MutationLedger {
	readonly dir: string;
	readonly #probe: ProcessProbe;
	readonly #self: DispatcherIdentity;

	constructor(stateDir: string, options: MutationLedgerOptions = {}) {
		this.dir = join(stateDir, "mutations");
		mkdirDurable(this.dir, { mode: 0o700 });
		this.#probe = options.processProbe ?? LIVE_PROBE;
		const self = options.self ?? { pid: process.pid };
		// The default token carries randomness: a token derived from the pid
		// alone would let a successor that recycled a dead dispatcher's pid call
		// that dispatcher's record its own and never inspect it.
		this.#self = { ...self, ownerToken: options.ownerToken ?? `pid#${self.pid}#${randomUUID().slice(0, 8)}` };
	}

	/** The identity this ledger writes as dispatcher. */
	get self(): DispatcherIdentity {
		return this.#self;
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
	 * Records that need a human: UNCERTAIN, oldest first. Abandoned DISPATCHED
	 * records are adopted first, so a Governor that died inside its crash
	 * window cannot make an obligation disappear from this list.
	 */
	awaitingReconciliation(): MutationRecord[] {
		this.adoptAbandoned();
		return this.list().filter((record) => record.state === "UNCERTAIN");
	}

	/**
	 * Every DISPATCHED record whose dispatching process is proven over becomes
	 * UNCERTAIN (`dispatcher_lost`). The verdict is the same one the recovery
	 * lease uses: `gone` or `replaced` adopts; `current` is a live owner and is
	 * fenced; `unknown` is reported and left. A record dispatched by THIS
	 * ledger's owner token is in flight here and never inspected.
	 */
	adoptAbandoned(): AdoptionReport {
		const adopted: MutationRecord[] = [];
		const inFlight: MutationRecord[] = [];
		const undecidable: { record: MutationRecord; verdict: ProcessIdentityVerdict }[] = [];
		for (const record of this.list()) {
			if (record.state !== "DISPATCHED") continue;
			const dispatcher = record.dispatchedBy;
			if (dispatcher === undefined || typeof dispatcher.pid !== "number") {
				undecidable.push({ record, verdict: "unknown" });
				continue;
			}
			if (dispatcher.ownerToken === this.#self.ownerToken) {
				inFlight.push(record);
				continue;
			}
			const verdict = classifyProcessIdentity(dispatcher, this.#probe);
			if (verdict === "current") {
				inFlight.push(record);
				continue;
			}
			if (!identityProvesProcessOver(verdict)) {
				undecidable.push({ record, verdict });
				continue;
			}
			// Re-read under the transition: another adopter may have got here first, in
			// which case the state is no longer DISPATCHED and the transition is refused.
			try {
				adopted.push(
					this.#transition(record.commandId, ["DISPATCHED"], {
						at: new Date().toISOString(),
						to: "UNCERTAIN",
						reason: `dispatcher_lost: the dispatching Governor process (pid ${dispatcher.pid}) is ${verdict}; the command may or may not have reached the substrate`,
						uncertainReason: "dispatcher_lost",
						adoption: { dispatcher, verdict, adoptedBy: this.#self },
					}),
				);
			} catch (error) {
				if (!(error instanceof MutationLedgerError) || error.code !== "illegal_transition") throw error;
			}
		}
		return { adopted, inFlight, undecidable };
	}

	/**
	 * Durably record intent. Must be called, and must return, before the
	 * envelope is written to the socket. Refuses an id that already exists:
	 * a command id is dispatched once, ever. The complete wire command is
	 * digested; environment-bearing fields are withheld from the stored copy.
	 */
	recordDispatch(input: {
		commandId: string;
		clientId: string;
		command: DaemonCommand;
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
		const stored = storableCommand(input.command);
		const record: MutationRecord = {
			schemaVersion: 2,
			commandId: input.commandId,
			clientId: input.clientId,
			commandType: input.command.type,
			command: stored.command,
			withheld: stored.withheld,
			commandDigest: commandDigest(input.command),
			sessionId: input.sessionId,
			activeSessionId: input.activeSessionId,
			incarnationIndex: input.incarnationIndex,
			dispatchedBy: this.#self,
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
		return this.#transition(commandId, ["DISPATCHED"], { at: new Date().toISOString(), to: "FAILED", reason: `typed pre-effect rejection ${proof.commandType} + ${proof.code}`, proof, response });
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
