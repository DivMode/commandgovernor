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
 * **Storage is a compare-and-swap** (`governor/fs/versioned.ts`): a record
 * is a directory of immutable versions and every write publishes the next
 * version exclusively, re-deriving against whatever is current when it
 * lands. Several Governors may share a state directory; none can regress a
 * record or lose another's write.
 *
 * **Superseding is a claim on the OLD record.** "Is O still UNCERTAIN?"
 * followed by "create R" is a check-then-act across two records, and the
 * CAS on each record alone does not serialise it: O could be resolved in
 * between, or two Governors could both pass the check and both send a
 * replacement. So `recordDispatch({ supersedes: O })` first writes
 * `supersededBy: R` onto O by compare-and-swap, which requires O to be
 * UNCERTAIN and unclaimed at the moment the write lands; only then is R's
 * record created and only then may R be sent. A resolution that lands
 * first makes the claim fail; a claim that lands first makes the second
 * claim fail. Exact evidence about O may still resolve it afterwards (the
 * claim is on the record for the human who reads it). A claim whose
 * claimant died before creating R is released by {@link adoptAbandoned}
 * under the same process-identity fence adoption uses.
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
import { join } from "node:path";

import { NO_CHANGE, VersionStore, VersionStoreError, type VersionStoreHooks } from "../fs/versioned.ts";
import type { DaemonCommand, DaemonResponse } from "../prime/protocol.ts";
import { classifyProcessIdentity, LIVE_PROBE, type ProcessIdentity, type ProcessIdentityVerdict, type ProcessProbe, identityProvesProcessOver } from "../process/identity.ts";
import type { PreEffectProof, UncertainReason, Verdict } from "./classify.ts";
import { commandDigest } from "./digest.ts";

export { MAX_CAS_ATTEMPTS } from "../fs/versioned.ts";

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
	/** For a supersede claim taken or released. */
	readonly claim?: { readonly action: "taken" | "released"; readonly replacement: string; readonly by: DispatcherIdentity; readonly verdict?: ProcessIdentityVerdict };
}

/** The Governor process that wrote a record, in the same terms as a recovery lease holder. */
export interface DispatcherIdentity extends ProcessIdentity {
	readonly ownerToken: string;
}

/** The durable claim that a replacement command is being dispatched for an uncertain record. */
export interface SupersedeClaim {
	/** The replacement command id. */
	readonly commandId: string;
	readonly claimedBy: DispatcherIdentity;
	readonly claimedAt: string;
}

/** Field names that carry environment values, at any depth, and are never stored in a record. */
export const WITHHELD_COMMAND_FIELDS: readonly string[] = ["launchEnv", "env"];

export interface MutationRecord {
	readonly schemaVersion: 2;
	readonly commandId: string;
	/** The version this snapshot is; the file name `v<version>.json` is the authority. */
	readonly version: number;
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
	/** The replacement claimed for this uncertain record; at most one, ever, unless released after its claimant died. */
	readonly supersededBy?: SupersedeClaim;
	/** Attempts to fetch the substrate's stored result for this id, for the record. */
	readonly probes?: readonly { readonly at: string; readonly response?: DaemonResponse; readonly detail?: string }[];
}

export type MutationLedgerErrorCode =
	| "duplicate_command_id"
	| "illegal_transition"
	| "unknown_command"
	| "supersedes_not_uncertain"
	| "already_superseded"
	| "contended"
	| "corrupt_history";

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
	/** Supersede claims whose replacement was never created and whose claimant is proven over; released. */
	readonly releasedClaims: readonly MutationRecord[];
	/** Supersede claims whose replacement was never created but whose claimant is alive or cannot be told. Left. */
	readonly pendingClaims: readonly { readonly record: MutationRecord; readonly verdict: ProcessIdentityVerdict }[];
	/** Entries under `mutations/` that are not record directories. Ignored by every listing and reported here. */
	readonly strays: readonly string[];
}

