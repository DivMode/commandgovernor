/**
 * TRN — the ChatGPT foreman transport, measured on the pinned `pi-gpt` package.
 *
 * The foreman loop has no Command Governor code: the transport is the pinned
 * `pi-gpt` package and the correlation rules are prose in
 * `harness/skills/cg-foreman/SKILL.md`. This file measures what those rules
 * stand on — that the shipped package, unmodified, gives a model enough to
 * apply them — and it does so credential-free, against a mock of the ChatGPT
 * backend served by this process. The live round trip against the real account
 * is evidence in `docs/research/2026-09-04-zero-custom-code-proof.md` §6; this
 * suite is what gates a re-pin of the package.
 *
 * What is asserted, and the control that makes each a measurement:
 *
 *   TRN-000 the tarball npm serves for the pinned spec hashes to the manifest's
 *           integrity, so the modules under test are the modules a user gets.
 *   TRN-001 exact-thread binding: the package sends into the requested
 *           conversation under its current leaf, with the caller's message id,
 *           persistently, and the readback carries ids on the active branch.
 *   TRN-002 the correlation rules are decidable from the readback alone: echo
 *           present (accepted), echo absent (no verdict), a reply naming
 *           another head SHA (stale revision), and the sent message moved off
 *           the active branch (browser edit) each classify differently.
 *   TRN-003 the letters-in-delivery-id rule is load-bearing: the package's own
 *           redaction destroys an all-digit id of the same length.
 *   TRN-004 ambiguous send, no blind resend: when the backend accepts the
 *           message and then drops the connection, the package issues exactly
 *           ONE send and the readback finds the delivery id (landed); when the
 *           backend rejects it, still exactly one send and the readback does
 *           not find it (not landed). Reconciliation is by reading.
 *   TRN-005 thread drift is observable: a backend that answers from another
 *           conversation returns a different id, and the requested thread
 *           shows no delivery.
 *   TRN-006 the repository's own patch holds on the shipped tool: `gpt_chat`,
 *           run through the extension's entry point, fails instead of
 *           reporting a drifted reply as the requested thread's, and fails
 *           instead of sending when the leaf cannot be read (control: the
 *           same call succeeds when the backend behaves).
 *
 * The rule checker in this file (`classifyReply`) is the executable statement
 * of the skill's rules. It is test code, not a product component: the product
 * applies the rule through the model reading the skill.
 *
 * The package under test is the vendored, patched tree `scripts/bootstrap.sh`
 * extracts to `pins/packages/`; TRN-000 checks the committed tarball against
 * the manifest so the tree and the pin cannot drift apart. No network.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import { readPins, REPO_ROOT } from "../lib/repo.ts";

const DRIVER = join(REPO_ROOT, "conformance", "lib", "foreman-transport-driver.mjs");
const HOOKS = join(REPO_ROOT, "conformance", "lib", "foreman-transport-hooks.mjs");
const PACKAGE_SPEC = "./pins/packages/pi-gpt-0.4.3";
const PRIME_NODE_MODULES = join(REPO_ROOT, "pins", "prime-0.9.1", "node_modules");

/** A delivery id in the skill's form: base32, so it always contains letters. */
const DELIVERY_ID = "CG-D-47B3FJU5QW2EG43V";
/** The same length, digits only: the negative control for TRN-003. */
const DIGITS_ONLY_ID = "CG-D-4732345678901234";
const SENT_SHA = "d76e307ed86b1899574ba52c85b5fc0151c3ac92";
const OTHER_SHA = "66f0a254ae8d34f8db3654c62187a3acba496038";

// ── mock ChatGPT backend ────────────────────────────────────────────────────

interface MockMessage {
	readonly id: string;
	readonly author: { readonly role: "user" | "assistant" | "system" | "tool" };
	readonly content: { readonly content_type: string; readonly parts: readonly string[] };
	readonly status: string;
	readonly create_time: number;
}

interface MockNode {
	readonly id: string;
	readonly parent: string | null;
	readonly children: string[];
	readonly message: MockMessage | null;
}

interface MockThread {
	readonly conversation_id: string;
	current_node: string;
	readonly mapping: Record<string, MockNode>;
	readonly default_model_slug: string;
}

type SendScenario =
	| { readonly kind: "reply"; readonly echo: boolean; readonly sha: string }
	| { readonly kind: "accepted-then-cut" }
	| { readonly kind: "rejected" }
	| { readonly kind: "drift"; readonly into: string };

