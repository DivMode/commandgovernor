/**
 * The Command Governor Prime adaptation layer: the one object that speaks to
 * a pinned Prime supervisor on behalf of the Governor, and the only place
 * the three Issue #17 invariants are enforced.
 *
 * - D8: every session it creates has an explicit canonical `sessionPath`.
 * - D1: a resident root whose worker died is reopened exactly once, under a
 *       fenced recovery lease, on the same path, keeping Prime's `sessionId`
 *       and recording the new active-session id as an incarnation. Stale
 *       incarnations are refused before dispatch.
 * - D2: every mutating command is journaled DISPATCHED before it is sent,
 *       classified structurally afterwards, and never re-dispatched or
 *       re-identified automatically when the outcome is unknown.
 *
 * It owns no runtime. Prime's supervisor, workers, leases, journals and
 * sessions are used as they are; this class adds the durable Governor records
 * Prime lacks and the fences Prime's own semantics leave to the client.
 */

import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { DaemonClient, RequestTimeout, TransportLost, connectWithRetry } from "./prime/daemon-client.ts";
import { buildLaunchEnv, type LaunchEnvOptions } from "./prime/env.ts";
import {
	activeSessionIdOf,
	commandKind,
	type DaemonCommand,
	type DaemonResponse,
	isSessionSummary,
	type SessionSummary,
} from "./prime/protocol.ts";
import { expectedSubstrate } from "./prime/substrate.ts";
import { assertProductionPolicy, classifyMutationOutcome, type ClassificationPolicy, DEFAULT_POLICY, type Verdict } from "./mutation/classify.ts";
import { MutationLedger, type MutationRecord, type ResolutionEvidence } from "./mutation/ledger.ts";
import { canonicalSessionPath, type CanonicalSessionPath } from "./session/paths.ts";
import { type Incarnation, RecoveryLeaseHeld, type SessionRecord, SessionRegistry } from "./session/registry.ts";

export interface GovernorOptions {
	/** Durable Governor state: registry, ledger, client identity. */
	readonly stateDir: string;
	/** The Prime supervisor's public socket. */
	readonly socketPath: string;
	/** Prime agent directory the sessions live under. */
	readonly agentDir: string;
	/** HOME for workers the supervisor launches on the Governor's behalf. */
	readonly home: string;
	/** TMPDIR for those workers; must be the supervisor's own, since worker sockets live under it. */
	readonly tmpDir: string;
	/** The directory every session transcript must lie within. */
	readonly sessionDir: string;
	/** Working directory for created sessions. */
	readonly cwd: string;
	/** Provider/model for created sessions. */
	readonly provider: string;
	readonly model: string;
	/** Environment policy for the wire `launchEnv`. */
	readonly env?: LaunchEnvOptions;
	readonly sourceEnv?: Readonly<Record<string, string | undefined>>;
	/** Wire evidence log path; env values are never written to it. */
	readonly wireLog?: string;
	/** Injected only by the conformance suite's negative controls. */
	readonly classificationPolicy?: ClassificationPolicy;
	/** Disable the recovery lease. Exists ONLY so the suite can show the fence matters. */
	readonly unsafeDisableRecoveryFence?: boolean;
}

export interface CreateSessionSpec {
	/** Required. There is no default and no fallback. */
	readonly sessionPath: unknown;
	readonly name?: string;
	readonly lifecycle?: "resident" | "client_owned";
}

export interface CreatedSession {
	readonly record: SessionRecord;
	readonly summary: SessionSummary;
	/** Names of env vars withheld from the wire, for evidence. */
	readonly withheldEnv: readonly string[];
}

export type RecoveryOutcome =
	| { readonly action: "healthy"; readonly incarnation: Incarnation }
	| { readonly action: "reopened"; readonly incarnation: Incarnation; readonly previous: Incarnation; readonly createCommandId: string }
	| { readonly action: "converged"; readonly incarnation: Incarnation; readonly previous: Incarnation }
	| { readonly action: "lease_held"; readonly holder: RecoveryLeaseHeld };

export interface DispatchResult {
	readonly record: MutationRecord;
	readonly verdict: Verdict;
}

