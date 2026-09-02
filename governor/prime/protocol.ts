/**
 * The narrow slice of Prime Agent's public daemon protocol that Command
 * Governor depends on, written down here rather than imported from the pinned
 * package.
 *
 * Why a copy and not an import: Prime's `dist/index.d.ts` does not export the
 * daemon protocol types, and the `dist/modes/daemon/daemon-protocol.js` module
 * is an implementation file, not a contract. Everything below is therefore a
 * *claim about the pin* -- and claims about the pin are checked, not trusted:
 * `conformance/tier1/prime-protocol.test.ts` loads the pinned module and
 * asserts that the read-only command set and the error-code vocabulary here
 * are exactly what the pinned build ships. A re-pin that changes either fails
 * that test before it can silently change what "structural" means.
 *
 * Source at the pin (commit 514633727bf26d74f39f3119c2b0e31a5ceb2a9d):
 *   packages/coding-agent/src/modes/daemon/daemon-protocol.ts
 *   packages/coding-agent/src/modes/daemon/daemon-errors.ts
 *   packages/coding-agent/src/modes/daemon/daemon-session-list.ts
 */

export const PRIME_DAEMON_PROTOCOL = { name: "prime-agent.daemon", version: 7 } as const;

export interface DaemonProtocolInfo {
	readonly name: string;
	readonly version: number;
}

/** The first frame the supervisor writes on every connection. */
export interface DaemonHello {
	readonly type: "daemon_hello";
	readonly socketPath: string;
	readonly protocol: DaemonProtocolInfo;
	readonly schemaId?: string;
	readonly schemaRevision?: number;
	readonly appVersion?: string;
	readonly supervisorGeneration?: string;
	readonly supervisorPid?: number;
	readonly supervisorOwnerToken?: string;
	readonly [extra: string]: unknown;
}

/**
 * The typed error vocabulary of the pinned daemon. This is the ENTIRE set: a
 * failure response carrying no `errorInfo` is untyped, and an untyped failure
 * proves nothing about whether the worker ran the command.
 */
export type DaemonErrorInfo =
	| { readonly code: "missing_session_cwd"; readonly issue: unknown }
	| { readonly code: "session_import_file_not_found"; readonly filePath: string }
	| {
			readonly code: "session_already_active";
			readonly sessionPath: string;
			readonly activeSessionId?: string;
	  }
	| {
			readonly code: "command_result_uncertain";
			readonly clientId: string;
			readonly commandId: string;
	  };

export type DaemonErrorCode = DaemonErrorInfo["code"];

export const DAEMON_ERROR_CODES: readonly DaemonErrorCode[] = [
	"missing_session_cwd",
	"session_import_file_not_found",
	"session_already_active",
	"command_result_uncertain",
];

/**
 * The codes `serializeDaemonError` (daemon-errors.ts) can produce. Both the
 * supervisor's and the WORKER's catch paths run it, so a typed code may be
 * relayed from a worker that has already acted (`daemon-mode.ts` writes
 * `failure(..., serializeDaemonError(error))` for a handler that threw).
 *
 * This is a VOCABULARY, not a proof: whether a given code was thrown before
 * or after a command's external effect depends on the command, and is
 * recorded per reviewed `(commandType, code)` pair in
 * `governor/mutation/proof.ts`. Nothing may treat membership here as
 * evidence that nothing happened.
 *
 * `command_result_uncertain` is absent because the journal path, not the
 * serializer, emits it: it is the substrate's own statement that the outcome
 * is unknown.
 */
export const SERIALIZED_ERROR_CODES: ReadonlySet<DaemonErrorCode> = new Set<DaemonErrorCode>([
	"missing_session_cwd",
	"session_import_file_not_found",
	"session_already_active",
]);

export interface DaemonSuccessResponse {
	readonly id?: string;
	readonly type: "response";
	readonly command: string;
	readonly success: true;
	readonly data?: unknown;
}

export interface DaemonFailureResponse {
	readonly id?: string;
	readonly type: "response";
	readonly command: string;
	readonly success: false;
	readonly error: string;
	readonly errorInfo?: DaemonErrorInfo;
}

export type DaemonResponse = DaemonSuccessResponse | DaemonFailureResponse;

export interface DaemonEventCursor {
	readonly generation: string;
	readonly sequence: number;
}

/**
 * Unsolicited frames. Session-scoped ones arrive as `session_event` (with a
 * cursor); supervisor-scoped ones as `event`. Both are events to a client.
 */
export interface DaemonEventMeta {
	readonly id?: string;
	readonly activeSessionId?: string;
	readonly sequence?: number;
	readonly cursor?: DaemonEventCursor;
	readonly emittedAt?: string;
}

export interface DaemonEventEnvelope {
	readonly type: "event" | "session_event";
	readonly id?: string;
	readonly activeSessionId?: string;
	readonly sequence?: number;
	/** Supervisor-scoped `event` frames carry the cursor here ... */
	readonly cursor?: DaemonEventCursor;
	/** ... session-scoped `session_event` frames carry it under `meta`. */
	readonly meta?: DaemonEventMeta;
	readonly emittedAt?: string;
	readonly event?: { readonly type?: string; readonly [extra: string]: unknown };
	readonly [extra: string]: unknown;
}

