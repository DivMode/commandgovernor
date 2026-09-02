/**
 * Mock OpenAI-compatible chat-completions server for the runtime tier.
 *
 * Deterministic, credential-free, and every request is logged to a JSONL file
 * so "did the runtime call the model again?" is answerable from a file rather
 * than inferred. Transplanted from the Issue #15 bake-off harness, where the
 * same behaviours produced the S1 evidence.
 *
 * Behaviour is selected by the LAST user message text:
 *   ECHO:<text>          -> stream <text> back, end_turn
 *   SLOW:<n>:<ms>        -> stream n chunks, one every ms milliseconds, then end
 * Anything else         -> "ok"
 *
 * Tool calls are deliberately not supported: a tool call would trigger
 * Prime's kernel bootstrap (uv + CPython, ~270 MB, network), and the
 * credential-free tier must not depend on that.
 *
 * Run as a script: `node conformance/lib/mock-provider.ts` with MOCK_PORT and
 * MOCK_LOG in the environment.
 */

import { appendFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";

const port = Number(process.env.MOCK_PORT ?? 0);
const logPath = process.env.MOCK_LOG;
if (!logPath) {
	console.error("mock-provider: MOCK_LOG is required");
	process.exit(2);
}
let seq = 0;

function log(entry: Record<string, unknown>): void {
	appendFileSync(logPath as string, `${JSON.stringify({ ts: new Date().toISOString(), ...entry })}\n`);
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

interface ChatMessage {
	role?: string;
	content?: unknown;
}

function lastUser(messages: ChatMessage[]): string {
	for (let i = messages.length - 1; i >= 0; i -= 1) {
		const message = messages[i];
		if (message?.role === "user") {
			const content = message.content;
			if (typeof content === "string") return content;
			if (Array.isArray(content)) return content.map((part) => (part as { text?: string }).text ?? "").join("");
		}
	}
	return "";
}

function sse(res: ServerResponse, payload: unknown): void {
	res.write(`data: ${JSON.stringify(payload)}\n\n`);
}

async function handleChat(res: ServerResponse, body: { model?: string; messages?: ChatMessage[]; stream?: boolean }): Promise<void> {
	const id = `chatcmpl-${++seq}`;
	const model = body.model ?? "mock";
	const messages = body.messages ?? [];
	const text = lastUser(messages);
	log({ kind: "request", id, model, stream: !!body.stream, lastUser: text.slice(0, 200), nMessages: messages.length });
	res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
	const base = { id, object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model };
	const chunk = (delta: Record<string, unknown>, finish: string | null = null) =>
		sse(res, { ...base, choices: [{ index: 0, delta, finish_reason: finish }] });
	const usage = () => sse(res, { ...base, choices: [], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } });
	chunk({ role: "assistant", content: "" });
	let match: RegExpMatchArray | null;
	if ((match = text.match(/^ECHO:(.*)$/s))) {
		chunk({ content: match[1] });
		chunk({}, "stop");
		usage();
		res.end("data: [DONE]\n\n");
		log({ kind: "response", id, mode: "echo" });
		return;
	}
	if ((match = text.match(/^SLOW:(\d+):(\d+)/))) {
		const n = Number(match[1]);
		const ms = Number(match[2]);
		for (let i = 0; i < n; i += 1) {
			chunk({ content: `tick${i} ` });
			log({ kind: "chunk", id, i });
			await sleep(ms);
			if (res.destroyed) {
				log({ kind: "client-gone", id, at: i });
				return;
			}
		}
		chunk({}, "stop");
		usage();
		res.end("data: [DONE]\n\n");
		log({ kind: "response", id, mode: "slow", n });
		return;
	}
	chunk({ content: "ok" });
	chunk({}, "stop");
	usage();
	res.end("data: [DONE]\n\n");
	log({ kind: "response", id, mode: "default" });
}

const server = createServer((req: IncomingMessage, res: ServerResponse) => {
	let buffer = "";
	req.on("data", (data: Buffer) => {
		buffer += data.toString("utf8");
	});
	req.on("end", () => {
		void (async () => {
			try {
				if (req.method === "GET" && req.url?.startsWith("/v1/models")) {
					res.writeHead(200, { "content-type": "application/json" });
					res.end(JSON.stringify({ object: "list", data: [{ id: "mock-1", object: "model", owned_by: "cg" }] }));
					return;
				}
				if (req.method === "POST" && req.url?.includes("/chat/completions")) {
					await handleChat(res, JSON.parse(buffer || "{}") as { model?: string; messages?: ChatMessage[]; stream?: boolean });
					return;
				}
				log({ kind: "unhandled", method: req.method, url: req.url });
				res.writeHead(404);
				res.end();
			} catch (error) {
				log({ kind: "error", error: String(error) });
				res.writeHead(500);
				res.end();
			}
		})();
	});
});

server.listen(port, "127.0.0.1", () => {
	const address = server.address();
	const bound = typeof address === "object" && address ? address.port : port;
	log({ kind: "listen", port: bound });
	// The fixture reads this line to learn the port.
	process.stdout.write(`MOCK_PORT=${bound}\n`);
});