export class SessionIdentityMismatch extends Error {
	readonly code = "session_identity_mismatch" as const;
	readonly expected: string;
	readonly actual: string;
	readonly sessionPath: string;
	constructor(expected: string, actual: string, sessionPath: string) {
		super(`reopening ${sessionPath} produced session ${actual}, not the logical session ${expected}; refusing to bind`);
		this.name = "SessionIdentityMismatch";
		this.expected = expected;
		this.actual = actual;
		this.sessionPath = sessionPath;
	}
}

export class NotRecoverable extends Error {
	readonly code = "not_recoverable" as const;
	readonly summary: SessionSummary | undefined;
	constructor(message: string, summary?: SessionSummary) {
		super(message);
		this.name = "NotRecoverable";
		this.summary = summary;
	}
}

function stableClientId(stateDir: string): string {
	const path = join(stateDir, "client-id");
	if (existsSync(path)) return readFileSync(path, "utf8").trim();
	const id = `cg:${randomUUID()}`;
	writeFileSync(path, `${id}\n`, { mode: 0o600 });
	return id;
}

function summaryOf(value: unknown, what: string): SessionSummary {
	if (!isSessionSummary(value)) throw new Error(`${what}: response data is not a session summary`);
	return value;
}

export class Governor {
	readonly stateDir: string;
	readonly clientId: string;
	/** Identifies this Governor process in leases and incarnation records. */
	readonly ownerToken: string;
	readonly registry: SessionRegistry;
	readonly ledger: MutationLedger;
	readonly options: GovernorOptions;
	#client: DaemonClient | undefined;
	readonly #policy: ClassificationPolicy;

	constructor(options: GovernorOptions) {
		mkdirSync(options.stateDir, { recursive: true, mode: 0o700 });
		this.options = options;
		this.stateDir = options.stateDir;
		this.clientId = stableClientId(options.stateDir);
		this.ownerToken = `${this.clientId}#${process.pid}#${randomUUID().slice(0, 8)}`;
		this.registry = new SessionRegistry(options.stateDir);
		this.ledger = new MutationLedger(options.stateDir);
		this.#policy = options.classificationPolicy ?? DEFAULT_POLICY;
		if (!options.classificationPolicy) assertProductionPolicy(this.#policy);
	}

	get client(): DaemonClient {
		if (!this.#client || this.#client.closed) throw new TransportLost("governor is not connected");
		return this.#client;
	}

	get connected(): boolean {
		return this.#client !== undefined && !this.#client.closed;
	}

	async connect(budgetMs = 20_000): Promise<DaemonClient> {
		if (this.#client && !this.#client.closed) return this.#client;
		this.#client = await connectWithRetry(
			this.options.socketPath,
			{ clientId: this.clientId, expected: expectedSubstrate(), wireLog: this.options.wireLog },
			budgetMs,
		);
		return this.#client;
	}

	close(): void {
		this.#client?.close();
		this.#client = undefined;
	}

	newCommandId(): string {
		return `cg-${randomUUID()}`;
	}

	#sessionConfig(): Record<string, unknown> {
		return {
			cwd: this.options.cwd,
			agentDir: this.options.agentDir,
			sessionDir: this.options.sessionDir,
			provider: this.options.provider,
			model: this.options.model,
			noExtensions: true,
			noSkills: true,
			noContextFiles: true,
			noPromptTemplates: true,
			noThemes: true,
			telemetryDisabled: true,
		};
	}

	#launchEnv(): ReturnType<typeof buildLaunchEnv> {
		return buildLaunchEnv(this.options.sourceEnv ?? process.env, {
			...this.options.env,
			overrides: {
				HOME: this.options.home,
				TMPDIR: this.options.tmpDir,
				PRIME_AGENT_CODING_AGENT_DIR: this.options.agentDir,
				PRIME_AGENT_TELEMETRY: "0",
				PRIME_AGENT_INSTALL_UV: "0",
				...this.options.env?.overrides,
			},
		});
	}

