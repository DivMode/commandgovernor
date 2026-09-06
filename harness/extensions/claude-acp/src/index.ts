/**
 * Claude on the user's subscription, as a Prime model provider that is an ACP
 * client.
 *
 * The ownership boundary, which is the whole design:
 *
 *   Claude Code owns the conversation. Its session, its transcript, its
 *   compaction, its resume cursor and therefore its prompt-cache lineage. One
 *   ACP session per Prime conversation, created once and loaded natively after
 *   a restart. A turn sends the NEW prompt and nothing else.
 *
 *   Prime owns everything around it. The UI, governance, orchestration, and —
 *   new here — approvals: Claude Code runs its own tools and asks its client
 *   before every one, and this extension is that client (see `approval.ts`).
 *
 * What Prime deliberately does NOT do is reconstruct Claude's history. The
 * provider it replaces rebuilt Claude Code's JSONL from Prime's message array
 * after an abort, a compaction or a tree navigation, and opened a fresh
 * `query()` with `resume` on every clean turn; both move the cache-breaking
 * prefix and both are paid for out of the user's plan allowance. Here, an abort
 * is `session/cancel` on a session that survives it, a Prime compaction is
 * ignored because Claude's own context is not Prime's to compact, and a Prime
 * branch mints a NEW Claude session rather than rewriting the old one.
 *
 * Consequence, stated because it is a real one: Prime's own tools (`ipython`
 * and every package-registered tool) are not offered to Claude on this
 * provider. Tool execution lives inside Claude Code, which is what makes the
 * permission boundary enforceable. Bridging Prime's tools back in would be an
 * MCP server hosted over ACP (`clientCapabilities.mcp`), and is not built here.
 */

import { createAssistantMessageEventStream, getModels, type AssistantMessage, type AssistantMessageEventStream, type Context, type Model, type SimpleStreamOptions, type Usage } from "@earendil-works/pi-ai";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import { AcpClient, HANDSHAKE_TIMEOUT_MS, initializeParams, type AcpPermissionOutcome, type AcpPermissionRequest } from "./acp-client.ts";
import { readConfig, resolveOutcome, type ApprovalPolicy } from "./approval.ts";
import { resolveAcpChildEnv } from "./child-env.ts";
import { assertNoLongContextSuffix, buildModels } from "./models.ts";
import { newPromptSince, readRecord, recordPath, watermarkBeforeLatest, writeRecord, type TimestampedMessage } from "./session-state.ts";

export const PROVIDER_ID = "claude-acp";

/**
 * The pinned ACP adapter's executable, resolved from this package.
 *
 * `pins/packages/claude-agent-acp-<version>` carries the adapter and its own
 * committed lockfile, installed by `scripts/bootstrap.sh`. Resolving through
 * `require.resolve` rather than a hardcoded path means the pin's own layout is
 * the authority and a missing install fails with a message that says so.
 */
const ADAPTER_PACKAGE = "@agentclientprotocol/claude-agent-acp";

/**
 * The adapter's CLI entry point.
 *
 * `scripts/bootstrap.sh` extracts the pinned tarball to `pins/packages/` and
 * links it into this package's own `node_modules`, so ordinary Node resolution
 * finds it and the pin's layout stays the authority. `CG_CLAUDE_ACP_ADAPTER`
 * overrides it for the conformance suite, which drives the adapter from a
 * fixture root; `adapterCommand` in the project's settings overrides it for a
 * user who installed it elsewhere.
 */
function resolveAdapterEntry(override?: string): string {
	const explicit = override ?? process.env.CG_CLAUDE_ACP_ADAPTER;
	if (explicit) return explicit;
	const require = createRequire(import.meta.url);
	try {
		return join(dirname(require.resolve(`${ADAPTER_PACKAGE}/package.json`)), "dist", "index.js");
	} catch (error) {
		throw new Error(
			`claude-acp: the pinned ACP adapter (${ADAPTER_PACKAGE}) is not installed. Run scripts/bootstrap.sh, or set adapterCommand in .prime/agent/claude-acp.json. (${String((error as Error).message)})`,
		);
	}
}

// --- one Claude session per Prime conversation -----------------------------

interface LiveSession {
	readonly client: AcpClient;
	readonly claudeSessionId: string;
	readonly primeSessionId: string;
	readonly recordFile: string;
	readonly cwd: string;
	model: string;
	watermark: number;
	/** Updates are ignored until the turn's own prompt has been sent. */
	promptInFlight: boolean;
	/** Receives `session/update` payloads for the turn in flight. */
	sink: ((update: Record<string, unknown>) => void) | undefined;
}