interface RecordedSend {
	readonly conversation_id?: string;
	readonly parent_message_id?: string;
	readonly history_and_training_disabled?: boolean;
	readonly messages?: readonly { readonly id?: string; readonly content?: { readonly parts?: readonly string[] } }[];
}

const threads = new Map<string, MockThread>();
const sends: RecordedSend[] = [];
let scenario: SendScenario = { kind: "reply", echo: true, sha: SENT_SHA };
let clock = 1_800_000_000;

function newThread(id: string): MockThread {
	const rootId = `${id}-root`;
	const foremanId = `${id}-foreman-turn`;
	const thread: MockThread = {
		conversation_id: id,
		current_node: foremanId,
		default_model_slug: "mock-thinking",
		mapping: {
			[rootId]: { id: rootId, parent: null, children: [foremanId], message: null },
			[foremanId]: {
				id: foremanId,
				parent: rootId,
				children: [],
				message: {
					id: foremanId,
					author: { role: "assistant" },
					content: { content_type: "text", parts: ["Paste this into the new session: ..."] },
					status: "finished_successfully",
					create_time: clock++,
				},
			},
		},
	};
	threads.set(id, thread);
	return thread;
}

function append(thread: MockThread, parent: string, message: MockMessage): void {
	thread.mapping[message.id] = { id: message.id, parent, children: [], message };
	thread.mapping[parent]?.children.push(message.id);
	thread.current_node = message.id;
}

function replyFor(userText: string, options: { echo: boolean; sha: string }): string {
	const delivery = /^CG-D:\s*(\S+)/m.exec(userText)?.[1] ?? "";
	const lines = options.echo ? [`CG-D: ${delivery}`] : ["Thanks, reviewing."];
	lines.push("VERDICT: APPROVE", `Reviewed PR #24 head ${options.sha} on base main.`);
	return lines.join("\n");
}

async function readBody(request: IncomingMessage): Promise<string> {
	const chunks: Buffer[] = [];
	for await (const chunk of request) chunks.push(chunk as Buffer);
	return Buffer.concat(chunks).toString("utf8");
}

function json(response: ServerResponse, status: number, value: unknown): void {
	response.writeHead(status, { "content-type": "application/json" });
	response.end(JSON.stringify(value));
}

async function handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
	const url = new URL(request.url ?? "/", "http://mock");
	const path = url.pathname;

	if (request.method === "POST" && path === "/backend-api/sentinel/chat-requirements") {
		await readBody(request);
		json(response, 200, { token: "mock-requirements", proofofwork: { required: false }, turnstile: { required: false } });
		return;
	}

	const detail = /^\/backend-api\/conversation\/([^/]+)$/.exec(path);
	if (request.method === "GET" && detail) {
		const thread = threads.get(detail[1]);
		if (!thread) return json(response, 404, { detail: "not found" });
		return json(response, 200, thread);
	}

	if (request.method === "POST" && path === "/backend-api/conversation") {
		const payload = JSON.parse(await readBody(request)) as RecordedSend;
		sends.push(payload);
		const requested = payload.conversation_id ?? "";
		const parent = payload.parent_message_id ?? "";
		const incoming = payload.messages?.[0];
		const userText = incoming?.content?.parts?.[0] ?? "";
		const userMessage: MockMessage = {
			id: incoming?.id ?? `user-${clock}`,
			author: { role: "user" },
			content: { content_type: "text", parts: [userText] },
			status: "finished_successfully",
			create_time: clock++,
		};

		if (scenario.kind === "rejected") return json(response, 500, { detail: "mock: rejected before recording" });

		const target = threads.get(scenario.kind === "drift" ? scenario.into : requested);
		if (!target) return json(response, 404, { detail: "unknown conversation" });
		append(target, scenario.kind === "drift" ? target.current_node : parent, userMessage);

		if (scenario.kind === "accepted-then-cut") {
			response.writeHead(200, { "content-type": "text/event-stream" });
			response.flushHeaders();
			setTimeout(() => response.socket?.destroy(), 30);
			return;
		}

		const assistant: MockMessage = {
			id: `assistant-${clock}`,
			author: { role: "assistant" },
			content: {
				content_type: "text",
				parts: [replyFor(userText, scenario.kind === "reply" ? scenario : { echo: true, sha: SENT_SHA })],
			},
			status: "finished_successfully",
			create_time: clock++,
		};
		append(target, userMessage.id, assistant);
		response.writeHead(200, { "content-type": "text/event-stream" });
		response.write(`data: ${JSON.stringify({ conversation_id: target.conversation_id, message: assistant })}\n\n`);
		response.write("data: [DONE]\n\n");
		response.end();
		return;
	}

	json(response, 404, { detail: `mock: no route for ${request.method} ${path}` });
}

