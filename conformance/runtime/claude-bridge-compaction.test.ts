/**
 * BRIDGE-005 — Prime's own context compaction works through the vendored bridge.
 *
 * LIVE and OPT-IN, for the same reason as `live-chatgpt.test.ts`: what is under
 * test is a real Claude Code child on the user's own login, and no mock can come
 * back negative about it. Haiku only; nothing leaves the machine but the three
 * short turns and one summarisation below.
 *
 *   CG_LIVE=1 scripts/conformance.sh
 *   CG_LIVE=1 node --test conformance/runtime/claude-bridge-compaction.test.ts
 *
 * What it protects, and why it is not decorative. Compaction is how a long
 * worker survives its context window at all, and on 2026-09-05 it failed 3/3
 * under the bridge with
 *
 *   Compaction failed: prompt-capture: no capture for this 317-char system
 *   prompt, and it embeds none of the 1 known.
 *
 * and no `compaction` entry in Prime's transcript
 * (`docs/research/2026-09-05-claude-bridge-cache-measurement.md` §5). The cause
 * is a Pi-vs-Prime seam: upstream Pi marks its nested completions
 * `cacheRetention: "none"` and the bridge routes those to an isolated
 * subprocess, while Prime's `core/compaction/compaction.js` calls pi-ai's
 * `completeSimple` with `{ maxTokens, signal, apiKey, headers }` and nothing
 * else — the string `cacheRetention` does not occur anywhere in Prime 0.9.1's
 * core — so every summarisation entered the resumable provider path and threw
 * on a system prompt `before_agent_start` never recorded. The fourth seam in
 * `pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch` routes it.
 *
 * That measurement is this test's negative control: against the unpatched
 * package no `compaction` entry ever appears and the first assertion times out
 * with the failure text on the client's screen.
 *
 * Two fixture decisions, each forced by a measured fact:
 *
 * 1. **The stock interactive client, not `--mode rpc`.** Prime suspends the
 *    session input queue across a compaction exactly as it does across an
 *    abort, and only the daemon protocol exposes `resume_queue`, so the turn
 *    after a successful `/compact` in rpc mode is rejected with "Cannot admit a
 *    session action while queued session input is suspended." Measured here on
 *    2026-09-05 and filed as `docs/upstream/2026-09-05-prime-rpc-queue-suspended.md`.
 *    The interactive client recovers, and it is the client a user drives.
 * 2. **The size that makes the session compactable is in the PROMPT.** Prime
 *    refuses to compact a session that is "too short", and picks the cut by
 *    walking backwards until `compaction.keepRecentTokens` is accumulated. A
 *    fixture that grows the session by asking the MODEL for a long answer is
 *    betting on output length; this one controls the bytes itself.
 */

import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, userInfo } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import { listAgents, ptyCli, waitUntil, startRoot, type PrimeRoot, type PtyClient } from "../lib/prime.ts";
import { readPins, REPO_ROOT } from "../lib/repo.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

const reason = process.env.CG_LIVE === "1" ? undefined : "opt-in: set CG_LIVE=1 to run a real Claude Code child on the user's own login";

/** Small enough that any of these turns is already past the cut. */
const KEEP_RECENT_TOKENS = 100;
/** ~2 KB on ONE line: a newline inside a TUI prompt is Enter, not text. */
const FILLER = "filler that makes this the turn the compaction cut lands on. ".repeat(34);

interface SessionEntry {
	readonly type?: string;
	readonly summary?: string;
	readonly message?: {
		readonly role?: string;
		readonly content?: readonly { readonly type?: string; readonly text?: string }[];
	};
}

let fixture: PrimeRoot;
let client: PtyClient | undefined;

/** Prime's own durable transcript for this run, whatever it named the file. */
function sessionEntries(): SessionEntry[] {
	if (!existsSync(fixture.sessionDir)) return [];
	return readdirSync(fixture.sessionDir, { recursive: true })
		.map(String)
		.filter((name) => name.endsWith(".jsonl"))
		.flatMap((name) => readFileSync(join(fixture.sessionDir, name), "utf8").split("\n"))
		.filter(Boolean)
		.map((line) => {
			try {
				return JSON.parse(line) as SessionEntry;
			} catch {
				return {};
			}
		});
}

function assistantSaid(token: string): boolean {
	return sessionEntries().some(
		(entry) =>
			entry.type === "message" &&
			entry.message?.role === "assistant" &&
			(entry.message.content ?? []).some((block) => block.type === "text" && (block.text ?? "").includes(token)),
	);
}