let live: LiveSession | null = null;
/**
 * Set when Prime navigated its session tree. The next turn mints a fresh Claude
 * session and starts its watermark at Prime's newest user message, so the
 * branch neither rewrites the old Claude session nor replays history into a new
 * one.
 */
let branchPending = false;
let piContext: ExtensionContext | undefined;
let approvalPolicy: ApprovalPolicy = "ask";
let adapterOverride: string | undefined;
/** Opt-in 200K clamp; see child-env.ts for why it is not a default. */
let disableLongContext = false;

function endSession(reason: string): void {
	if (!live) return;
	log(`${reason}: closing Claude session ${live.claudeSessionId.slice(0, 8)}`);
	live.client.close();
	live = null;
}

function log(message: string): void {
	if (process.env.CG_CLAUDE_ACP_DEBUG) process.stderr.write(`claude-acp: ${message}\n`);
}

// --- the approval hook ------------------------------------------------------

async function handlePermission(request: AcpPermissionRequest): Promise<AcpPermissionOutcome> {
	const ui = piContext?.hasUI ? piContext.ui : undefined;
	const ask = ui
		? async (prompt: string, optionNames: string[]): Promise<number | undefined> => {
				const chosen = await ui.select(`Claude asks: ${prompt}`, optionNames);
				return chosen === undefined ? undefined : optionNames.indexOf(chosen);
			}
		: undefined;
	const outcome = await resolveOutcome(request, approvalPolicy, ask);
	log(`permission ${JSON.stringify(request.toolCall?.title ?? request.toolCall?.kind ?? "?")} -> ${JSON.stringify(outcome)}`);
	return outcome;
}

// --- session lifecycle ------------------------------------------------------

/**
 * Which identity Claude Code is about to bill, as the adapter reports it.
 *
 * `_auth/status_update` (adapter >= 0.75.0) is the enforcement point that
 * environment stripping cannot provide. Stripping only reaches variables; an
 * `apiKeyHelper` in the user's own `settings.json` outranks the subscription
 * login inside Claude Code and would quietly pay for turns with an API key. The
 * adapter reads the real precedence and says `kind: "api_key"`, so refusing on
 * it closes the one hole the environment boundary has.
 */
interface AuthStatus {
	readonly kind?: string;
	readonly label?: string;
	readonly detail?: string;
}

/** Identities this product refuses to run on, and what to say about each. */
const REFUSED_AUTH_KINDS: Record<string, string> = {
	api_key: "an Anthropic API key (usage-billed)",
	gateway: "a custom model gateway",
	external: "a non-first-party backend (Bedrock/Vertex/Foundry)",
	none: "no Claude Code login at all",
};

/**
 * Refuse a turn whose identity is not the user's subscription.
 *
 * Unknown or not-yet-reported is NOT a refusal: an older adapter never sends the
 * notification, and blocking on a signal that may never arrive would turn a
 * missing feature into a broken product. The check is still falsifiable — a
 * reported `api_key` fails it, and the conformance suite drives exactly that.
 */
function assertSubscriptionIdentity(status: AuthStatus | undefined): void {
	if (!status?.kind) return;
	const refusal = REFUSED_AUTH_KINDS[status.kind];
	if (!refusal) return;
	throw new Error(
		`claude-acp: Claude Code reports it would bill ${refusal}${status.label ? ` (${status.label})` : ""}. ` +
			"Command Governor runs Claude only on the user's own Claude Code subscription login; " +
			"remove the apiKeyHelper / API key / gateway from the Claude Code settings that apply here, or run `claude /login` and choose the subscription account.",
	);
}

let authStatus: AuthStatus | undefined;
let authStatusArrived: Promise<void> = Promise.resolve();