	/** A read-only command. Not journaled; a failure here proves nothing and changes nothing. */
	async read(command: DaemonCommand, timeoutMs = 60_000): Promise<DaemonResponse> {
		if (commandKind(command.type) !== "read") {
			throw new Error(`${command.type} is a mutating command; use dispatchMutation`);
		}
		return this.client.request(command, this.newCommandId(), timeoutMs);
	}

	async list(): Promise<SessionSummary[]> {
		const response = await this.read({ type: "list" });
		if (!response.success) throw new Error(`list failed: ${response.error}`);
		const data = response.data as { sessions?: unknown } | unknown[];
		const rows = Array.isArray(data) ? data : (data as { sessions?: unknown }).sessions;
		if (!Array.isArray(rows)) throw new Error("list: unexpected response shape");
		return rows.filter(isSessionSummary);
	}

	async findSummary(sessionId: string): Promise<SessionSummary | undefined> {
		return (await this.list()).find((summary) => summary.sessionId === sessionId);
	}

	// -------------------------------------------------------------------
	// D8: create
	// -------------------------------------------------------------------

	/**
	 * Create a session. Preflight refuses a missing, relative, non-canonical
	 * or out-of-tree `sessionPath` before anything is sent.
	 */
	async createSession(spec: CreateSessionSpec): Promise<CreatedSession> {
		const sessionPath = canonicalSessionPath(spec.sessionPath, this.options.sessionDir);
		const existing = this.registry.findByPath(sessionPath);
		if (existing) {
			throw new Error(`session path ${sessionPath} already belongs to session ${existing.sessionId}; reopen it instead of creating`);
		}
		const lifecycle = spec.lifecycle ?? "resident";
		const launch = this.#launchEnv();
		const commandId = this.newCommandId();
		const command: DaemonCommand = {
			type: "create",
			sessionPath,
			...(spec.name !== undefined ? { name: spec.name } : {}),
			...(lifecycle === "client_owned" ? { lifecycle } : {}),
			config: this.#sessionConfig(),
			launchEnv: launch.env,
		};
		// `create` is a mutation: journaled so a crash between send and record
		// leaves evidence, though a duplicate create on the same path converges
		// in Prime rather than duplicating the root.
		const dispatch = await this.#dispatch(command, commandId, { sessionId: `path:${sessionPath}`, activeSessionId: "-", incarnationIndex: -1 }, 120_000);
		if (dispatch.verdict.verdict !== "completed") {
			throw new Error(`create ${sessionPath}: ${dispatch.verdict.verdict}${"response" in dispatch.verdict && dispatch.verdict.response && !dispatch.verdict.response.success ? ` (${dispatch.verdict.response.error})` : ""}`);
		}
		const summary = summaryOf(dispatch.verdict.response.data, "create");
		const record = this.registry.create({
			sessionId: summary.sessionId,
			sessionPath,
			lifecycle,
			activeSessionId: activeSessionIdOf(summary),
			workerPid: summary.workerPid,
			openedBy: this.ownerToken,
		});
		return { record, summary, withheldEnv: launch.withheld };
	}

	// -------------------------------------------------------------------
	// D2: dispatch
	// -------------------------------------------------------------------