/** The generation-scoped cursor of an event frame, wherever the pin puts it. */
export function eventCursor(event: DaemonEventEnvelope): DaemonEventCursor | undefined {
	return event.cursor ?? event.meta?.cursor;
}

/** Resident session-host process state, as the supervisor reports it. */
export type WorkerState = "starting" | "ready" | "recovering" | "stopping" | "failed";

export const WORKER_STATES: readonly WorkerState[] = ["starting", "ready", "recovering", "stopping", "failed"];

/**
 * The subset of `SessionSummary` the Governor reads. `activity` is present in
 * the wire object but is deliberately NOT modelled: Issue #15 D10 showed it
 * reporting "working" for minutes after a worker died, so nothing in the
 * Governor may reason from it.
 */
export interface SessionSummary {
	readonly id: string;
	readonly activeSessionId?: string;
	readonly sessionId: string;
	readonly sessionFile?: string;
	readonly sessionName?: string;
	readonly lifecycle?: string;
	readonly isStreaming?: boolean;
	readonly attachedClients?: number;
	readonly messageCount?: number;
	readonly workerState?: WorkerState;
	readonly workerPid?: number;
	readonly [extra: string]: unknown;
}

/** The active-session id of a summary. `id` is the legacy field name for it. */
export function activeSessionIdOf(summary: Pick<SessionSummary, "id" | "activeSessionId">): string {
	return summary.activeSessionId ?? summary.id;
}

/**
 * Commands the pinned supervisor treats as read-only: they bypass the command
 * journal and are never idempotency-keyed. Everything else is a mutation and
 * is journaled by `clientId + commandId` before dispatch.
 *
 * Copied from the pin; the conformance suite diffs it against the pinned build.
 */
export const READ_ONLY_DAEMON_COMMANDS: ReadonlySet<string> = new Set([
	"ack_result",
	"list",
	"list_saved_sessions",
	"attach",
	"reattach",
	"agent_messages_status",
	"wait_for_idle",
	"get_session_header",
	"get_state",
	"get_connection_state",
	"get_messages",
	"get_rlm_children",
	"get_session_stats",
	"get_context_tree",
	"get_commands",
	"get_resource_snapshot",
	"get_model_catalog",
	"get_available_models",
	"get_queue",
	"cron_list",
	"heartbeats_list",
	"heartbeat_get",
	"get_session_context",
	"get_session_tree",
	"get_user_messages_for_forking",
	"get_last_assistant_text",
	"get_system_prompt",
	"get_rlm_max_depth_status",
	"get_tool_definition",
]);

export type CommandKind = "read" | "mutating";

export function commandKind(commandType: string): CommandKind {
	return READ_ONLY_DAEMON_COMMANDS.has(commandType) ? "read" : "mutating";
}

/** A daemon command as the Governor sends it: a type plus whatever fields it needs. */
export interface DaemonCommand {
	readonly type: string;
	readonly [field: string]: unknown;
}

export interface DaemonCommandEnvelope {
	readonly type: "command";
	readonly id: string;
	readonly protocol: DaemonProtocolInfo;
	readonly clientId: string;
	readonly command: DaemonCommand;
}

// ---------------------------------------------------------------------------
// Runtime guards. The wire is untrusted input; the types above are what we
// expect, these are what we check.
// ---------------------------------------------------------------------------

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isDaemonHello(value: unknown): value is DaemonHello {
	return (
		isRecord(value) &&
		value.type === "daemon_hello" &&
		isRecord(value.protocol) &&
		typeof value.protocol.name === "string" &&
		typeof value.protocol.version === "number"
	);
}

export function isDaemonErrorInfo(value: unknown): value is DaemonErrorInfo {
	return isRecord(value) && typeof value.code === "string" && (DAEMON_ERROR_CODES as readonly string[]).includes(value.code);
}

export function isDaemonResponse(value: unknown): value is DaemonResponse {
	if (!isRecord(value) || value.type !== "response" || typeof value.command !== "string") return false;
	if (value.success === true) return true;
	if (value.success !== false || typeof value.error !== "string") return false;
	return value.errorInfo === undefined || isDaemonErrorInfo(value.errorInfo);
}

export function isDaemonEvent(value: unknown): value is DaemonEventEnvelope {
	return isRecord(value) && (value.type === "event" || value.type === "session_event");
}

export function isSessionSummary(value: unknown): value is SessionSummary {
	if (!isRecord(value) || typeof value.id !== "string" || typeof value.sessionId !== "string") return false;
	if (value.workerState !== undefined && !(WORKER_STATES as readonly unknown[]).includes(value.workerState)) return false;
	if (value.workerPid !== undefined && typeof value.workerPid !== "number") return false;
	return true;
}