async function startClient(cwd: string, registry: unknown): Promise<AcpClient> {
	// The refusal happens HERE, before any child exists: a harness-held
	// Anthropic credential must never reach a process, not merely be unused by
	// one.
	const env = await resolveAcpChildEnv(registry as Parameters<typeof resolveAcpChildEnv>[0], process.env, { disableLongContext });
	const entry = resolveAdapterEntry(adapterOverride);

	authStatus = undefined;
	let arrived: () => void = () => {};
	authStatusArrived = new Promise<void>((resolve) => {
		arrived = resolve;
	});

	const client = AcpClient.start(
		{ command: process.execPath, args: [entry], cwd, env },
		{
			onUpdate: (_sessionId, update) => {
				if (live?.promptInFlight) live.sink?.(update);
			},
			onNotification: (method, params) => {
				if (method !== "_auth/status_update") return;
				const status = (params as { authStatus?: AuthStatus } | undefined)?.authStatus;
				if (!status) return;
				authStatus = status;
				log(`auth identity: ${JSON.stringify(status.kind)} ${JSON.stringify(status.label ?? "")}`);
				arrived();
			},
			onPermission: handlePermission,
			onExit: (code) => {
				if (live?.client === client) live = null;
				log(`adapter exited with code ${String(code)}`);
			},
		},
	);
	try {
		await client.request("initialize", initializeParams(), HANDSHAKE_TIMEOUT_MS);
		// The adapter probes the CLI for its identity in the background and pushes
		// the answer; `initialize` never waits on it. Give it a bounded moment so
		// the refusal below acts on a real reading rather than on silence.
		await Promise.race([
			authStatusArrived,
			new Promise<void>((resolve) => {
				setTimeout(resolve, AUTH_STATUS_GRACE_MS).unref?.();
			}),
		]);
		assertSubscriptionIdentity(authStatus);
	} catch (error) {
		client.kill();
		throw error;
	}
	return client;
}

/** How long to wait for the adapter's identity push before proceeding without it. */
const AUTH_STATUS_GRACE_MS = 8_000;

/**
 * Reattach to a Claude session by id, preferring the method that does not
 * replay.
 *
 * ACP defines two restore paths and the difference is exactly the one that
 * matters here: `session/resume` MUST NOT replay the conversation, while
 * `session/load` replays every prior turn back to the client as `session/update`
 * notifications. Prime has its own record of the conversation and does not need
 * a second copy, so `resume` is the right call and `load` is the fallback for an
 * adapter that does not implement it. Either way the id is Claude's own and the
 * transcript stays Claude's; what is being avoided is a client-side rebuild, not
 * a replay we would merely ignore.
 */
async function restoreSession(client: AcpClient, sessionId: string, params: Record<string, unknown>): Promise<string> {
	try {
		await client.request("session/resume", { sessionId, ...params }, HANDSHAKE_TIMEOUT_MS);
		return sessionId;
	} catch (error) {
		log(`session/resume unavailable (${String((error as Error).message)}); falling back to session/load`);
	}
	await client.request("session/load", { sessionId, ...params }, HANDSHAKE_TIMEOUT_MS);
	return sessionId;
}

/**
 * Get the Claude session for this Prime conversation, creating or restoring one.
 *
 * The restart path is native: the adapter reattaches to Claude's own transcript
 * for that id. Prime hands over an id, never a history.
 */
async function ensureSession(model: string, ctx: ExtensionContext | undefined, messages: readonly TimestampedMessage[]): Promise<LiveSession> {
	const cwd = ctx?.cwd ?? process.cwd();
	const sessionManager = ctx?.sessionManager;
	const primeSessionId = sessionManager?.getSessionId?.() ?? "no-session";
	const sessionDir = sessionManager?.getSessionDir?.() ?? join(cwd, ".prime", "agent", "sessions");

	if (live && live.primeSessionId === primeSessionId && live.client.alive && !branchPending) {
		if (live.model !== model) {
			await live.client.request("session/set_config_option", { sessionId: live.claudeSessionId, configId: "model", value: model }, HANDSHAKE_TIMEOUT_MS);
			live.model = model;
		}
		return live;
	}
	if (live) endSession(branchPending ? "session_tree" : "prime session changed");

	const file = recordPath(sessionDir, primeSessionId);
	const stored = branchPending ? undefined : readRecord(file);
	const client = await startClient(cwd, ctx?.modelRegistry);

	// `_meta.claudeCode.options.model` is how the adapter is told which model to
	// run; no `[1m]` suffix ever (see models.ts).
	assertNoLongContextSuffix([model]);
	const params = { cwd, mcpServers: [], _meta: { claudeCode: { options: { model } } } };

	let claudeSessionId: string;
	if (stored) {
		claudeSessionId = await restoreSession(client, stored.claudeSessionId, params);
		log(`restored Claude session ${claudeSessionId.slice(0, 8)} for Prime session ${primeSessionId.slice(0, 8)}`);
	} else {
		const created = await client.request<{ sessionId?: string }>("session/new", params, HANDSHAKE_TIMEOUT_MS);
		if (typeof created?.sessionId !== "string") throw new Error("claude-acp: session/new returned no sessionId");
		claudeSessionId = created.sessionId;
		log(`created Claude session ${claudeSessionId.slice(0, 8)} for Prime session ${primeSessionId.slice(0, 8)}`);
	}

	// Force the mode that routes every tool call back here for approval. The
	// user's own Claude Code settings may default to a permissive mode; on this
	// product the approval boundary is not theirs to switch off by accident.
	await client.request("session/set_mode", { sessionId: claudeSessionId, modeId: "default" }, HANDSHAKE_TIMEOUT_MS);

	// Model selection is `session/set_config_option`, not `_meta`. The adapter
	// takes `_meta.claudeCode.options.model` as a hint that its own resolved
	// `settings.model` can override (claude-agent-acp#1056), so the config option
	// is the only authoritative channel and is sent unconditionally.
	await client.request("session/set_config_option", { sessionId: claudeSessionId, configId: "model", value: model }, HANDSHAKE_TIMEOUT_MS);

	live = {
		client,
		claudeSessionId,
		primeSessionId,
		recordFile: file,
		cwd,
		model,
		// A loaded session resumes its own watermark. A NEW one starts caught up
		// with everything except the prompt about to be sent — see
		// `watermarkBeforeLatest`: minting a session is never an excuse to replay.
		watermark: stored?.watermark ?? watermarkBeforeLatest(messages),
		promptInFlight: false,
		sink: undefined,
	};
	branchPending = false;
	return live;
}

