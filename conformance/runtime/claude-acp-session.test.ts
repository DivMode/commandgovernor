/**
 * ACP-SESSION — Claude owns the conversation, and Prime never rebuilds it.
 *
 * This is the product claim the whole `claude-acp` provider exists to make, and
 * it is a claim about the WIRE: what Command Governor sends to Claude, turn by
 * turn. So it is measured on the wire. A stub ACP agent
 * (`conformance/lib/claude-acp-stub-agent.mjs`) stands in for
 * `@agentclientprotocol/claude-agent-acp` — same methods, same response shapes —
 * and records every request the client makes. Everything else is real: the
 * pinned Prime, the stock `--mode rpc` client, and the shipped extension.
 *
 * No model is called, no credential is used, and nothing leaves the machine.
 * That is not a compromise: none of these invariants are about what Claude
 * answers, and a real model would make the assertions slower and flakier
 * without making them stronger. What a real model DOES prove — that the session
 * genuinely carries context across a restart — is measured separately in the
 * live proof recorded on the pull request.
 *
 *   ACP-101 one Claude session id across several turns in one process, and each
 *           turn carries ONLY its own new prompt. Control: the earlier turns'
 *           text appears in Prime's own history, so its absence on the wire is
 *           a decision rather than an accident.
 *   ACP-102 a Prime compaction does not rewrite or re-open Claude's session and
 *           does not resend anything: same session id, next prompt is only the
 *           next prompt.
 *   ACP-103 an aborted turn maps to `session/cancel`, and the session survives
 *           it — the next turn continues on the same session id with no
 *           `session/new`.
 *   ACP-104 after a process restart the session is restored NATIVELY by id
 *           (`session/resume`, or `session/load`), never rebuilt: no
 *           `session/new`, and no earlier turn's text on the wire.
 *   ACP-105 `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` reaches the adapter PROCESS.
 *           ACP-005 proves the env builder sets it; this proves a real child was
 *           started with it.
 *   ACP-106 with no interactive surface, a permission request is REJECTED.
 *           A headless run has no user, and "ask" must not degrade to "yes".
 *   ACP-107 an adapter reporting that an API key would pay for the turn is
 *           refused, with the rule named. Control: the same run with the
 *           subscription identity succeeds.
 */

import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import { sleep, startRoot, waitUntil, type PrimeRoot } from "../lib/prime.ts";
import { REPO_ROOT } from "../lib/repo.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

const EXTENSION_ENTRY = join(REPO_ROOT, "harness", "extensions", "claude-acp", "src", "index.ts");
const STUB_AGENT = join(REPO_ROOT, "conformance", "lib", "claude-acp-stub-agent.mjs");
const MODEL = ["--provider", "claude-acp", "--model", "claude-haiku-4-5"];
/** Discovery off: this file loads exactly one extension, by path, on purpose. */
const FLAGS = ["-e", EXTENSION_ENTRY, "-ne", "-ns", "-np", "-nc", "--no-themes", ...MODEL];

interface StubEntry {
	readonly pid: number;
	readonly at: number;
	readonly kind: string;
	readonly method?: string;
	readonly params?: { readonly sessionId?: string; readonly prompt?: { type?: string; text?: string }[]; readonly [key: string]: unknown };
	readonly answer?: { readonly outcome?: { readonly outcome?: string; readonly optionId?: string } };
}

let fixture: PrimeRoot;
let project = "";
let logCounter = 0;

/** A fresh recording log, so each scenario reads only its own wire traffic. */
function newLog(): string {
	const path = join(fixture.root, `acp-stub-${++logCounter}.jsonl`);
	writeFileSync(path, "");
	return path;
}

function readLog(path: string): StubEntry[] {
	if (!existsSync(path)) return [];
	return readFileSync(path, "utf8")
		.split("\n")
		.filter(Boolean)
		.map((line) => JSON.parse(line) as StubEntry);
}

const methods = (entries: readonly StubEntry[]): string[] => entries.filter((entry) => entry.kind === "request").map((entry) => String(entry.method));
const sessionIds = (entries: readonly StubEntry[]): string[] => [
	...new Set(entries.filter((entry) => entry.kind === "request" && typeof entry.params?.sessionId === "string").map((entry) => String(entry.params?.sessionId))),
];
const prompts = (entries: readonly StubEntry[]): string[] =>
	entries
		.filter((entry) => entry.kind === "request" && entry.method === "session/prompt")
		.map((entry) => (entry.params?.prompt ?? []).map((part) => part.text ?? "").join(""));

