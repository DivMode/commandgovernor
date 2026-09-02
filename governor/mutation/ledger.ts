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
 * **Storage is a compare-and-swap, not a rename.** Several Governors may
 * share a state directory, and a read-modify-rename would let a stale
 * writer put an old snapshot over a newer one: exact evidence that resolved
 * a record could vanish and the mutation become uncertain, and supersedable,
 * again. So a record is a directory `<commandId>/` of immutable versions
 * `v1.json, v2.json, ...`, and every write is "read the highest version N,
 * derive the next state, publish `v(N+1).json` with an exclusive
 * (`link(2)`) create". If `v(N+1)` already exists, another writer got there
 * first; this one re-reads and re-applies against the new state, where a
 * transition that is no longer legal is refused. Nothing is ever renamed
 * over, unlinked or locked, so there is no stale lock to reclaim and no
 * partial file to observe: every version is complete and fsynced before its
 * name exists. The highest version is the record; older ones are its
 * history, kept.
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

import { createFileExclusiveDurable, mkdirDurable } from "../fs/durable.ts";
import type { DaemonCommand, DaemonResponse } from "../prime/protocol.ts";
import { classifyProcessIdentity, LIVE_PROBE, type ProcessIdentity, type ProcessIdentityVerdict, type ProcessProbe, identityProvesProcessOver } from "../process/identity.ts";
import type { PreEffectProof, UncertainReason, Verdict } from "./classify.ts";
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
	/** Attempts to fetch the substrate's stored result for this id, for the record. */
	readonly probes?: readonly { readonly at: string; readonly response?: DaemonResponse; readonly detail?: string }[];
}

export type MutationLedgerErrorCode = "duplicate_command_id" | "illegal_transition" | "unknown_command" | "supersedes_not_uncertain" | "contended" | "corrupt_history";

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
	/** Entries under `mutations/` that are not record directories. Ignored by every listing and reported here. */
	readonly strays: readonly string[];
}

/**
 * Test seams. `beforeCommit` runs after a write has read the current
 * version and derived the next, and before it tries to publish, which is
 * exactly where a concurrent writer can win. Production callers pass none.
 */
export interface MutationLedgerHooks {
	readonly beforeCommit?: (commandId: string, fromVersion: number) => void;
}

export interface MutationLedgerOptions {
	/** How dispatcher processes are inspected. Injectable so the suite can fabricate pid reuse. */
	readonly processProbe?: ProcessProbe;
	/** This process's identity, as written into the records it dispatches and the adoptions it makes. */
	readonly self?: ProcessIdentity;
	/** This Governor instance's owner token. */
	readonly ownerToken?: string;
	readonly hooks?: MutationLedgerHooks;
	/** Attempts a write makes before reporting `contended`; defaults to {@link MAX_CAS_ATTEMPTS}. Tests lower it. */
	readonly maxAttempts?: number;
}

/**
 * Attempts a write makes against concurrent writers before it reports
 * contention. Every lost attempt means another writer made progress, so
 * exhaustion needs that many OTHER writes to land on one record while this
 * one keeps losing; with the backoff below that took more than 32 processes
 * hammering one record to approach at the previous limit of 64.
 */
export const MAX_CAS_ATTEMPTS = 1024;

/** Upper bound of the jittered pause after a lost attempt, in milliseconds. */
const CAS_BACKOFF_MAX_MS = 25;

const VERSION_FILE = /^v([1-9]\d*)\.json$/;
const COMMAND_ID = /^[A-Za-z0-9._:-]+$/;

const sleeper = new Int32Array(new SharedArrayBuffer(4));

