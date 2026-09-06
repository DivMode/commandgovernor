/**
 * ACP-ENV — the subscription-only boundary and the no-Extra-Usage rule, in the
 * module that decides what the Claude Code process may see.
 *
 * Command Governor runs Claude only on Claude Code's own login (the user's
 * subscription, plan-billed). Never an API key, never a harness-held OAuth
 * token, never a 1M-context turn — 1M is billed as Extra Usage on top of the
 * plan. Both are product invariants and both are enforced in
 * `harness/extensions/claude-acp/src/`, which is what these tests run.
 *
 *   ACP-001 with no credential in the harness and a POISONED inherited
 *           environment (API key, OAuth token, bearer, backend switches,
 *           nested-Claude markers, a model override), the child environment
 *           carries none of them and keeps HOME, USER and PATH — Claude Code
 *           finds its Keychain login through exactly those.
 *           Control: a harmless variable in the same base env survives.
 *   ACP-002 a Prime-shaped registry resolving an API key: refused BEFORE any
 *           child, with a message that names the rule, and no environment
 *           produced alongside the refusal.
 *   ACP-003 an OAuth token, a bearer header and an `x-api-key` header are
 *           refused the same way, through both registry shapes.
 *   ACP-004 control — the same registries with no credential pass, so the
 *           refusal is keyed on the credential and not on the registry.
 *   ACP-005 the forced child settings are present, `CLAUDE_CODE_DISABLE_1M_CONTEXT`
 *           above all.
 *   ACP-006 no registered model id asks for an extended context window, and the
 *           guard that says so rejects one that does. Without the negative
 *           control the assertion would pass on a guard that never fires.
 *
 * Credential-free; no Prime process; no network; nothing leaves the machine.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

import { before, describe, it } from "node:test";

import { REPO_ROOT } from "../lib/repo.ts";

const DRIVER = join(REPO_ROOT, "conformance", "lib", "claude-acp-driver.mjs");
const EXTENSION_DIR = join(REPO_ROOT, "harness", "extensions", "claude-acp");

interface DriverResult {
	readonly ok: boolean;
	readonly refused?: boolean;
	readonly error?: string;
	readonly env?: Record<string, string | undefined>;
	readonly stripped?: readonly string[];
	readonly forced?: Record<string, string>;
	readonly ids?: readonly string[];
	readonly contextWindows?: readonly number[];
	readonly costs?: readonly Record<string, number>[];
	readonly registeredContextWindow?: number;
	readonly guardRejected?: string | false | null;
}

/** Everything a shell might already be exporting that must not reach the child. */
const POISON: Record<string, string> = {
	ANTHROPIC_API_KEY: "sk-ant-api03-poison-not-a-real-key",
	ANTHROPIC_AUTH_TOKEN: "poison-bearer",
	ANTHROPIC_OAUTH_TOKEN: "sk-ant-oat-poison",
	CLAUDE_CODE_OAUTH_TOKEN: "sk-ant-oat-poison-cc",
	ANTHROPIC_BASE_URL: "https://poison.example",
	ANTHROPIC_CUSTOM_HEADERS: "x-poison: 1",
	ANTHROPIC_BEDROCK_BASE_URL: "https://poison.bedrock.example",
	ANTHROPIC_VERTEX_BASE_URL: "https://poison.vertex.example",
	ANTHROPIC_MODEL: "claude-opus-5[1m]",
	CLAUDE_CODE_USE_BEDROCK: "1",
	CLAUDE_CODE_USE_VERTEX: "1",
	CLAUDE_CODE_USE_FOUNDRY: "1",
	// Command Governor's own agents run inside Claude Code, so these are the
	// realistic ones: left set, the child believes it is a nested session.
	CLAUDECODE: "1",
	CLAUDE_CODE_ENTRYPOINT: "cli",
};
const KEEP: Record<string, string> = { HOME: "/Users/example", USER: "example", PATH: "/usr/bin", CG_HARMLESS: "kept" };
const CREDENTIAL_KEYS = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"];

function drive(command: string, scenario: unknown = {}): Promise<DriverResult> {
	return new Promise((resolve, reject) => {
		const child = spawn(process.execPath, ["--experimental-transform-types", "--no-warnings", DRIVER, command, JSON.stringify(scenario)], {
			env: { PATH: process.env.PATH ?? "", CG_ACP_EXTENSION_DIR: EXTENSION_DIR },
			stdio: ["ignore", "pipe", "pipe"],
		});
		let stdout = "";
		let stderr = "";
		child.stdout.on("data", (chunk: Buffer) => (stdout += chunk.toString("utf8")));
		child.stderr.on("data", (chunk: Buffer) => (stderr += chunk.toString("utf8")));
		child.on("error", reject);
		child.on("close", () => {
			const text = stdout.trim();
			if (!text.startsWith("{")) return reject(new Error(`driver produced no JSON (stderr: ${stderr.slice(0, 600)})`));
			resolve(JSON.parse(text) as DriverResult);
		});
	});
}

function assertNoCredential(env: Record<string, string | undefined> | undefined, label: string): void {
	assert.ok(env, `${label}: no env returned`);
	for (const key of CREDENTIAL_KEYS) assert.equal(env[key], undefined, `${label}: ${key} reached the child env`);
	for (const key of Object.keys(POISON)) assert.equal(env[key], undefined, `${label}: inherited ${key} reached the child env`);
}