// ── driver ──────────────────────────────────────────────────────────────────

interface DriverResult {
	readonly ok: boolean;
	readonly error?: string;
	readonly leaf?: string | null;
	readonly conversationId?: string | null;
	readonly text?: string;
	readonly currentNode?: string | null;
	readonly chain?: readonly { readonly id: string; readonly role: string | null; readonly status: string | null; readonly text: string }[];
	/** Messages present in the thread but not on the active branch (other branches after a browser edit). */
	readonly elsewhere?: readonly { readonly id: string; readonly role: string | null }[];
	readonly redacted?: string;
	readonly registered?: readonly string[];
	readonly result?: { readonly content?: readonly { readonly text?: string }[]; readonly details?: { readonly conversation_id?: string | null } };
}

let fixture = "";
let packageDir = "";
let server: Server;
let baseUrl = "";

/**
 * Spawned asynchronously on purpose: the mock backend runs on this process's
 * event loop, and a synchronous spawn would block it, so the driver's requests
 * would never be answered.
 */
function drive(...args: string[]): Promise<DriverResult> {
	return new Promise((resolve, reject) => {
		const child = spawn(process.execPath, ["--experimental-transform-types", "--no-warnings", "--import", HOOKS, DRIVER, ...args], {
			env: { ...process.env, CG_MOCK_BASE: baseUrl, CG_PIGPT_DIR: packageDir, CG_PRIME_NODE_MODULES: PRIME_NODE_MODULES, CODEX_HOME: join(fixture, "codex") },
			stdio: ["ignore", "pipe", "pipe"],
		});
		let stdout = "";
		let stderr = "";
		child.stdout.on("data", (chunk: Buffer) => (stdout += chunk.toString("utf8")));
		child.stderr.on("data", (chunk: Buffer) => (stderr += chunk.toString("utf8")));
		const timer = setTimeout(() => child.kill("SIGKILL"), 60_000);
		child.on("error", reject);
		child.on("close", () => {
			clearTimeout(timer);
			const text = stdout.trim();
			if (!text.startsWith("{")) return reject(new Error(`driver produced no JSON (stderr: ${stderr.slice(0, 400)})`));
			resolve(JSON.parse(text) as DriverResult);
		});
	});
}

function envelope(deliveryId: string, sha: string): string {
	return [
		`CG-D: ${deliveryId}`,
		"CG-TASK: conformance",
		`CG-REV: PR #24 head ${sha} on base main`,
		`CG-REPLY-CONTRACT: first line "CG-D: ${deliveryId}"; then VERDICT for that head`,
		"",
		"Report body.",
	].join("\n");
}

// ── the skill's rules, as a checker ─────────────────────────────────────────

type Classification =
	| { readonly status: "accepted"; readonly replyId: string; readonly sha: string }
	| { readonly status: "no_echo"; readonly replyId: string }
	| { readonly status: "stale_revision"; readonly replyId: string; readonly sha: string }
	| { readonly status: "not_on_active_branch" }
	| { readonly status: "landed_no_reply" }
	| { readonly status: "not_landed" };

function classifyReply(
	read: DriverResult,
	sent: { readonly userMessageId: string; readonly deliveryId: string; readonly sha: string },
): Classification {
	const chain = read.chain ?? [];
	const index = chain.findIndex((entry) => entry.id === sent.userMessageId);
	if (index < 0) {
		const onAnotherBranch = (read.elsewhere ?? []).some((entry) => entry.id === sent.userMessageId);
		return onAnotherBranch ? { status: "not_on_active_branch" } : { status: "not_landed" };
	}
	const reply = chain
		.slice(index + 1)
		.find((entry) => entry.role === "assistant" && entry.status === "finished_successfully" && entry.text.trim() !== "");
	if (!reply) return { status: "landed_no_reply" };
	if (!reply.text.split("\n")[0].trim().endsWith(sent.deliveryId)) return { status: "no_echo", replyId: reply.id };
	const sha = /\b([0-9a-f]{40})\b/.exec(reply.text)?.[1] ?? "";
	if (sha !== sent.sha) return { status: "stale_revision", replyId: reply.id, sha };
	return { status: "accepted", replyId: reply.id, sha };
}

