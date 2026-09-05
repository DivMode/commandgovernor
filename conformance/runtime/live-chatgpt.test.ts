/**
 * LIVE — the user's subscriptions inside a real Prime worker: ChatGPT Web
 * through the vendored pi-gpt, and Claude through the vendored pi-claude-agent-sdk
 * (the real Claude Code binary on its own login). OPT-IN; never a merge gate.
 *
 * Everything else in this suite is credential-free and blind, by design, to
 * the one thing that will actually break this transport: the provider. pi-gpt
 * drives undocumented endpoints with pinned client build strings and solves
 * security-control checks the provider can change at any time. TRN measures
 * the package against a mock and cannot come back negative when chatgpt.com
 * changes. This file can. It runs only when the user says so:
 *
 *   CG_LIVE=1 scripts/conformance.sh          (needs ~/.codex/auth.json)
 *   CG_LIVE=1 node --test conformance/runtime/live-chatgpt.test.ts
 *
 * What it does, and what it leaves behind: nothing. The scripted model makes
 * Prime's worker call `gpt_account_status` (the read path: token, GET, no
 * security-control tokens) and `gpt_chat` into a TEMPORARY chat (the send
 * path: sentinel, proof-of-work, turnstile, SSE), which ChatGPT does not keep.
 * Optionally, with CG_FOREMAN_THREAD=<conversation id>, it also reads the
 * exact thread through `gpt_get_conversation`, which is the read the
 * cg-foreman skill relies on. No message is ever sent into that thread here.
 *
 * LIVE-004 asks Prime for a Claude model through `claude-bridge`: the bridge
 * starts the real Claude Code binary with every inherited Anthropic variable
 * stripped and no credential from Prime, so Claude Code uses its own login
 * (the Max plan, plan-billed). That is the only Claude path this product uses;
 * an API key is never configured. The fixture therefore keeps the user's real
 * HOME for this file only, so the child can see that login.
 *
 * Everything runs inside the Prime worker, so what is measured is the product
 * path: package loaded by Prime, tool or provider executed by Prime, result
 * recorded in the session transcript or printed by the stock client.
 */

import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, userInfo } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import { startRoot, type PrimeRoot } from "../lib/prime.ts";
import { readPins, REPO_ROOT } from "../lib/repo.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

const codexHome = process.env.CODEX_HOME ?? join(homedir(), ".codex");
const enabled = process.env.CG_LIVE === "1";
const loggedIn = existsSync(join(codexHome, "auth.json"));
const reason = !enabled ? "opt-in: set CG_LIVE=1 to run against the real ChatGPT account" : !loggedIn ? `CG_LIVE=1 but no Codex login at ${codexHome}/auth.json` : undefined;
const thread = process.env.CG_FOREMAN_THREAD;

/**
 * Extension discovery stays ON (no `-ne`): the tools under test are the
 * package's. `-nc` stays because context-file discovery walks up out of the
 * fixture.
 */
const FLAGS = ["--provider", "mock", "--model", "mock-1", "-nc", "--no-themes"];

const PROBE_TOKEN = `PONG-CG-${Date.now().toString(36).toUpperCase()}`;

interface TranscriptEntry {
	readonly type?: string;
	readonly message?: {
		readonly role?: string;
		readonly toolName?: string;
		readonly isError?: boolean;
		readonly content?: readonly { readonly type?: string; readonly text?: string }[];
		readonly details?: Record<string, unknown>;
	};
}

let fixture: PrimeRoot;
let project = "";

function toolResults(): TranscriptEntry[] {
	const files = readdirSync(fixture.sessionDir, { recursive: true }).map(String).filter((name) => name.endsWith(".jsonl"));
	return files
		.flatMap((name) => readFileSync(join(fixture.sessionDir, name), "utf8").split("\n"))
		.filter(Boolean)
		.map((line) => {
			try {
				return JSON.parse(line) as TranscriptEntry;
			} catch {
				return {};
			}
		})
		.filter((entry) => entry.type === "message" && entry.message?.role === "toolResult");
}

function runTool(name: string, args: Record<string, unknown>): TranscriptEntry {
	const seen = toolResults().length;
	const result = fixture.cli(["-p", ...FLAGS, "--session-dir", fixture.sessionDir, `TOOL:${name}|${JSON.stringify(args)}`], {
		timeout: 300_000,
		cwd: project,
	});
	assert.equal(result.status, 0, `prime-agent -p exited ${result.status}: ${result.stdout}${result.stderr}`);
	const fresh = toolResults().slice(seen).filter((entry) => entry.message?.toolName === name);
	assert.equal(fresh.length, 1, `expected exactly one ${name} result in the transcript, found ${fresh.length}`);
	return fresh[0];
}

function textOf(entry: TranscriptEntry): string {
	return (entry.message?.content ?? []).map((part) => part.text ?? "").join("\n");
}