// --- the provider ----------------------------------------------------------

function emptyUsage(): Usage {
	return { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };
}

interface AcpUsage {
	readonly inputTokens?: number;
	readonly outputTokens?: number;
	readonly cachedReadTokens?: number;
	readonly cachedWriteTokens?: number;
	readonly totalTokens?: number;
}

/** ACP per-turn usage as Prime's `Usage`. Costs stay zero: these turns are plan-billed. */
function toPrimeUsage(usage: AcpUsage | undefined): Usage {
	const input = usage?.inputTokens ?? 0;
	const output = usage?.outputTokens ?? 0;
	const cacheRead = usage?.cachedReadTokens ?? 0;
	const cacheWrite = usage?.cachedWriteTokens ?? 0;
	return {
		input,
		output,
		cacheRead,
		cacheWrite,
		totalTokens: usage?.totalTokens ?? input + output + cacheRead + cacheWrite,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	};
}

function streamClaudeAcp(model: Model<never>, context: Context, options?: SimpleStreamOptions): AssistantMessageEventStream {
	const stream = createAssistantMessageEventStream();
	void runTurn(stream, model, context, options).catch((error: unknown) => {
		const message = error instanceof Error ? error.message : String(error);
		stream.push({
			type: "error",
			reason: "error",
			error: { ...baseMessage(model), stopReason: "error", errorMessage: message },
		});
		stream.end({ ...baseMessage(model), stopReason: "error", errorMessage: message });
	});
	return stream;
}

