// A stub ACP agent, so the conformance suite can see EXACTLY what Command
// Governor sends to Claude.
//
// The product claim under test is a claim about the wire: one Claude session
// per Prime conversation, only the new prompt each turn, a native restore after
// a restart, and no rewrite after an abort or a Prime compaction. None of that
// is observable from Claude's side and none of it needs a model to check, so
// this stands in for `@agentclientprotocol/claude-agent-acp` and records every
// request the client makes to `$CG_ACP_STUB_LOG` as JSONL.
//
// It is deliberately NOT a mock of Claude: it implements the ACP methods the
// real adapter implements, with the real adapter's response shapes (read from
// its `dist/acp-agent.js` at 0.75.1), so a client change that would break
// against the real adapter breaks here too.
//
// Environment:
//   CG_ACP_STUB_LOG         JSONL file every inbound request is appended to (required)
//   CG_ACP_STUB_SESSION_ID  the session id `session/new` returns (default: a fixed one)
//   CG_ACP_STUB_AUTH_KIND   the `kind` pushed on `_auth/status_update` (default: account)
//   CG_ACP_STUB_PERMISSION  when set, every prompt first asks the client for permission
//   CG_ACP_STUB_DELAY_MS    delay before a prompt answers, so a test can cancel it
//   CG_ACP_STUB_RECORD_ENV  when set, record this process's own environment — the
//                           only place a test can read the environment a real
//                           child of the shipped spawn path was started with

import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";

const logPath = process.env.CG_ACP_STUB_LOG;
if (!logPath) {
	process.stderr.write("claude-acp-stub-agent: CG_ACP_STUB_LOG is required\n");
	process.exit(2);
}

const SESSION_ID = process.env.CG_ACP_STUB_SESSION_ID || "11111111-2222-3333-4444-555555555555";
const AUTH_KIND = process.env.CG_ACP_STUB_AUTH_KIND || "account";
const DELAY_MS = Number.parseInt(process.env.CG_ACP_STUB_DELAY_MS ?? "0", 10) || 0;

const record = (entry) => appendFileSync(logPath, `${JSON.stringify({ pid: process.pid, at: Date.now(), ...entry })}\n`);
const write = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);

if (process.env.CG_ACP_STUB_RECORD_ENV) record({ kind: "child_env", env: { ...process.env } });

let nextId = 1000;
const pendingClientReplies = new Map();

/** Ask the client something and await its answer (the permission path). */
function ask(method, params) {
	const id = nextId++;
	return new Promise((resolve) => {
		pendingClientReplies.set(id, resolve);
		write({ jsonrpc: "2.0", id, method, params });
	});
}

/** The turn in flight, so `session/cancel` can settle it as the adapter does. */
let activeTurn = null;

async function handlePrompt(params) {
	const sessionId = params?.sessionId;
	write({
		jsonrpc: "2.0",
		method: "session/update",
		params: { sessionId, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "STUB-REPLY" } } },
	});

	if (process.env.CG_ACP_STUB_PERMISSION) {
		const answer = await ask("session/request_permission", {
			sessionId,
			toolCall: { title: "Write /etc/hosts", kind: "edit" },
			options: [
				{ optionId: "allow-once", name: "Allow once", kind: "allow_once" },
				{ optionId: "reject-once", name: "Reject", kind: "reject_once" },
			],
		});
		record({ kind: "permission_answer", answer });
	}

	if (DELAY_MS > 0) {
		const settled = await new Promise((resolve) => {
			activeTurn = () => resolve("cancelled");
			setTimeout(() => resolve("end_turn"), DELAY_MS);
		});
		activeTurn = null;
		if (settled === "cancelled") return { stopReason: "cancelled", usage: usage() };
	}
	return { stopReason: "end_turn", usage: usage() };
}

const usage = () => ({ inputTokens: 7, outputTokens: 3, cachedReadTokens: 100, cachedWriteTokens: 0, totalTokens: 110 });

async function dispatch(method, params) {
	switch (method) {
		case "initialize":
			// Pushed after the response, exactly as the real adapter does.
			setTimeout(() => write({ jsonrpc: "2.0", method: "_auth/status_update", params: { authStatus: { kind: AUTH_KIND, label: AUTH_KIND === "account" ? "Claude Max" : "Anthropic API key" } } }), 5);
			return {
				protocolVersion: 1,
				agentCapabilities: { loadSession: true, sessionCapabilities: { resume: {}, list: {}, close: {} }, promptCapabilities: { image: true, embeddedContext: true } },
				agentInfo: { name: "claude-acp-stub-agent", title: "Stub", version: "0.0.0" },
				authMethods: [],
			};
		case "session/new":
			return { sessionId: SESSION_ID, modes: { currentModeId: "default", availableModes: [{ id: "default", name: "Manual" }] } };
		case "session/resume":
		case "session/load":
			return { modes: { currentModeId: "default", availableModes: [{ id: "default", name: "Manual" }] } };
		case "session/set_mode":
			return {};
		case "session/set_config_option":
			return { configOptions: [] };
		case "session/prompt":
			return handlePrompt(params);
		default: {
			const error = new Error(`Method not found: ${method}`);
			error.code = -32601;
			throw error;
		}
	}
}

createInterface({ input: process.stdin }).on("line", (line) => {
	let message;
	try {
		message = JSON.parse(line);
	} catch {
		return;
	}
	if (typeof message !== "object" || message === null) return;

	// A reply to something we asked the client.
	if (message.id !== undefined && message.method === undefined) {
		const resolve = pendingClientReplies.get(message.id);
		if (resolve) {
			pendingClientReplies.delete(message.id);
			resolve(message.result ?? { error: message.error });
		}
		return;
	}

	record({ kind: "request", method: message.method, params: message.params });

	if (message.method === "session/cancel") {
		activeTurn?.();
		return;
	}
	if (message.id === undefined) return; // another notification; nothing to answer

	void dispatch(message.method, message.params).then(
		(result) => write({ jsonrpc: "2.0", id: message.id, result }),
		(error) => write({ jsonrpc: "2.0", id: message.id, error: { code: error.code ?? -32603, message: String(error.message ?? error) } }),
	);
});

// An ACP agent exits on stdin EOF; nothing else keeps this alive.
process.stdin.on("end", () => process.exit(0));