/** Synchronous, jittered pause; the write path is synchronous on purpose (see durable.ts). */
function backoff(attempt: number): void {
	const cap = Math.min(CAS_BACKOFF_MAX_MS, 1 + attempt);
	const ms = Math.random() * cap;
	if (ms >= 0.5) Atomics.wait(sleeper, 0, 0, ms);
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

function serialise(record: MutationRecord): string {
	return `${JSON.stringify(record, null, 2)}\n`;
}

export class MutationLedger {
	readonly dir: string;
	readonly #probe: ProcessProbe;
	readonly #self: DispatcherIdentity;
	readonly #hooks: MutationLedgerHooks;
	readonly #maxAttempts: number;

	constructor(stateDir: string, options: MutationLedgerOptions = {}) {
		this.dir = join(stateDir, "mutations");
		mkdirDurable(this.dir, { mode: 0o700 });
		this.#probe = options.processProbe ?? LIVE_PROBE;
		const self = options.self ?? { pid: process.pid };
		// The default token carries randomness: a token derived from the pid
		// alone would let a successor that recycled a dead dispatcher's pid call
		// that dispatcher's record its own and never inspect it.
		this.#self = { ...self, ownerToken: options.ownerToken ?? `pid#${self.pid}#${randomUUID().slice(0, 8)}` };
		this.#hooks = options.hooks ?? {};
		this.#maxAttempts = options.maxAttempts ?? MAX_CAS_ATTEMPTS;
	}

	/** The identity this ledger writes as dispatcher. */
	get self(): DispatcherIdentity {
		return this.#self;
	}

	#recordDir(commandId: string): string {
		if (!COMMAND_ID.test(commandId)) throw new Error(`refusing to use ${JSON.stringify(commandId)} as a file name`);
		return join(this.dir, commandId);
	}

	/** Record directory names under `mutations/`, and the entries that are not. */
	#entries(): { ids: string[]; strays: string[] } {
		const ids: string[] = [];
		const strays: string[] = [];
		for (const entry of readdirSync(this.dir, { withFileTypes: true })) {
			if (entry.isDirectory() && COMMAND_ID.test(entry.name)) ids.push(entry.name);
			else strays.push(entry.name);
		}
		return { ids, strays };
	}

	/** Entries under `mutations/` that are not record directories; never read, always reported. */
	strays(): string[] {
		return this.#entries().strays;
	}

	#versionPath(commandId: string, version: number): string {
		return join(this.#recordDir(commandId), `v${version}.json`);
	}

	/** The highest version on disk for `commandId`, or undefined for no record. */
	#current(commandId: string): { record: MutationRecord; version: number } | undefined {
		let names: string[];
		try {
			names = readdirSync(this.#recordDir(commandId));
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
			throw error;
		}
		let version = 0;
		for (const name of names) {
			const match = VERSION_FILE.exec(name);
			if (match) version = Math.max(version, Number(match[1]));
		}
		if (version === 0) return undefined; // a directory with no published version yet (a creator died before its link)
		// A version is published complete (link of an fsynced file); it is never partial.
		const parsed = JSON.parse(readFileSync(this.#versionPath(commandId, version), "utf8")) as MutationRecord;
		return { record: { ...parsed, version }, version };
	}

	/** The path of the current version of `commandId`; for operators and tests. */
	currentVersionPath(commandId: string): string {
		const current = this.#current(commandId);
		if (!current) throw new MutationLedgerError("unknown_command", `no ledger record for ${commandId}`);
		return this.#versionPath(commandId, current.version);
	}

	get(commandId: string): MutationRecord | undefined {
		return this.#current(commandId)?.record;
	}

	require(commandId: string): MutationRecord {
		const record = this.get(commandId);
		if (!record) throw new MutationLedgerError("unknown_command", `no ledger record for ${commandId}`);
		return record;
	}

	/** Every version of `commandId`, oldest first: the record's history. */
	history(commandId: string): MutationRecord[] {
		const current = this.#current(commandId);
		if (!current) throw new MutationLedgerError("unknown_command", `no ledger record for ${commandId}`);
		const versions: MutationRecord[] = [];
		for (let version = 1; version <= current.version; version += 1) {
			let raw: string;
			try {
				raw = readFileSync(this.#versionPath(commandId, version), "utf8");
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
				// Nothing in the Governor removes a version; a gap is damage, and is named as such.
				throw new MutationLedgerError("corrupt_history", `${commandId}: version ${version} is missing although version ${current.version} exists`);
			}
			versions.push({ ...(JSON.parse(raw) as MutationRecord), version });
		}
		return versions;
	}

	list(): MutationRecord[] {
		return this.#entries()
			.ids.map((id) => this.get(id))
			.filter((record): record is MutationRecord => record !== undefined)
			.sort((a, b) => a.dispatchedAt.localeCompare(b.dispatchedAt) || a.commandId.localeCompare(b.commandId));
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
		const strays = this.strays();
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
		return { adopted, inFlight, undecidable, strays };
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
			version: 1,
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
		mkdirDurable(this.#recordDir(input.commandId), { mode: 0o700 });
		// Version 1 is an exclusive create too: two dispatchers of one id cannot both succeed.
		const created = createFileExclusiveDurable(this.#versionPath(input.commandId, 1), serialise(record), { mode: 0o600 });
		if (created.outcome !== "created") {
			throw new MutationLedgerError("duplicate_command_id", `command id ${input.commandId} was dispatched concurrently by another writer; a command id is never reused`);
		}
		return record;
	}

	/**
	 * The compare-and-swap every write goes through: read the current version,
	 * check `from` against its state, derive the next record, publish it as the
	 * next version with an exclusive create. A version that appears in between
	 * means another writer won; re-read and re-apply against the new state.
	 */
	#update(commandId: string, from: readonly MutationState[] | undefined, target: MutationState | undefined, derive: (current: MutationRecord) => Omit<MutationRecord, "version">): MutationRecord {
		for (let attempt = 0; attempt < this.#maxAttempts; attempt += 1) {
			const current = this.#current(commandId);
			if (!current) throw new MutationLedgerError("unknown_command", `no ledger record for ${commandId}`);
			if (from && !from.includes(current.record.state)) {
				throw new MutationLedgerError("illegal_transition", `${commandId}: ${current.record.state} -> ${target ?? "?"} is not a legal transition`);
			}
			const next: MutationRecord = { ...derive(current.record), version: current.version + 1 };
			this.#hooks.beforeCommit?.(commandId, current.version);
			const published = createFileExclusiveDurable(this.#versionPath(commandId, next.version), serialise(next), { mode: 0o600 });
			if (published.outcome === "created") return next;
			// "exists": a concurrent writer published this version; "vanished"
			// cannot happen (versions are never removed) and is treated the same.
			backoff(attempt);
		}
		throw new MutationLedgerError("contended", `${commandId}: ${this.#maxAttempts} attempts each found a newer version; giving up without writing`);
	}

	#transition(commandId: string, from: readonly MutationState[], transition: MutationTransition): MutationRecord {
		return this.#update(commandId, from, transition.to, (current) => ({ ...current, state: transition.to, transitions: [...current.transitions, transition] }));
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

	/** The only way out of UNCERTAIN: exact evidence about the external effect. */
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
		return this.#update(commandId, undefined, undefined, (current) => ({ ...current, probes: [...(current.probes ?? []), { at: new Date().toISOString(), ...probe }] }));
	}
}