describe("LIVE: ChatGPT Web inside a Prime worker through the pinned pi-gpt", { skip: reason }, () => {
	before(async () => {
		const pinned = readPins().packages.find((entry) => String(entry.source).includes("pi-gpt"));
		assert.ok(pinned, "pi-gpt must be pinned");
		const source = join(REPO_ROOT, String(pinned.source).replace(/^\.\//, ""));
		assert.ok(existsSync(join(source, "package.json")), `${String(pinned.source)} is not extracted; run scripts/bootstrap.sh first`);

		// The fixture owns HOME; the Codex login is handed to every Prime process
		// explicitly, which is also how a user would point pi-gpt at a login that
		// is not under their HOME.
		// Real HOME and USER on purpose (see the header): Prime's own state still
		// lives under the fixture (agent dir, sessions, socket). Claude Code finds
		// its macOS Keychain login by HOME and USER, measured: with USER missing
		// it reports "Not logged in" even when HOME is right.
		fixture = await startRoot({ label: "live-subscriptions", extraEnv: { CODEX_HOME: codexHome, HOME: homedir(), USER: userInfo().username } });
		project = join(fixture.root, "project");
		mkdirSync(project, { recursive: true });
		writeFileSync(join(project, "README.md"), "# live probe project\n");
		const install = fixture.cli(["package", "install", "--local", source], { timeout: 600_000, cwd: project, withoutSocket: true });
		assert.equal(install.status, 0, `${install.stdout}${install.stderr}`);
		const bridge = readPins().packages.find((entry) => String(entry.source).includes("pi-claude-agent-sdk"));
		assert.ok(bridge, "pi-claude-agent-sdk must be pinned");
		const bridgeSource = join(REPO_ROOT, String(bridge.source).replace(/^\.\//, ""));
		assert.ok(existsSync(join(bridgeSource, "node_modules")), `${String(bridge.source)} has no dependencies; run scripts/bootstrap.sh first`);
		const bridgeInstall = fixture.cli(["package", "install", "--local", bridgeSource], { timeout: 600_000, cwd: project, withoutSocket: true });
		assert.equal(bridgeInstall.status, 0, `${bridgeInstall.stdout}${bridgeInstall.stderr}`);
	});

	after(async () => {
		if (fixture) assertCleanTeardown(await fixture.stop());
	});

	it("LIVE-001: the read path works from inside the worker (gpt_account_status)", () => {
		const entry = runTool("gpt_account_status", {});
		assert.equal(entry.message?.isError, false, textOf(entry));
		const text = textOf(entry);
		assert.match(text, /plan|account|email/i, `unexpected account status text: ${text.slice(0, 300)}`);
	});

	it("LIVE-002: the send path works from inside the worker (gpt_chat into a temporary chat)", () => {
		const entry = runTool("gpt_chat", {
			prompt: `Command Governor transport probe. Reply with exactly: ${PROBE_TOKEN}`,
			temporary: true,
			intelligence: "instant",
		});
		assert.equal(entry.message?.isError, false, textOf(entry));
		assert.ok(textOf(entry).includes(PROBE_TOKEN), `ChatGPT did not echo the probe token: ${textOf(entry).slice(0, 300)}`);
		assert.match(String(entry.message?.details?.conversation_id ?? ""), /^[0-9a-f-]{36}$/, "no conversation id came back");
	});

	it("LIVE-003: the exact foreman thread is readable from inside the worker (gpt_get_conversation)", { skip: thread ? undefined : "set CG_FOREMAN_THREAD=<conversation id> to read the exact thread" }, () => {
		const entry = runTool("gpt_get_conversation", { conversation_id: thread, max_messages: 3 });
		assert.equal(entry.message?.isError, false, textOf(entry));
		const details = entry.message?.details as { messages?: { id?: string; role?: string }[] } | undefined;
		assert.ok((details?.messages?.length ?? 0) > 0, "the thread came back with no messages");
		assert.ok(details?.messages?.every((message) => typeof message.id === "string" && message.id.length > 0), "messages must carry ids; correlation depends on them");
	});

	it("LIVE-004: a Claude model answers through claude-bridge on Claude Code's own login, with no credential in Prime", () => {
		const authPath = join(fixture.agentDir, "auth.json");
		const auth = existsSync(authPath) ? (JSON.parse(readFileSync(authPath, "utf8")) as Record<string, unknown>) : {};
		assert.equal(auth.anthropic, undefined, "Prime must hold no Anthropic credential: the point is that Claude Code brings its own");
		const token = `ZEBRA-${Date.now().toString(36).toUpperCase()}`;
		const result = fixture.cli(
			["-p", ...FLAGS, "--no-session", "--provider", "claude-bridge", "--model", "claude-haiku-4-5", `Reply with exactly the text ${token} and nothing else`],
			{ timeout: 180_000, cwd: project },
		);
		assert.equal(result.status, 0, `prime-agent exited ${result.status}: ${result.stderr.slice(0, 500)}`);
		assert.ok(result.stdout.includes(token), `Claude did not answer through the bridge: ${result.stdout.slice(0, 300)} ${result.stderr.slice(0, 300)}`);
	});
});