/** Environment for one scenario's Prime run. */
function stubEnv(log: string, extra: Record<string, string> = {}): Record<string, string> {
	return { CG_CLAUDE_ACP_ADAPTER: STUB_AGENT, CG_ACP_STUB_LOG: log, ...extra };
}

/**
 * Drive one stock `--mode rpc` client through a scripted sequence.
 *
 * `--mode rpc` is a stock Prime client, so everything asserted here is
 * reachable by a user. The script is a list of commands to write and how long
 * to let each settle; the whole stdout is returned for the negative controls.
 */
async function driveRpc(options: {
	readonly log: string;
	readonly sessionDir: string;
	readonly extraEnv?: Record<string, string>;
	readonly script: readonly { command: Record<string, unknown>; settleMs?: number }[];
}): Promise<string> {
	const child = fixture.cliSpawn(["--mode", "rpc", ...FLAGS, "--session-dir", options.sessionDir], {
		cwd: project,
		extraEnv: stubEnv(options.log, options.extraEnv),
	});
	let out = "";
	child.stdout?.on("data", (data: Buffer) => (out += data.toString("utf8")));
	child.stderr?.on("data", (data: Buffer) => (out += data.toString("utf8")));
	try {
		await sleep(4000);
		for (const step of options.script) {
			child.stdin?.write(`${JSON.stringify(step.command)}\n`);
			await sleep(step.settleMs ?? 6000);
		}
	} finally {
		try {
			child.kill("SIGKILL");
		} catch {
			/* already gone */
		}
		await sleep(1200);
	}
	return out;
}

