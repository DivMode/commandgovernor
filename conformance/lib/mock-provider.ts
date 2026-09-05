/**
 * Mock OpenAI-compatible chat-completions server for the runtime tier.
 *
 * Deterministic, credential-free, and every request AND every response
 * decision is logged to a JSONL file, so "did the runtime ask the model
 * again?" and "did it issue the tool again?" are answered from a file rather
 * than inferred. A counter that cannot see a duplicate proves nothing, so the
 * runtime suite always pairs an effect-once assertion with a negative control
 * measured through this same log.
 *
 * Behaviour is selected by the LAST user message text. The markers are matched
 * ANYWHERE in that text, not anchored: `prime-agent send` and scheduled
 * prompts wrap the user's words in a header, and an anchored match would fall
 * through to the default answer and silently measure nothing.
 *
 *   TOOL:<name>|<json args>  -> emit one tool_call for <name>; when the tool
 *                               result comes back, close the turn ("tool-done")
 *   ECHO:<text>              -> stream <text> back, end_turn
 *   SLOW:<n>:<ms>            -> stream n chunks, one every ms, then end
 *   anything else            -> "ok"
 *
 * `TOOL:` is what makes a REAL external effect reachable from a stock client:
 * Prime executes the tool inside the worker's own agent loop, so the effect is
 * produced by the product path rather than by the test. It costs Prime's
 * Python kernel bootstrap (uv + CPython into `<agentDir>/kernel-venv`, network
 * on first use), which is why only the runtime tier uses it.
 *
 * `MOCK_DUMP_TOOLS=1` additionally records the full `tools` array of the first
 * request, which is how a tool name and schema are discovered.
 *
 * Run as a script: `node conformance/lib/mock-provider.ts` with MOCK_PORT
 * (0 = ephemeral) and MOCK_LOG in the environment.
 */

import { appendFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";

const port = Number(process.env.MOCK_PORT ?? 0);
const logPath = process.env.MOCK_LOG;
if (!logPath) {
	console.error("mock-provider: MOCK_LOG is required");
	process.exit(2);
}
const logFile: string = logPath;
let seq = 0;
let dumpedTools = false;

function log(entry: Record<string, unknown>): void {
	appendFileSync(logFile, `${JSON.stringify({ ts: new Date().toISOString(), ...entry })}\n`);
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

interface ChatMessage {
	role?: string;
	content?: unknown;
}

interface ChatBody {
	model?: string;
	messages?: ChatMessage[];
	stream?: boolean;
	tools?: { function?: { name?: string } }[];
}

function textOf(content: unknown): string {
	if (typeof content === "string") return content;
	if (Array.isArray(content)) return content.map((part) => (part as { text?: string }).text ?? "").join("");
	return "";
}

function roleText(messages: ChatMessage[], role: string): string[] {
	return messages.filter((message) => message?.role === role).map((message) => textOf(message.content));
}

function lastUser(messages: ChatMessage[]): string {
	for (let i = messages.length - 1; i >= 0; i -= 1) {
		const message = messages[i];
		if (message?.role === "user") return textOf(message.content);
	}
	return "";
}

function sse(res: ServerResponse, payload: unknown): void {
	res.write(`data: ${JSON.stringify(payload)}\n\n`);
}

async function handleChat(res: ServerResponse, body: ChatBody): Promise<void> {
	const id = `chatcmpl-${++seq}`;
	const model = body.model ?? "mock";
	const messages = body.messages ?? [];
	const text = lastUser(messages);
	const toolResults = messages.filter((message) => message?.role === "tool").length;
	log({
		kind: "request",
		id,
		model,
		stream: !!body.stream,
		lastUser: text.slice(0, 300),
		nMessages: messages.length,
		roles: messages.map((message) => message?.role ?? "?").join(","),
		toolResults,
		toolNames: (body.tools ?? []).map((tool) => tool?.function?.name),
		// The full system prompt and every user turn, because several things a
		// black-box test must observe are only visible there: the skills and
		// commands a package registered, and the text Prime feeds back into an
		// autonomous continuation when a gate fails.
		system: roleText(messages, "system").join("\n"),
		userMessages: roleText(messages, "user").map((entry) => entry.slice(0, 4000)),
	});
	if (process.env.MOCK_DUMP_TOOLS && !dumpedTools) {
		dumpedTools = true;
		log({ kind: "tools-dump", id, tools: body.tools ?? [] });
	}

	res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
	const base = { id, object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model };
	const chunk = (delta: Record<string, unknown>, finish: string | null = null) =>
		sse(res, { ...base, choices: [{ index: 0, delta, finish_reason: finish }] });
	const usage = () => sse(res, { ...base, choices: [], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } });
	const done = (mode: string, extra: Record<string, unknown> = {}) => {
		usage();
		res.end("data: [DONE]\n\n");
		log({ kind: "response", id, mode, ...extra });
	};

	chunk({ role: "assistant", content: "" });

	// A tool result already came back: close the turn so the worker goes idle.
	if (toolResults > 0 && messages[messages.length - 1]?.role === "tool") {
		chunk({ content: "tool-done" });
		chunk({}, "stop");
		done("after-tool");
		return;
	}

	let match: RegExpMatchArray | null;
	if ((match = text.match(/TOOL:([A-Za-z0-9_]+)\|(\{.*\})/s))) {
		const name = match[1].trim();
		const args = match[2];
		chunk({ tool_calls: [{ index: 0, id: `call_${id}`, type: "function", function: { name, arguments: "" } }] });
		chunk({ tool_calls: [{ index: 0, function: { arguments: args } }] });
		chunk({}, "tool_calls");
		done("tool_call", { tool: name, args: args.slice(0, 300) });
		return;
	}
	if ((match = text.match(/ECHO:(.*)$/s))) {
		chunk({ content: match[1] });
		chunk({}, "stop");
		done("echo");
		return;
	}
	if ((match = text.match(/SLOW:(\d+):(\d+)/))) {
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
		done("slow", { n });
		return;
	}
	chunk({ content: "ok" });
	chunk({}, "stop");
	done("default");
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
					await handleChat(res, JSON.parse(buffer || "{}") as ChatBody);
					return;
				}
				log({ kind: "unhandled", method: req.method, url: req.url });
				res.writeHead(404);
				res.end();
			} catch (error) {
				log({ kind: "error", error: String(error) });
				try {
					res.writeHead(500);
					res.end();
				} catch {
					/* the client is already gone; the log entry above is the record */
				}
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