// ── fixture ─────────────────────────────────────────────────────────────────

describe("TRN: ChatGPT foreman transport on the pinned pi-gpt", () => {
	before(async () => {
		fixture = mkdtempSync(join(tmpdir(), "cg-trn-"));
		mkdirSync(join(fixture, "codex"), { recursive: true });
		// pi-gpt reads its bearer from CODEX_HOME/auth.json. The value is not a
		// credential: the mock never checks it, and nothing else receives it.
		writeFileSync(
			join(fixture, "codex", "auth.json"),
			JSON.stringify({ tokens: { access_token: "cg-conformance-fixture-token-not-a-real-credential", account_id: "acct-fixture" } }),
		);

		const pinned = readPins().packages.find((entry) => entry.source === PACKAGE_SPEC);
		assert.ok(pinned, `${PACKAGE_SPEC} must be pinned in pins/pins.json`);
		const tarball = join(REPO_ROOT, String(pinned.tarball));
		assert.ok(existsSync(tarball), `the committed tarball ${String(pinned.tarball)} is missing`);
		const integrity = `sha512-${createHash("sha512").update(readFileSync(tarball)).digest("base64")}`;
		assert.equal(integrity, String(pinned.integrity), "TRN-000: the committed tarball must hash to the pinned integrity");
		packageDir = join(REPO_ROOT, PACKAGE_SPEC.slice(2));
		assert.ok(existsSync(join(packageDir, "src", "conversation.ts")), `${PACKAGE_SPEC} is not extracted; run scripts/bootstrap.sh first`);
		for (const patch of Array.isArray(pinned.patches) ? (pinned.patches as string[]) : []) {
			assert.ok(existsSync(join(REPO_ROOT, patch)), `pinned patch ${patch} is missing`);
		}
		assert.ok(
			readFileSync(join(packageDir, "extensions", "chatgpt.ts"), "utf8").includes("Command Governor guard"),
			"the extracted tree does not carry the repository's patch; bootstrap did not apply it",
		);

		server = createServer((request, response) => {
			handle(request, response).catch((error: unknown) => json(response, 500, { detail: String(error) }));
		});
		await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
		baseUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
	});

	after(async () => {
		await new Promise<void>((resolve) => server?.close(() => resolve()));
		if (fixture && !process.env.CG_KEEP_FIXTURE) rmSync(fixture, { recursive: true, force: true });
	});

	it("TRN-001: sends into the exact thread under its leaf, persistently, and reads ids back on the active branch", async () => {
		const thread = newThread("thread-exact");
		scenario = { kind: "reply", echo: true, sha: SENT_SHA };
		const before = sends.length;

		const leaf = await drive("leaf", thread.conversation_id);
		assert.equal(leaf.ok, true, leaf.error);
		assert.equal(leaf.leaf, thread.current_node, "the leaf the package binds to must be the thread's current node");

		const userMessageId = "11111111-1111-4111-8111-111111111111";
		const promptFile = join(fixture, "prompt-exact.txt");
		writeFileSync(promptFile, envelope(DELIVERY_ID, SENT_SHA));
		const sent = await drive("send", thread.conversation_id, leaf.leaf ?? "", userMessageId, promptFile);
		assert.equal(sent.ok, true, sent.error);
		assert.equal(sent.conversationId, thread.conversation_id, "the package must report the conversation it was asked for");
		assert.equal(sends.length, before + 1, "exactly one send");
		const payload = sends[sends.length - 1];
		assert.equal(payload.conversation_id, thread.conversation_id);
		assert.equal(payload.parent_message_id, leaf.leaf, "the send must hang off the bound leaf");
		assert.equal(payload.messages?.[0]?.id, userMessageId, "the caller's message id must be the one sent");
		assert.equal(payload.history_and_training_disabled, false, "a foreman thread is persistent, never temporary");
		assert.match(payload.messages?.[0]?.content?.parts?.[0] ?? "", /^CG-D: CG-D-/, "the envelope must lead the message body");

		const read = await drive("read", thread.conversation_id);
		assert.equal(read.ok, true, read.error);
		const ours = read.chain?.find((entry) => entry.id === userMessageId);
		assert.ok(ours, "our message, by our id, must be on the active branch of the readback");
		assert.match(ours.text, new RegExp(`^CG-D: ${DELIVERY_ID}`, "m"));
		const verdict = classifyReply(read, { userMessageId, deliveryId: DELIVERY_ID, sha: SENT_SHA });
		assert.equal(verdict.status, "accepted");
		assert.equal((sent.text ?? "").split("\n")[0], `CG-D: ${DELIVERY_ID}`, "the streamed reply and the readback are the same message");
	});

	it("TRN-002: echo, stale revision and branch placement are each decidable from the readback", async () => {
		const cases: { name: string; scenario: SendScenario; expect: Classification["status"]; edit?: boolean }[] = [
			{ name: "echo present", scenario: { kind: "reply", echo: true, sha: SENT_SHA }, expect: "accepted" },
			{ name: "echo absent", scenario: { kind: "reply", echo: false, sha: SENT_SHA }, expect: "no_echo" },
			{ name: "other head SHA", scenario: { kind: "reply", echo: true, sha: OTHER_SHA }, expect: "stale_revision" },
			{ name: "browser edit moved the branch", scenario: { kind: "reply", echo: true, sha: SENT_SHA }, expect: "not_on_active_branch", edit: true },
		];
		for (const [index, entry] of cases.entries()) {
			const thread = newThread(`thread-rule-${index}`);
			scenario = entry.scenario;
			const userMessageId = `22222222-2222-4222-8222-${String(index).padStart(12, "0")}`;
			const promptFile = join(fixture, `prompt-rule-${index}.txt`);
			writeFileSync(promptFile, envelope(DELIVERY_ID, SENT_SHA));
			const sent = await drive("send", thread.conversation_id, thread.current_node, userMessageId, promptFile);
			assert.equal(sent.ok, true, `${entry.name}: ${sent.error}`);
			if (entry.edit) {
				// The user edited the foreman's message in the browser: a sibling
				// branch becomes current and our message is no longer on it.
				append(thread, `${thread.conversation_id}-root`, {
					id: `${thread.conversation_id}-edited`,
					author: { role: "assistant" },
					content: { content_type: "text", parts: ["(edited turn)"] },
					status: "finished_successfully",
					create_time: clock++,
				});
			}
			const read = await drive("read", thread.conversation_id);
			assert.equal(read.ok, true, read.error);
			const verdict = classifyReply(read, { userMessageId, deliveryId: DELIVERY_ID, sha: SENT_SHA });
			assert.equal(verdict.status, entry.expect, `${entry.name}: ${JSON.stringify(verdict)}`);
		}
	});

	it("TRN-003: the package's redaction keeps a lettered delivery id and destroys an all-digit one", async () => {
		const lettered = await drive("redact", `reply: CG-D: ${DELIVERY_ID} end`);
		assert.equal(lettered.ok, true, lettered.error);
		assert.equal(lettered.redacted, `reply: CG-D: ${DELIVERY_ID} end`);
		const digits = await drive("redact", `reply: CG-D: ${DIGITS_ONLY_ID} end`);
		assert.equal(digits.ok, true, digits.error);
		assert.notEqual(digits.redacted, `reply: CG-D: ${DIGITS_ONLY_ID} end`, "negative control: digits-only ids must NOT survive, or the rule is decoration");
		assert.match(digits.redacted ?? "", /<PHONE>/);
	});

	it("TRN-004: an ambiguous send is resolved by reading, never by a second send", async () => {
		// Accepted, then the connection is cut before any reply frame.
		const landed = newThread("thread-cut");
		scenario = { kind: "accepted-then-cut" };
		let before = sends.length;
		const cutId = "33333333-3333-4333-8333-333333333333";
		writeFileSync(join(fixture, "prompt-cut.txt"), envelope("CG-D-CUTAMBIGUOUSXYZ", SENT_SHA));
		const cut = await drive("send", landed.conversation_id, landed.current_node, cutId, join(fixture, "prompt-cut.txt"));
		assert.equal(cut.ok, false, "the package must surface the failure, not invent a result");
		assert.equal(sends.length, before + 1, "exactly one send: the package did not retry on its own");
		const cutRead = await drive("read", landed.conversation_id);
		assert.equal(classifyReply(cutRead, { userMessageId: cutId, deliveryId: "CG-D-CUTAMBIGUOUSXYZ", sha: SENT_SHA }).status, "landed_no_reply");

		// Rejected before recording: the same failure shape at the caller, the opposite readback.
		const missing = newThread("thread-rejected");
		scenario = { kind: "rejected" };
		before = sends.length;
		const rejectedId = "44444444-4444-4444-8444-444444444444";
		writeFileSync(join(fixture, "prompt-rejected.txt"), envelope("CG-D-REJECTEDXYZABC", SENT_SHA));
		const rejected = await drive("send", missing.conversation_id, missing.current_node, rejectedId, join(fixture, "prompt-rejected.txt"));
		assert.equal(rejected.ok, false);
		assert.equal(sends.length, before + 1, "exactly one send");
		const rejectedRead = await drive("read", missing.conversation_id);
		assert.equal(classifyReply(rejectedRead, { userMessageId: rejectedId, deliveryId: "CG-D-REJECTEDXYZABC", sha: SENT_SHA }).status, "not_landed");
	});

	it("TRN-005: thread drift is visible in the returned id and absent from the requested thread", async () => {
		const requested = newThread("thread-requested");
		const elsewhere = newThread("thread-elsewhere");
		scenario = { kind: "drift", into: elsewhere.conversation_id };
		const driftId = "55555555-5555-4555-8555-555555555555";
		writeFileSync(join(fixture, "prompt-drift.txt"), envelope("CG-D-DRIFTCHECKABCD", SENT_SHA));
		const sent = await drive("send", requested.conversation_id, requested.current_node, driftId, join(fixture, "prompt-drift.txt"));
		assert.equal(sent.ok, true, sent.error);
		assert.notEqual(sent.conversationId, requested.conversation_id, "the package reports the backend's id, so the skill's equality check has something to catch");
		const read = await drive("read", requested.conversation_id);
		assert.equal(classifyReply(read, { userMessageId: driftId, deliveryId: "CG-D-DRIFTCHECKABCD", sha: SENT_SHA }).status, "not_landed");
	});

	it("TRN-006: the repository's patch makes gpt_chat fail on drift and on an unreadable leaf, and pass otherwise", async () => {
		// Control: the shipped tool, through the extension's own entry point,
		// succeeds when the backend answers from the requested thread.
		const thread = newThread("thread-tool-ok");
		scenario = { kind: "reply", echo: true, sha: SENT_SHA };
		const ok = await drive("tool", "gpt_chat", JSON.stringify({ prompt: envelope("CG-D-TOOLCONTROLXYZ", SENT_SHA), conversation_id: thread.conversation_id }));
		assert.equal(ok.ok, true, ok.error);
		assert.ok(ok.registered?.includes("gpt_chat"), `the extension registered ${JSON.stringify(ok.registered)}`);
		assert.equal(ok.result?.details?.conversation_id, thread.conversation_id);
		assert.match(ok.result?.content?.[0]?.text ?? "", /^CG-D: CG-D-TOOLCONTROLXYZ/);

		// Drift: the unpatched tool reports the other conversation's reply as a
		// result; the patched tool fails and names both ids.
		const requested = newThread("thread-tool-drift");
		const elsewhere = newThread("thread-tool-elsewhere");
		scenario = { kind: "drift", into: elsewhere.conversation_id };
		const drift = await drive("tool", "gpt_chat", JSON.stringify({ prompt: envelope("CG-D-TOOLDRIFTXYZAB", SENT_SHA), conversation_id: requested.conversation_id }));
		assert.equal(drift.ok, false, "a drifted reply must fail the tool call");
		assert.match(drift.error ?? "", /requested conversation thread-tool-drift.*answered from thread-tool-elsewhere/);

		// Unreadable leaf: the unpatched tool swallows the read failure and sends
		// with a fabricated parent; the patched tool fails before sending.
		const before = sends.length;
		const missing = await drive("tool", "gpt_chat", JSON.stringify({ prompt: envelope("CG-D-TOOLNOLEAFXYZ", SENT_SHA), conversation_id: "thread-does-not-exist" }));
		assert.equal(missing.ok, false, "an unreadable leaf must fail the call");
		assert.match(missing.error ?? "", /404|not sending|could not read/);
		assert.equal(sends.length, before, "nothing was sent after the leaf read failed");
	});
});