/**
 * Test seams. `beforeCommit` runs after a write has read the current
 * version and derived the next, and before it tries to publish, which is
 * exactly where a concurrent writer can win. `afterClaim` runs after a
 * supersede claim has been published on the old record and before the
 * replacement record is created, which is where a Governor can die.
 * Production callers pass none.
 */
export interface MutationLedgerHooks extends VersionStoreHooks {
	readonly afterClaim?: (superseded: string, replacement: string) => void;
}

export interface MutationLedgerOptions {
	/** How dispatcher processes are inspected. Injectable so the suite can fabricate pid reuse. */
	readonly processProbe?: ProcessProbe;
	/** This process's identity, as written into the records it dispatches and the adoptions it makes. */
	readonly self?: ProcessIdentity;
	/** This Governor instance's owner token. */
	readonly ownerToken?: string;
	readonly hooks?: MutationLedgerHooks;
	/** Attempts a write makes before reporting `contended`; defaults to the store's bound. Tests lower it. */
	readonly maxAttempts?: number;
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

/** The store's errors in the ledger's vocabulary. */
function translate(error: unknown): never {
	if (error instanceof VersionStoreError) {
		switch (error.code) {
			case "unknown_record":
				throw new MutationLedgerError("unknown_command", `no ledger record for ${error.id}`);
			case "duplicate_record":
				throw new MutationLedgerError("duplicate_command_id", `command id ${error.id} was already dispatched; a command id is never reused`);
			case "contended":
				throw new MutationLedgerError("contended", error.message);
			case "corrupt_history":
				throw new MutationLedgerError("corrupt_history", error.message);
			case "bad_id":
				throw new Error(`refusing to use ${JSON.stringify(error.id)} as a file name`);
		}
	}
	throw error;
}

export class MutationLedger {
	readonly dir: string;
	readonly #store: VersionStore<MutationRecord>;
	readonly #probe: ProcessProbe;
	readonly #self: DispatcherIdentity;
	readonly #hooks: MutationLedgerHooks;

	constructor(stateDir: string, options: MutationLedgerOptions = {}) {
		this.dir = join(stateDir, "mutations");
		this.#hooks = options.hooks ?? {};
		this.#store = new VersionStore<MutationRecord>(this.dir, {
			hooks: this.#hooks,
			...(options.maxAttempts !== undefined ? { maxAttempts: options.maxAttempts } : {}),
		});
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

	/** The path of the current version of `commandId`; for operators and tests. */
	currentVersionPath(commandId: string): string {
		try {
			return this.#store.currentVersionPath(commandId);
		} catch (error) {
			return translate(error);
		}
	}

	get(commandId: string): MutationRecord | undefined {
		try {
			return this.#store.get(commandId);
		} catch (error) {
			return translate(error);
		}
	}

	require(commandId: string): MutationRecord {
		try {
			return this.#store.require(commandId);
		} catch (error) {
			return translate(error);
		}
	}

	/** Every version of `commandId`, oldest first: the record's history. */
	history(commandId: string): MutationRecord[] {
		try {
			return this.#store.history(commandId);
		} catch (error) {
			return translate(error);
		}
	}

	/** Entries under `mutations/` that are not record directories; never read, always reported. */
	strays(): string[] {
		return this.#store.entries().strays;
	}