	/**
	 * Dispatch a mutating command to the current incarnation of `sessionId`.
	 *
	 * Refuses a stale `activeSessionId` before any I/O. Records DISPATCHED
	 * durably, sends, classifies, records the verdict. Never retries, never
	 * mints a second id for the same intent.
	 */
	async dispatchMutation(
		sessionId: string,
		activeSessionId: string,
		command: DaemonCommand,
		options: { timeoutMs?: number; supersedes?: string } = {},
	): Promise<DispatchResult> {
		if (commandKind(command.type) !== "mutating") {
			throw new Error(`${command.type} is a read-only command; use read`);
		}
		const incarnation = this.registry.assertCurrent(sessionId, activeSessionId);
		const commandId = this.newCommandId();
		return this.#dispatch(
			{ ...command, activeSessionId },
			commandId,
			{ sessionId, activeSessionId, incarnationIndex: incarnation.index, supersedes: options.supersedes },
			options.timeoutMs ?? 60_000,
		);
	}

	async #dispatch(
		command: DaemonCommand,
		commandId: string,
		identity: { sessionId: string; activeSessionId: string; incarnationIndex: number; supersedes?: string },
		timeoutMs: number,
	): Promise<DispatchResult> {
		const client = this.client;
		this.ledger.recordDispatch({
			commandId,
			clientId: this.clientId,
			commandType: command.type,
			...identity,
		});
		let verdict: Verdict;
		try {
			const response = await client.request(command, commandId, timeoutMs);
			verdict = classifyMutationOutcome({ kind: "response", response }, this.#policy);
		} catch (error) {
			if (error instanceof TransportLost) {
				verdict = classifyMutationOutcome({ kind: "transport_lost", detail: error.message }, this.#policy);
			} else if (error instanceof RequestTimeout) {
				verdict = classifyMutationOutcome({ kind: "timeout", timeoutMs }, this.#policy);
			} else {
				throw error;
			}
		}
		const record = this.#record(commandId, verdict);
		return { record, verdict };
	}

	#record(commandId: string, verdict: Verdict): MutationRecord {
		switch (verdict.verdict) {
			case "completed":
				return this.ledger.markCompleted(commandId, verdict.response);
			case "failed":
				return this.ledger.markFailed(commandId, verdict.proof, verdict.response);
			case "uncertain":
				return this.ledger.markUncertain(commandId, verdict.reason, verdict.response, verdict.detail);
		}
	}

	/**
	 * Fetch the substrate's stored result for an UNCERTAIN command by
	 * re-presenting the SAME `clientId + commandId`. Prime's journal answers a
	 * journaled id from its stored result without executing; the answer is
	 * classified with the same policy, so a stored untyped failure stays
	 * UNCERTAIN. Explicit, never automatic, and documented: if the supervisor
	 * never journaled the receipt, Prime treats the id as new work.
	 */
	async probeStoredResult(commandId: string, command: DaemonCommand, timeoutMs = 60_000): Promise<{ record: MutationRecord; verdict: Verdict }> {
		const record = this.ledger.require(commandId);
		if (record.state !== "UNCERTAIN") {
			throw new Error(`${commandId} is ${record.state}; only an UNCERTAIN command may be probed`);
		}
		let verdict: Verdict;
		try {
			const response = await this.client.request(command, commandId, timeoutMs);
			this.ledger.recordProbe(commandId, { response });
			verdict = classifyMutationOutcome({ kind: "response", response }, this.#policy);
		} catch (error) {
			if (error instanceof TransportLost || error instanceof RequestTimeout) {
				this.ledger.recordProbe(commandId, { detail: error.message });
				verdict = classifyMutationOutcome({ kind: error instanceof TransportLost ? "transport_lost" : "timeout", detail: error.message, timeoutMs }, this.#policy);
			} else {
				throw error;
			}
		}
		// A probe that comes back `completed` is a substrate-stored success:
		// exact evidence, resolvable without another dispatch.
		if (verdict.verdict === "completed") {
			return { record: this.ledger.resolveUncertain(commandId, { kind: "effect_observed", by: "substrate stored result", detail: "journal replayed a success response", observedAt: new Date().toISOString() }), verdict };
		}
		return { record: this.ledger.require(commandId), verdict };
	}

	/** Resolve an UNCERTAIN command with exact external evidence. No dispatch happens. */
	resolveUncertain(commandId: string, evidence: ResolutionEvidence): MutationRecord {
		return this.ledger.resolveUncertain(commandId, evidence);
	}

	// -------------------------------------------------------------------
	// D1: recovery
	// -------------------------------------------------------------------

	/**
	 * Recover the resident root for logical session `sessionId` if its worker
	 * has died. Lifecycle is read from `workerState` -- never from `activity`.
	 */
	async recoverResidentRoot(sessionId: string): Promise<RecoveryOutcome> {
		const record = this.registry.require(sessionId);
		const before = this.registry.current(sessionId);
		const summary = await this.findSummary(sessionId);
		if (!summary) {
			throw new NotRecoverable(`session ${sessionId} is not registered with the supervisor; nothing to recover from and nothing to reopen safely without operator review`);
		}
		const liveId = activeSessionIdOf(summary);
		if (summary.workerState === "ready" && liveId === before.activeSessionId) {
			return { action: "healthy", incarnation: before };
		}
		if (summary.workerState === "ready" && liveId !== before.activeSessionId) {
			const { incarnation } = this.registry.recordIncarnation({ sessionId, activeSessionId: liveId, workerPid: summary.workerPid, cause: "converged", openedBy: this.ownerToken });
			return { action: "converged", incarnation, previous: before };
		}
		if (summary.workerState !== "failed") {
			throw new NotRecoverable(`session ${sessionId} worker is ${String(summary.workerState)}; only a failed resident root is reopened`, summary);
		}
		let lease: ReturnType<SessionRegistry["acquireRecoveryLease"]> | undefined;
		if (!this.options.unsafeDisableRecoveryFence) {
			try {
				lease = this.registry.acquireRecoveryLease(sessionId, this.ownerToken);
			} catch (error) {
				if (error instanceof RecoveryLeaseHeld) return { action: "lease_held", holder: error };
				throw error;
			}
		}
		try {
			// Re-check under the lease: another owner may have reopened between our read and our lease.
			const again = await this.findSummary(sessionId);
			if (again && again.workerState === "ready") {
				const id = activeSessionIdOf(again);
				const { incarnation } = this.registry.recordIncarnation({ sessionId, activeSessionId: id, workerPid: again.workerPid, cause: "converged", openedBy: this.ownerToken });
				return id === before.activeSessionId ? { action: "healthy", incarnation } : { action: "converged", incarnation, previous: before };
			}
			const launch = this.#launchEnv();
			const commandId = this.newCommandId();
			const command: DaemonCommand = {
				type: "create",
				sessionPath: record.sessionPath,
				config: this.#sessionConfig(),
				launchEnv: launch.env,
			};
			const dispatch = await this.#dispatch(command, commandId, { sessionId, activeSessionId: before.activeSessionId, incarnationIndex: before.index }, 120_000);
			if (dispatch.verdict.verdict !== "completed") {
				throw new NotRecoverable(`reopen of ${sessionId} was ${dispatch.verdict.verdict}; recorded as ${commandId}, not retried`, summary);
			}
			const reopened = summaryOf(dispatch.verdict.response.data, "reopen");
			if (reopened.sessionId !== sessionId) {
				throw new SessionIdentityMismatch(sessionId, reopened.sessionId, record.sessionPath);
			}
			const { incarnation } = this.registry.recordIncarnation({ sessionId, activeSessionId: activeSessionIdOf(reopened), workerPid: reopened.workerPid, cause: "reopen", openedBy: this.ownerToken });
			return { action: "reopened", incarnation, previous: before, createCommandId: commandId };
		} finally {
			lease?.release();
		}
	}

	/** Wait until the supervisor reports the worker `ready`, then attach and record its generation. */
	async waitReady(sessionId: string, timeoutMs = 60_000): Promise<SessionSummary> {
		const deadline = Date.now() + timeoutMs;
		for (;;) {
			const summary = await this.findSummary(sessionId);
			if (summary?.workerState === "ready") return summary;
			if (Date.now() > deadline) throw new Error(`session ${sessionId} not ready within ${timeoutMs} ms (state ${String(summary?.workerState)})`);
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	}

	/** Wait until the supervisor reports the worker `failed`, i.e. the recoverable state. */
	async waitFailed(sessionId: string, timeoutMs = 30_000): Promise<SessionSummary> {
		const deadline = Date.now() + timeoutMs;
		for (;;) {
			const summary = await this.findSummary(sessionId);
			if (summary?.workerState === "failed") return summary;
			if (Date.now() > deadline) throw new Error(`session ${sessionId} not failed within ${timeoutMs} ms (state ${String(summary?.workerState)})`);
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	}

	/** Attach the Governor's connection to the current incarnation (read-only). */
	async attach(sessionId: string): Promise<DaemonResponse> {
		const current = this.registry.current(sessionId);
		const launch = this.#launchEnv();
		return this.read({ type: "attach", activeSessionId: current.activeSessionId, clientId: this.clientId, telemetryDisabled: true, launchEnv: launch.env }, 60_000);
	}
}

export type { CanonicalSessionPath };
