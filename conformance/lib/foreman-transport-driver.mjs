// Child-process driver for conformance/runtime/foreman-transport.test.ts.
//
// Runs the PINNED pi-gpt package's own modules, unmodified, against a mock of
// chatgpt.com that the test process serves. Two things are done here and not
// in the test file, and both are the reason this is a separate process:
//
//   1. pi-gpt hardcodes `https://chatgpt.com`. The only black-box way to point
//      it at a mock is to substitute the global `fetch` it calls, and that
//      substitution must live in the process that imports the package, not in
//      the process that asserts on it.
//   2. pi-gpt's TypeScript uses a constructor parameter property, which Node's
//      default strip-only loader rejects; the parent spawns this file with
//      `--experimental-transform-types`, leaving the suite's own loader alone.
//
// Nothing here re-implements a transport rule. Commands map one-to-one onto the
// package's public surface: `leaf` -> leafMessageId, `send` -> complete,
// `read` -> the same GET the package's gpt_get_conversation issues, `redact`
// -> its output redaction. Output is one JSON object on stdout.

import { readFileSync } from "node:fs";

const base = process.env.CG_MOCK_BASE;
const packageDir = process.env.CG_PIGPT_DIR;
if (!base || !packageDir) {
	process.stdout.write(JSON.stringify({ ok: false, error: "CG_MOCK_BASE and CG_PIGPT_DIR are required" }));
	process.exit(2);
}

const realFetch = globalThis.fetch;
globalThis.fetch = (input, init) => {
	const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
	return realFetch(url.replace(/^https:\/\/chatgpt\.com/, base), init);
};

const { BackendClient } = await import(`${packageDir}/src/client.ts`);
const { ConversationClient } = await import(`${packageDir}/src/conversation.ts`);
const { redact } = await import(`${packageDir}/src/redact.ts`);

const [command, ...args] = process.argv.slice(2);
const emit = (value) => process.stdout.write(JSON.stringify(value));

try {
	const backend = new BackendClient();
	const conversation = new ConversationClient(backend);
	switch (command) {
		case "leaf": {
			emit({ ok: true, leaf: await conversation.leafMessageId(args[0]) });
			break;
		}
		case "send": {
			const [conversationId, parentMessageId, userMessageId, promptFile] = args;
			const content = readFileSync(promptFile, "utf8");
			const result = await conversation.complete("mock-thinking", [{ role: "user", content, id: userMessageId }], {
				conversationId,
				parentMessageId,
				temporary: false,
				thinkingEffort: "extended",
			});
			emit({ ok: true, conversationId: result.conversationId, text: result.text });
			break;
		}
		case "read": {
			const detail = await backend.get(`/backend-api/conversation/${args[0]}`);
			const mapping = detail?.mapping ?? {};
			const chain = [];
			let cursor = detail?.current_node;
			while (cursor && mapping[cursor]) {
				const node = mapping[cursor];
				if (node.message) {
					chain.push({
						id: node.message.id,
						role: node.message.author?.role ?? null,
						status: node.message.status ?? null,
						text: (node.message.content?.parts ?? []).filter((part) => typeof part === "string").join("\n"),
					});
				}
				cursor = node.parent;
			}
			chain.reverse();
			const onBranch = new Set(chain.map((entry) => entry.id));
			const elsewhere = Object.values(mapping)
				.filter((node) => node.message && !onBranch.has(node.message.id))
				.map((node) => ({ id: node.message.id, role: node.message.author?.role ?? null }));
			emit({ ok: true, conversationId: detail?.conversation_id ?? null, currentNode: detail?.current_node ?? null, chain, elsewhere });
			break;
		}
		case "redact": {
			emit({ ok: true, redacted: redact(args[0]) });
			break;
		}
		case "tool": {
			// Run one of the extension's registered tools exactly as Prime would
			// call it, through the extension's own entry point. `pi` is a stand-in
			// that records registrations and ignores everything else.
			const [toolName, argsJson] = args;
			const registered = new Map();
			const pi = new Proxy(
				{ registerTool: (tool) => registered.set(tool.name, tool) },
				{ get: (target, key) => (key in target ? target[key] : () => undefined) },
			);
			const extension = await import(`${packageDir}/extensions/chatgpt.ts`);
			extension.default(pi);
			const tool = registered.get(toolName);
			if (!tool) throw new Error(`extension did not register ${toolName}; it registered ${[...registered.keys()].join(",")}`);
			const result = await tool.execute("call-1", JSON.parse(argsJson), undefined, undefined, { cwd: process.cwd() });
			emit({ ok: true, registered: [...registered.keys()], result });
			break;
		}
		default:
			emit({ ok: false, error: `unknown command ${command}` });
			process.exitCode = 2;
	}
} catch (error) {
	emit({ ok: false, error: String(error?.message ?? error) });
	process.exitCode = 1;
}