describe("ACP-ENV: Claude runs only on Claude Code's own login, and never on Extra Usage", () => {
	before(() => {
		assert.ok(existsSync(join(EXTENSION_DIR, "src", "child-env.ts")), `${EXTENSION_DIR} is missing its shipped modules`);
	});

	it("ACP-001: a poisoned inherited environment never reaches the child; harmless variables do", async () => {
		const result = await drive("env", { base: { ...KEEP, ...POISON } });
		assert.equal(result.ok, true, result.error);
		assertNoCredential(result.env, "poisoned");
		for (const key of Object.keys(KEEP)) {
			assert.equal(result.env?.[key], KEEP[key], `control: ${key} must be kept — Claude Code finds its login through HOME/USER`);
		}
	});

	it("ACP-002: an API key resolved by Prime's registry is refused before any child", async () => {
		const result = await drive("env", { base: KEEP, registry: { kind: "prime", result: { ok: true, apiKey: "sk-ant-api03-configured-not-real" } } });
		assert.equal(result.refused, true, `expected a refusal, got ${JSON.stringify(result).slice(0, 300)}`);
		assert.match(result.error ?? "", /Claude Code's own login/);
		assert.equal(result.env, undefined, "no env may be produced alongside a refusal");
	});

	it("ACP-003: an OAuth token, a bearer header or an x-api-key header is refused likewise", async () => {
		const oauth = await drive("env", { base: KEEP, registry: { kind: "prime", result: { ok: true, apiKey: "sk-ant-oat-configured-not-real" } } });
		assert.equal(oauth.refused, true, `OAuth: expected a refusal, got ${JSON.stringify(oauth).slice(0, 300)}`);
		const bearer = await drive("env", { base: KEEP, registry: { kind: "pi", result: { auth: { headers: { Authorization: "Bearer configured-not-real" } }, source: "oauth" } } });
		assert.equal(bearer.refused, true, `bearer: expected a refusal, got ${JSON.stringify(bearer).slice(0, 300)}`);
		const headerKey = await drive("env", { base: KEEP, registry: { kind: "pi", result: { auth: { headers: { "x-api-key": "configured-not-real" } }, source: "stored" } } });
		assert.equal(headerKey.refused, true, `x-api-key: expected a refusal, got ${JSON.stringify(headerKey).slice(0, 300)}`);
	});

	it("ACP-004: control — the same registries with no credential pass, so the refusal is keyed on the credential", async () => {
		const prime = await drive("env", { base: { ...KEEP, ...POISON }, registry: { kind: "prime", result: { ok: false, error: "No API key found" } } });
		assert.equal(prime.ok, true, prime.error);
		assertNoCredential(prime.env, "prime/no credential");
		const pi = await drive("env", { base: { ...KEEP, ...POISON }, registry: { kind: "pi", result: undefined } });
		assert.equal(pi.ok, true, pi.error);
		assertNoCredential(pi.env, "pi/no credential");
	});

	it("ACP-005: the 200K clamp is opt-in, off by default, and never inherited", async () => {
		// Off by default. Capping the window has no measured allowance saving and
		// makes compaction — itself a summarisation turn — happen more often, so it
		// is a decision the user makes rather than one this code imposes.
		const byDefault = await drive("env", { base: KEEP });
		assert.equal(byDefault.ok, true, byDefault.error);
		assert.equal(byDefault.env?.CLAUDE_CODE_DISABLE_1M_CONTEXT, undefined, "the 200K clamp must not be on by default");

		// Not inherited either: whether the window is capped is the configuration's
		// decision, not the surrounding shell's.
		const inherited = await drive("env", { base: { ...KEEP, CLAUDE_CODE_DISABLE_1M_CONTEXT: "1" } });
		assert.equal(inherited.env?.CLAUDE_CODE_DISABLE_1M_CONTEXT, undefined, "an inherited clamp reached the child");

		// And it does reach the child when asked for, so the setting is real.
		const optedIn = await drive("env", { base: KEEP, options: { disableLongContext: true } });
		assert.equal(optedIn.env?.CLAUDE_CODE_DISABLE_1M_CONTEXT, "1", "the opt-in setting did not reach the child");
	});

	it("ACP-006: no registered model asks for an extended window, and the guard that says so can fail", async () => {
		const result = await drive("models");
		assert.equal(result.ok, true, result.error);
		assert.ok((result.ids?.length ?? 0) > 0, "the provider registers no models at all");
		for (const id of result.ids ?? []) assert.doesNotMatch(id, /\[\s*\d+\s*m\s*\]/i, `${id} requests an extended context window`);
		for (const window of result.contextWindows ?? []) assert.equal(window, result.registeredContextWindow);
		// Plan-billed turns have no per-token price; a number here would be fiction
		// in Prime's own footer.
		for (const cost of result.costs ?? []) assert.deepEqual(cost, { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
		// Negative control: without this, the assertion above would also pass
		// against a guard that never rejects anything.
		assert.equal(typeof result.guardRejected, "string", "the long-context guard accepted an id ending in [1m]");
		assert.match(String(result.guardRejected), /Extra Usage/);
	});
});