function baseMessage(model: Model<never>): AssistantMessage {
	return {
		role: "assistant",
		content: [],
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: emptyUsage(),
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

async function runTurn(stream: AssistantMessageEventStream, model: Model<never>, context: Context, options?: SimpleStreamOptions): Promise<void> {
	const messages = context.messages as unknown as TimestampedMessage[];
	const session = await ensureSession(model.id, piContext, messages);
	const next = newPromptSince(messages, session.watermark);
	if (next.text.length === 0) {
		throw new Error(
			"claude-acp: this turn carries no new user message. Claude Code holds the conversation, so there is nothing for Prime to resend; " +
				"a provider re-entry without new user input is not a turn.",
		);
	}

	const partial = baseMessage(model);
	const message: AssistantMessage = { ...partial, content: [] };
	stream.push({ type: "start", partial: message });

	let contentIndex = -1;
	let openKind: "text" | "thinking" | undefined;
	let buffer = "";

	const closeOpen = (): void => {
		if (openKind === undefined) return;
		if (openKind === "text") {
			message.content.push({ type: "text", text: buffer });
			stream.push({ type: "text_end", contentIndex, content: buffer, partial: message });
		} else {
			message.content.push({ type: "thinking", thinking: buffer });
			stream.push({ type: "thinking_end", contentIndex, content: buffer, partial: message });
		}
		openKind = undefined;
		buffer = "";
	};

	const open = (kind: "text" | "thinking"): void => {
		if (openKind === kind) return;
		closeOpen();
		contentIndex += 1;
		openKind = kind;
		buffer = "";
		stream.push(kind === "text" ? { type: "text_start", contentIndex, partial: message } : { type: "thinking_start", contentIndex, partial: message });
	};

	session.sink = (update) => {
		const kind = update.sessionUpdate;
		if (kind === "agent_message_chunk" || kind === "agent_thought_chunk") {
			const content = update.content as { type?: string; text?: string } | undefined;
			if (content?.type !== "text" || typeof content.text !== "string" || content.text.length === 0) return;
			open(kind === "agent_message_chunk" ? "text" : "thinking");
			buffer += content.text;
			stream.push(
				kind === "agent_message_chunk"
					? { type: "text_delta", contentIndex, delta: content.text, partial: message }
					: { type: "thinking_delta", contentIndex, delta: content.text, partial: message },
			);
			return;
		}
		// Claude Code executes its own tools, so a tool call is progress to show,
		// not work for Prime to do. Surfacing it as a Prime toolCall would make
		// Prime try to run a tool it does not have.
		if (kind === "tool_call" || kind === "tool_call_update") {
			const title = typeof update.title === "string" ? update.title : undefined;
			const status = typeof update.status === "string" ? update.status : undefined;
			if (title && piContext?.hasUI) piContext.ui.setWorkingMessage(`Claude: ${title}${status && status !== "completed" ? ` (${status})` : ""}`);
		}
	};

	// Prime's abort is ACP's `session/cancel`. The session survives it: the next
	// turn continues on the same Claude session id, with no rewrite.
	const onAbort = (): void => {
		log(`abort -> session/cancel on ${session.claudeSessionId.slice(0, 8)}`);
		session.client.notify("session/cancel", { sessionId: session.claudeSessionId });
	};
	options?.signal?.addEventListener("abort", onAbort, { once: true });

	session.promptInFlight = true;
	try {
		const response = await session.client.request<{ stopReason?: string; usage?: AcpUsage }>("session/prompt", {
			sessionId: session.claudeSessionId,
			prompt: [{ type: "text", text: next.text }],
		});
		closeOpen();
		if (piContext?.hasUI) piContext.ui.setWorkingMessage();

		// The watermark advances only on a delivered prompt, and it advances even
		// for a cancelled turn: Claude received that prompt, so resending it would
		// be a duplicate, not a retry.
		session.watermark = next.watermark;
		writeRecord(session.recordFile, { claudeSessionId: session.claudeSessionId, watermark: session.watermark, model: session.model });

		message.usage = toPrimeUsage(response?.usage);
		const cancelled = response?.stopReason === "cancelled";
		message.stopReason = cancelled ? "aborted" : response?.stopReason === "max_tokens" ? "length" : "stop";
		if (cancelled) {
			stream.push({ type: "error", reason: "aborted", error: message });
			stream.end(message);
			return;
		}
		stream.push({ type: "done", reason: message.stopReason === "length" ? "length" : "stop", message });
		stream.end(message);
	} finally {
		session.promptInFlight = false;
		session.sink = undefined;
		options?.signal?.removeEventListener("abort", onAbort);
	}
}

// --- registration -----------------------------------------------------------

export default function (pi: ExtensionAPI): void {
	const config = readConfig(process.cwd(), (message) => process.stderr.write(`${message}\n`));
	approvalPolicy = config.approval;
	adapterOverride = config.adapterCommand;
	disableLongContext = config.disableLongContext === true;

	pi.on("session_start", (event, ctx) => {
		piContext = ctx;
		if (event.reason === "new" || event.reason === "fork") {
			// A new or forked Prime session is a different conversation; the old
			// Claude session is left exactly as it is.
			endSession(`session_start:${event.reason}`);
			branchPending = event.reason === "fork";
		}
	});

	// A Prime compaction rewrites PRIME's history. Claude's context is Claude's
	// to compact, so there is deliberately no handler for `session_compact`:
	// doing nothing here is the behaviour under test.

	pi.on("session_tree", () => {
		// Rewind, fork-at-point or branch switch. Mint a new Claude session on the
		// next turn rather than rewriting the one that already exists.
		branchPending = true;
	});

	pi.on("session_shutdown", () => {
		endSession("session_shutdown");
	});

	const models = buildModels(getModels("anthropic") as unknown as { id: string; name?: string }[]);
	assertNoLongContextSuffix(models.map((entry) => entry.id));

	pi.registerProvider(PROVIDER_ID, {
		baseUrl: PROVIDER_ID,
		apiKey: "not-used",
		api: PROVIDER_ID,
		models,
		// Cast: pi-ai's AssistantMessageEventStream is a diamond dependency between
		// pi-coding-agent and pi-agent-core, so the two declarations are structurally
		// identical but nominally distinct.
		streamSimple: streamClaudeAcp as unknown as NonNullable<Parameters<ExtensionAPI["registerProvider"]>[1]["streamSimple"]>,
	});
}

/** Exported for the conformance suite: it must be able to see the watermark rule work. */
export { newPromptSince, watermarkBeforeLatest };