/** Type one prompt and wait for the answer to land in Prime's transcript. */
async function turn(prompt: string, token: string, timeoutMs = 300_000): Promise<void> {
	await client!.submit(prompt, 1500);
	await waitUntil(() => assistantSaid(token) || undefined, timeoutMs, 1000, `the answer ${token}: ${client!.screen().slice(-400).replace(/\s+/g, " ")}`);
}

describe("BRIDGE: Prime compaction through the vendored claude-bridge", { skip: reason }, () => {
	before(async () => {
		const bridge = readPins().packages.find((entry) => String(entry.source).includes("pi-claude-agent-sdk"));
		assert.ok(bridge, "pi-claude-agent-sdk must be pinned in pins/pins.json");
		const bridgeSource = join(REPO_ROOT, String(bridge.source).replace(/^\.\//, ""));
		assert.ok(existsSync(join(bridgeSource, "node_modules")), `${String(bridge.source)} is not installed; run scripts/bootstrap.sh first`);
		assert.ok(
			readFileSync(join(bridgeSource, "src", "prompt-capture.ts"), "utf8").includes("canAccount"),
			"the extracted bridge does not carry the compaction seam; bootstrap did not apply the repository's patch",
		);

		// Real HOME and USER, as in live-chatgpt.test.ts: Claude Code finds its
		// own login by both. Prime's state still lives inside the fixture root.
		fixture = await startRoot({ label: "bridge-compaction", extraEnv: { HOME: homedir(), USER: userInfo().username } });
		// The stock clients run in `fixture.work`, so the project settings go
		// there. Prime installs a path package named in project settings on
		// startup, which is how a user's project loads the bridge. Lowering
		// keepRecentTokens is the supported way to make a short session
		// compactable: it moves where the cut lands and nothing else.
		mkdirSync(join(fixture.work, ".prime", "agent"), { recursive: true });
		writeFileSync(
			join(fixture.work, ".prime", "agent", "settings.json"),
			JSON.stringify({ packages: [bridgeSource], compaction: { keepRecentTokens: KEEP_RECENT_TOKENS } }, null, 1),
		);

		client = ptyCli(
			fixture,
			["--provider", "claude-bridge", "--model", "claude-haiku-4-5", "--session-dir", fixture.sessionDir, "-nc", "--no-themes"],
			{ name: "bridge-tui" },
		);
		// Ready means the daemon says so, not that the screen has drawn: the
		// package install on startup is what takes the time here.
		await waitUntil(
			() => listAgents(fixture).sessions.find((row) => row.workerState === "ready" && row.workerPid),
			180_000,
			500,
			`a ready session under claude-bridge: ${client.screen().slice(-400).replace(/\s+/g, " ")}`,
		);
	});

	after(async () => {
		client?.kill();
		if (fixture) assertCleanTeardown(await fixture.stop());
	});

	it("BRIDGE-005: /compact succeeds through the bridge, lands a compaction entry, and the session continues", async () => {
		// Two turns: one for the summariser to summarise, one to keep.
		await turn("Reply with exactly: BRIDGE005-T1", "BRIDGE005-T1");
		await turn(`${FILLER} Ignore the filler. Reply with exactly: BRIDGE005-T2`, "BRIDGE005-T2");

		await client!.submit("/compact", 1500);
		// The durable record is the assertion. A failed compaction writes no
		// entry at all, so this is what times out when the seam regresses — the
		// client's own error text comes back with it.
		await waitUntil(
			() => sessionEntries().some((entry) => entry.type === "compaction") || undefined,
			300_000,
			1000,
			`a compaction entry in Prime's transcript: ${client!.screen().slice(-900).replace(/\s+/g, " ")}`,
		);

		const compactions = sessionEntries().filter((entry) => entry.type === "compaction");
		assert.equal(compactions.length, 1, `expected exactly one compaction entry, found ${compactions.length}`);
		assert.equal(typeof compactions[0].summary, "string", "the compaction entry carries no summary");
		assert.ok(String(compactions[0].summary).length > 0, "the compaction entry's summary is empty");

		// And the session keeps working across the history Prime just rewrote —
		// the bridge's `session_compact` → REBUILD path.
		await turn("Reply with exactly: BRIDGE005-OK", "BRIDGE005-OK");
	});
});