	list(): MutationRecord[] {
		return this.#store.list().sort((a, b) => a.dispatchedAt.localeCompare(b.dispatchedAt) || a.commandId.localeCompare(b.commandId));
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
	 *
	 * Likewise a supersede claim whose replacement record was never created
	 * (the claimant died between the claim and the create) is released when
	 * the claimant is proven over, and reported otherwise.
	 */
	adoptAbandoned(): AdoptionReport {
		const adopted: MutationRecord[] = [];
		const inFlight: MutationRecord[] = [];
		const undecidable: { record: MutationRecord; verdict: ProcessIdentityVerdict }[] = [];
		const releasedClaims: MutationRecord[] = [];
		const pendingClaims: { record: MutationRecord; verdict: ProcessIdentityVerdict }[] = [];
		const strays = this.strays();
		for (const record of this.list()) {
			if (record.state === "UNCERTAIN" && record.supersededBy) {
				const claim = record.supersededBy;
				const replacement = this.get(claim.commandId);
				if (replacement !== undefined && replacement.supersedes === record.commandId) continue; // the replacement exists: the claim is doing its job
				const verdict = claim.claimedBy.ownerToken === this.#self.ownerToken ? "current" : classifyProcessIdentity(claim.claimedBy, this.#probe);
				if (!identityProvesProcessOver(verdict)) {
					pendingClaims.push({ record, verdict });
					continue;
				}
				releasedClaims.push(
					this.#update(record.commandId, (current) => {
						if (current.state !== "UNCERTAIN" || !current.supersededBy || current.supersededBy.commandId !== claim.commandId) return NO_CHANGE;
						const { supersededBy: _released, ...rest } = current;
						return {
							...rest,
							transitions: [
								...current.transitions,
								{
									at: new Date().toISOString(),
									to: "UNCERTAIN",
									reason: `supersede claim by ${claim.commandId} released: the claimant (pid ${claim.claimedBy.pid}) is ${verdict} and never created the replacement`,
									claim: { action: "released", replacement: claim.commandId, by: this.#self, verdict },
								},
							],
						};
					}),
				);
				continue;
			}
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
			// The transition is a compare-and-swap from the DISPATCHED version: if
			// another adopter (or the dispatcher's own late result) published a
			// newer version first, this one is refused and nothing is written.
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
		return { adopted, inFlight, undecidable, releasedClaims, pendingClaims, strays };
	}

	/**
	 * Durably record intent. Must be called, and must return, before the
	 * envelope is written to the socket. Refuses an id that already exists:
	 * a command id is dispatched once, ever. The complete wire command is
	 * digested; environment-bearing fields are withheld from the stored copy.
	 *
	 * With `supersedes`, the claim on the old record is taken FIRST, by
	 * compare-and-swap, and the replacement is created only if it succeeds.
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
			this.#claim(input.supersedes, input.commandId);
			this.#hooks.afterClaim?.(input.supersedes, input.commandId);
		}
		const now = new Date().toISOString();
		const stored = storableCommand(input.command);
		const record: Omit<MutationRecord, "version"> = {
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
			transitions: [{ at: now, to: "DISPATCHED", reason: input.supersedes !== undefined ? `intent recorded before send; supersedes ${input.supersedes}` : "intent recorded before send" }],
			...(input.supersedes !== undefined ? { supersedes: input.supersedes } : {}),
		};
		try {
			return this.#store.create(input.commandId, record);
		} catch (error) {
			return translate(error);
		}
	}

	/**
	 * The serialisation point of a supersede: `supersededBy` is written onto
	 * the OLD record by compare-and-swap, and the write is refused unless the
	 * record is UNCERTAIN and unclaimed at the moment it lands.
	 */
	#claim(superseded: string, replacement: string): MutationRecord {
		return this.#update(superseded, (current) => {
			if (current.state !== "UNCERTAIN") {
				throw new MutationLedgerError("supersedes_not_uncertain", `${superseded} is ${current.state}, not UNCERTAIN; only an uncertain mutation may be superseded`);
			}
			if (current.supersededBy !== undefined) {
				throw new MutationLedgerError("already_superseded", `${superseded} is already superseded by ${current.supersededBy.commandId} (claimed by ${current.supersededBy.claimedBy.ownerToken}); a second replacement is refused`);
			}
			const claimedAt = new Date().toISOString();
			return {
				...current,
				supersededBy: { commandId: replacement, claimedBy: this.#self, claimedAt },
				transitions: [...current.transitions, { at: claimedAt, to: "UNCERTAIN", reason: `supersede claim taken by ${replacement}`, claim: { action: "taken", replacement, by: this.#self } }],
			};
		});
	}

	#update(commandId: string, derive: (current: MutationRecord) => Omit<MutationRecord, "version"> | typeof NO_CHANGE): MutationRecord {
		try {
			return this.#store.update(commandId, derive);
		} catch (error) {
			return translate(error);
		}
	}

	#transition(commandId: string, from: readonly MutationState[], transition: MutationTransition): MutationRecord {
		return this.#update(commandId, (current) => {
			if (!from.includes(current.state)) {
				throw new MutationLedgerError("illegal_transition", `${commandId}: ${current.state} -> ${transition.to} is not a legal transition`);
			}
			return { ...current, state: transition.to, transitions: [...current.transitions, transition] };
		});
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

	/**
	 * Record the dispatcher's own outcome for a command it sent. Normally a
	 * DISPATCHED transition. If an adopter got there first (the record is
	 * UNCERTAIN now), the outcome is not discarded: a success response IS
	 * exact evidence that the effect happened, a typed pre-effect rejection IS
	 * exact evidence that it did not, and an uncertain outcome is appended as
	 * a probe. Anything else is the caller's error and propagates.
	 */
	recordOutcome(commandId: string, verdict: Verdict): MutationRecord {
		try {
			switch (verdict.verdict) {
				case "completed":
					return this.markCompleted(commandId, verdict.response);
				case "failed":
					return this.markFailed(commandId, verdict.proof, verdict.response);
				case "uncertain":
					return this.markUncertain(commandId, verdict.reason, verdict.response, verdict.detail);
			}
		} catch (error) {
			if (!(error instanceof MutationLedgerError) || error.code !== "illegal_transition" || this.require(commandId).state !== "UNCERTAIN") throw error;
		}
		const observedAt = new Date().toISOString();
		switch (verdict.verdict) {
			case "completed":
				this.recordProbe(commandId, { response: verdict.response, detail: "the dispatcher's own response, after adoption" });
				return this.resolveUncertain(commandId, { kind: "effect_observed", by: "dispatcher's own response", detail: "the substrate returned success to the dispatcher after an adopter marked the record uncertain", observedAt });
			case "failed":
				this.recordProbe(commandId, { response: verdict.response, detail: "the dispatcher's own response, after adoption" });
				return this.resolveUncertain(commandId, { kind: "effect_absent_proven", by: "dispatcher's own response", detail: `typed pre-effect rejection ${verdict.proof.commandType} + ${verdict.proof.code}, returned to the dispatcher after an adopter marked the record uncertain`, observedAt });
			case "uncertain":
				return this.recordProbe(commandId, { ...(verdict.response ? { response: verdict.response } : {}), detail: `the dispatcher's own outcome after adoption: ${verdict.reason}${verdict.detail ? ` (${verdict.detail})` : ""}` });
		}
	}

	/**
	 * The only way out of UNCERTAIN: exact evidence about the external effect.
	 * A record that carries a supersede claim may still be resolved: the
	 * evidence is about the original command, and the claim stays on the
	 * record for whoever reads it.
	 */
	resolveUncertain(commandId: string, evidence: ResolutionEvidence): MutationRecord {
		const to: MutationState = evidence.kind === "effect_observed" ? "COMPLETED" : "FAILED";
		return this.#transition(commandId, ["UNCERTAIN"], { at: new Date().toISOString(), to, reason: `resolved by ${evidence.kind}`, evidence });
	}

	/**
	 * Record that the substrate's stored result for this id was fetched. Does
	 * not change state, and is applied on whatever the current version is when
	 * it lands: a probe written while another Governor resolved the record is
	 * appended to the resolved record, never over it.
	 */
	recordProbe(commandId: string, probe: { response?: DaemonResponse; detail?: string }): MutationRecord {
		return this.#update(commandId, (current) => ({ ...current, probes: [...(current.probes ?? []), { at: new Date().toISOString(), ...probe }] }));
	}
}