describe("ACP-SESSION: Claude owns the conversation; Prime never rebuilds it", () => {
	before(async () => {
		fixture = await startRoot({ label: "claude-acp" });
		project = join(fixture.root, "project");
		mkdirSync(project, { recursive: true });
		writeFileSync(join(project, "README.md"), "# conformance scratch project\n");
		assert.ok(existsSync(EXTENSION_ENTRY), `${EXTENSION_ENTRY} is missing`);
		assert.ok(existsSync(STUB_AGENT), `${STUB_AGENT} is missing`);
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("ACP-101: one Claude session across turns, and each turn sends only its own prompt", async () => {
		const log = newLog();
		const sessions = join(fixture.root, "s101");
		await driveRpc({
			log,
			sessionDir: sessions,
			script: [{ command: { id: "t1", type: "prompt", message: "ACP-T1-ALPHA" } }, { command: { id: "t2", type: "prompt", message: "ACP-T2-BETA" } }],
		});
		const entries = readLog(log);
		fixture.note("ACP-101 methods:", JSON.stringify(methods(entries)));

		assert.ok(methods(entries).includes("session/new"), `no session was created: ${JSON.stringify(methods(entries))}`);
		assert.equal(methods(entries).filter((method) => method === "session/new").length, 1, "a second session/new means a new conversation was started mid-conversation");
		assert.equal(sessionIds(entries).length, 1, `more than one Claude session id was used: ${JSON.stringify(sessionIds(entries))}`);

		const sent = prompts(entries);
		assert.deepEqual(sent, ["ACP-T1-ALPHA", "ACP-T2-BETA"], `each turn must carry only its own new prompt; the wire said ${JSON.stringify(sent)}`);

		// Control: Prime's own transcript DOES hold the first turn, so its absence
		// from turn 2's prompt is a decision this code made, not an artefact of
		// the first turn never having existed.
		const transcript = readFileSync(join(sessions, findSessionFile(sessions)), "utf8");
		assert.match(transcript, /ACP-T1-ALPHA/, "Prime's own session does not contain turn 1; the assertion above proves nothing");
	});

	it("ACP-102: a Prime compaction neither reopens Claude's session nor resends history", async () => {
		const log = newLog();
		const sessions = join(fixture.root, "s102");
		await driveRpc({
			log,
			sessionDir: sessions,
			script: [
				{ command: { id: "t1", type: "prompt", message: "ACP-C1-ALPHA" } },
				{ command: { id: "c", type: "compact" }, settleMs: 8000 },
				{ command: { id: "t2", type: "prompt", message: "ACP-C2-BETA" } },
			],
		});
		const entries = readLog(log);
		fixture.note("ACP-102 methods:", JSON.stringify(methods(entries)));

		assert.equal(methods(entries).filter((method) => method === "session/new").length, 1, "the compaction opened a second Claude session");
		assert.equal(methods(entries).filter((method) => method === "session/load" || method === "session/resume").length, 0, "the compaction re-opened Claude's session");
		assert.equal(sessionIds(entries).length, 1, `the compaction changed the Claude session id: ${JSON.stringify(sessionIds(entries))}`);
		assert.deepEqual(prompts(entries), ["ACP-C1-ALPHA", "ACP-C2-BETA"], "the turn after a compaction must still carry only its own prompt");
	});

	it("ACP-103: an abort is session/cancel, and the session survives it", async () => {
		const log = newLog();
		const sessions = join(fixture.root, "s103");
		await driveRpc({
			log,
			sessionDir: sessions,
			// The stub holds the first prompt open long enough to abort it.
			extraEnv: { CG_ACP_STUB_DELAY_MS: "20000" },
			script: [
				{ command: { id: "t1", type: "prompt", message: "ACP-A1-ALPHA" }, settleMs: 4000 },
				{ command: { id: "x", type: "abort" }, settleMs: 6000 },
				{ command: { id: "t2", type: "prompt", message: "ACP-A2-BETA" }, settleMs: 26000 },
			],
		});
		const entries = readLog(log);
		fixture.note("ACP-103 methods:", JSON.stringify(methods(entries)));

		assert.ok(methods(entries).includes("session/cancel"), `Prime's abort did not reach the agent as session/cancel: ${JSON.stringify(methods(entries))}`);
		assert.equal(methods(entries).filter((method) => method === "session/new").length, 1, "the abort rotated the Claude session; it must survive untouched");
		assert.equal(sessionIds(entries).length, 1, `the abort changed the Claude session id: ${JSON.stringify(sessionIds(entries))}`);
		const sent = prompts(entries);
		assert.ok(sent.includes("ACP-A2-BETA"), `the turn after the abort never reached the agent: ${JSON.stringify(sent)}`);
		assert.equal(sent.filter((text) => text === "ACP-A1-ALPHA").length, 1, `the aborted prompt was resent: ${JSON.stringify(sent)}`);
	});

	it("ACP-104: after a restart the session is restored by id, never rebuilt", async () => {
		const log = newLog();
		const sessions = join(fixture.root, "s104");
		await driveRpc({ log, sessionDir: sessions, script: [{ command: { id: "t1", type: "prompt", message: "ACP-R1-ALPHA" } }] });
		const first = readLog(log);
		const firstSession = sessionIds(first)[0];
		assert.ok(firstSession, "the first run created no Claude session");

		// A genuinely separate process, continuing the same Prime session.
		const secondLog = newLog();
		const child = fixture.cliSpawn(["--mode", "rpc", "-c", ...FLAGS, "--session-dir", sessions], { cwd: project, extraEnv: stubEnv(secondLog) });
		try {
			await sleep(4000);
			child.stdin?.write(`${JSON.stringify({ id: "t2", type: "prompt", message: "ACP-R2-BETA" })}\n`);
			await waitUntil(() => (readLog(secondLog).some((entry) => entry.method === "session/prompt") ? true : undefined), 90_000, 500, "the restarted process to send its turn");
		} finally {
			try {
				child.kill("SIGKILL");
			} catch {
				/* already gone */
			}
			await sleep(1200);
		}

		const second = readLog(secondLog);
		fixture.note("ACP-104 restart methods:", JSON.stringify(methods(second)));
		assert.ok(
			methods(second).includes("session/resume") || methods(second).includes("session/load"),
			`the restarted process did not reattach to Claude's session: ${JSON.stringify(methods(second))}`,
		);
		assert.equal(methods(second).filter((method) => method === "session/new").length, 0, "the restart minted a new Claude session instead of reattaching");
		assert.deepEqual(sessionIds(second), [firstSession], "the restarted process reattached to a different session id");
		assert.deepEqual(prompts(second), ["ACP-R2-BETA"], `the restart replayed history: ${JSON.stringify(prompts(second))}`);
	});

	it("ACP-105: the long-context switch reaches the adapter process itself", async () => {
		const log = newLog();
		const sessions = join(fixture.root, "s105");
		// The stub records its own environment at start-up, which is the only place
		// a test can read the environment a real child of the shipped spawn path
		// was given. The Prime process is deliberately poisoned first, so the
		// stripping assertions below always have something to strip — otherwise
		// they would pass vacuously on a machine that exports none of it.
		await driveRpc({
			log,
			sessionDir: sessions,
			extraEnv: {
				CG_ACP_STUB_RECORD_ENV: "1",
				CLAUDECODE: "1",
				CLAUDE_CODE_ENTRYPOINT: "cli",
				ANTHROPIC_BASE_URL: "https://poison.example",
				CLAUDE_CODE_USE_BEDROCK: "1",
			},
			script: [{ command: { id: "t1", type: "prompt", message: "ACP-E1" } }],
		});
		const env = readLog(log).find((entry) => entry.kind === "child_env");
		assert.ok(env, "the stub agent recorded no environment; the recording hook did not run");
		const observed = (env as unknown as { env?: Record<string, string> }).env ?? {};
		assert.equal(observed.CLAUDE_CODE_DISABLE_1M_CONTEXT, "1", "the adapter process was started without the long-context switch");
		for (const key of ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK"]) {
			assert.equal(observed[key], undefined, `${key} reached the adapter process`);
		}
		// Control: the poison really was in the parent, so the four assertions
		// above are about stripping rather than about an empty environment.
		assert.equal(observed.CG_ACP_STUB_RECORD_ENV, "1", "the child inherited nothing at all; the stripping assertions prove nothing");
	});

	it("ACP-108: an Anthropic API key configured in the harness refuses the run, naming the rule", async () => {
		const log = newLog();
		const out = await driveRpc({
			log,
			sessionDir: join(fixture.root, "s108"),
			// Prime resolves `ANTHROPIC_API_KEY` from its own environment, so this is
			// the product surface: a key configured for the harness, not merely one
			// lying around in a shell.
			extraEnv: { ANTHROPIC_API_KEY: "sk-ant-api03-configured-not-real" },
			script: [{ command: { id: "t1", type: "prompt", message: "ACP-K3" } }],
		});
		assert.match(out, /Claude Code's own login|would bill/, `the run did not refuse a configured API key: ${out.slice(-1500)}`);
		assert.equal(methods(readLog(log)).filter((method) => method === "session/prompt").length, 0, "a prompt was sent despite a configured API key");
	});

	it("ACP-106: with no interactive surface, a permission request is rejected", async () => {
		const log = newLog();
		const sessions = join(fixture.root, "s106");
		await driveRpc({ log, sessionDir: sessions, extraEnv: { CG_ACP_STUB_PERMISSION: "1" }, script: [{ command: { id: "t1", type: "prompt", message: "ACP-P1" } }] });
		const answer = readLog(log).find((entry) => entry.kind === "permission_answer");
		assert.ok(answer, "the agent asked for permission and never got an answer; the turn would hang");
		const outcome = answer.answer?.outcome;
		assert.ok(
			outcome?.outcome === "cancelled" || String(outcome?.optionId ?? "").startsWith("reject"),
			`a headless run has no user, so "ask" must not become "yes"; it answered ${JSON.stringify(outcome)}`,
		);
	});

	it("ACP-107: an identity that would bill an API key is refused, and the subscription identity is not", async () => {
		const refusedLog = newLog();
		const out = await driveRpc({
			log: refusedLog,
			sessionDir: join(fixture.root, "s107a"),
			extraEnv: { CG_ACP_STUB_AUTH_KIND: "api_key" },
			script: [{ command: { id: "t1", type: "prompt", message: "ACP-K1" } }],
		});
		assert.match(out, /Claude Code subscription login|would bill/, `the run did not refuse an API-key identity: ${out.slice(-1200)}`);
		assert.equal(methods(readLog(refusedLog)).filter((method) => method === "session/prompt").length, 0, "a prompt was sent despite the refusal");

		// Control: the identical run on the subscription identity gets through, so
		// the refusal is keyed on the identity and not on the scenario.
		const okLog = newLog();
		await driveRpc({ log: okLog, sessionDir: join(fixture.root, "s107b"), script: [{ command: { id: "t1", type: "prompt", message: "ACP-K2" } }] });
		assert.deepEqual(prompts(readLog(okLog)), ["ACP-K2"], "the control run did not reach the agent, so the refusal above proves nothing");
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});

/** The single session JSONL a fixture session directory holds. */
function findSessionFile(dir: string): string {
	const files = readdirSync(dir).filter((name) => name.endsWith(".jsonl"));
	assert.equal(files.length, 1, `expected exactly one session file in ${dir}, found ${JSON.stringify(files)}`);
	return files[0];
}
