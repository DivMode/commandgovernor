/**
 * BRIDGE — the subscription-only boundary, enforced by the vendored bridge.
 *
 * Command Governor runs Claude only on Claude Code's own login (the user's
 * subscription, plan-billed). Never an API key, never a harness-held OAuth
 * token, never "extra usage". That is a product invariant, and the place it
 * is enforced is the vendored `pi-claude-agent-sdk`'s `child-env.ts` as
 * patched by `pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch`: the
 * module that builds the environment of every Claude Code child.
 *
 * These tests run that shipped module, unmodified, from the tree
 * `scripts/bootstrap.sh` extracted and patched. Each has a control.
 *
 *   BRIDGE-001 with no credential in the harness and a POISONED inherited
 *              environment (API key, OAuth token, bearer, backend switches),
 *              the child env carries none of them and keeps HOME, USER, PATH.
 *              Control: the same base env with a harmless variable keeps it.
 *   BRIDGE-002 a Prime-shaped registry resolving an API key: refused before
 *              any child, and no credential variable is set.
 *   BRIDGE-003 a Prime-shaped registry resolving an OAuth token, and a
 *              Pi-shaped registry resolving a bearer header: refused likewise.
 *   BRIDGE-004 the no-credential case through the same registries (Prime
 *              answering "not ok", Pi answering undefined) passes: the control
 *              that shows the refusal is keyed on the credential, not on the
 *              registry's presence.
 *
 * Credential-free; no Prime process; nothing leaves the machine.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { before, describe, it } from "node:test";

import { readPins, REPO_ROOT } from "../lib/repo.ts";

const DRIVER = join(REPO_ROOT, "conformance", "lib", "claude-bridge-driver.mjs");
const PACKAGE_SPEC = "./pins/packages/pi-claude-agent-sdk-0.8.6";

interface DriverResult {
	readonly ok: boolean;
	readonly refused?: boolean;
	readonly error?: string;
	readonly env?: Record<string, string | undefined>;
	readonly stripped?: readonly string[];
}

const POISON: Record<string, string> = {
	ANTHROPIC_API_KEY: "sk-ant-api03-poison-not-a-real-key",
	ANTHROPIC_AUTH_TOKEN: "poison-bearer",
	ANTHROPIC_OAUTH_TOKEN: "sk-ant-oat-poison",
	CLAUDE_CODE_OAUTH_TOKEN: "sk-ant-oat-poison-cc",
	ANTHROPIC_BASE_URL: "https://poison.example",
	ANTHROPIC_CUSTOM_HEADERS: "x-poison: 1",
	CLAUDE_CODE_USE_BEDROCK: "1",
	CLAUDE_CODE_USE_VERTEX: "1",
	CLAUDE_CODE_USE_FOUNDRY: "1",
};
const KEEP: Record<string, string> = { HOME: "/Users/example", USER: "example", PATH: "/usr/bin", CG_HARMLESS: "kept" };
const CREDENTIAL_KEYS = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"];

let packageDir = "";

function drive(scenario: unknown): Promise<DriverResult> {
	return new Promise((resolve, reject) => {
		const child = spawn(process.execPath, ["--experimental-transform-types", "--no-warnings", DRIVER, "env", JSON.stringify(scenario)], {
			env: { PATH: process.env.PATH ?? "", CG_BRIDGE_DIR: packageDir },
			stdio: ["ignore", "pipe", "pipe"],
		});
		let stdout = "";
		let stderr = "";
		child.stdout.on("data", (chunk: Buffer) => (stdout += chunk.toString("utf8")));
		child.stderr.on("data", (chunk: Buffer) => (stderr += chunk.toString("utf8")));
		child.on("error", reject);
		child.on("close", () => {
			const text = stdout.trim();
			if (!text.startsWith("{")) return reject(new Error(`driver produced no JSON (stderr: ${stderr.slice(0, 400)})`));
			resolve(JSON.parse(text) as DriverResult);
		});
	});
}

function assertNoCredential(env: Record<string, string | undefined> | undefined, label: string): void {
	assert.ok(env, `${label}: no env returned`);
	for (const key of CREDENTIAL_KEYS) assert.equal(env[key], undefined, `${label}: ${key} reached the child env`);
	for (const key of Object.keys(POISON)) assert.equal(env[key], undefined, `${label}: inherited ${key} reached the child env`);
}

describe("BRIDGE: Claude runs only on Claude Code's own login", () => {
	before(() => {
		const pinned = readPins().packages.find((entry) => entry.source === PACKAGE_SPEC);
		assert.ok(pinned, `${PACKAGE_SPEC} must be pinned in pins/pins.json`);
		packageDir = join(REPO_ROOT, PACKAGE_SPEC.slice(2));
		assert.ok(existsSync(join(packageDir, "src", "child-env.ts")), `${PACKAGE_SPEC} is not extracted; run scripts/bootstrap.sh first`);
		const source = readFileSync(join(packageDir, "src", "child-env.ts"), "utf8");
		assert.ok(source.includes("Command Governor patch"), "the extracted tree does not carry the repository's patch; bootstrap did not apply it");
		assert.ok(!/env\.CLAUDE_CODE_OAUTH_TOKEN\s*=|env\.ANTHROPIC_API_KEY\s*=/.test(source), "the patched module must not contain a credential injection at all");
	});

	it("BRIDGE-001: a poisoned inherited environment never reaches the child; harmless variables do", async () => {
		const result = await drive({ base: { ...KEEP, ...POISON } });
		assert.equal(result.ok, true, result.error);
		assertNoCredential(result.env, "poisoned");
		for (const key of Object.keys(KEEP)) assert.equal(result.env?.[key], KEEP[key], `control: ${key} must be kept`);
		assert.equal(result.env?.ENABLE_CLAUDEAI_MCP_SERVERS, "0", "the bridge's own child settings still apply");
	});

	it("BRIDGE-002: an API key resolved by Prime's registry is refused before any child", async () => {
		const result = await drive({ base: KEEP, registry: { kind: "prime", result: { ok: true, apiKey: "sk-ant-api03-configured-not-real" } } });
		assert.equal(result.refused, true, `expected a refusal, got ${JSON.stringify(result).slice(0, 300)}`);
		assert.match(result.error ?? "", /Claude Code's own login/);
		assert.equal(result.env, undefined, "no env may be produced alongside a refusal");
	});

	it("BRIDGE-003: an OAuth token or a bearer resolved by the harness is refused likewise", async () => {
		const oauth = await drive({ base: KEEP, registry: { kind: "prime", result: { ok: true, apiKey: "sk-ant-oat-configured-not-real" } } });
		assert.equal(oauth.refused, true, `OAuth: expected a refusal, got ${JSON.stringify(oauth).slice(0, 300)}`);
		const bearer = await drive({ base: KEEP, registry: { kind: "pi", result: { auth: { headers: { Authorization: "Bearer configured-not-real" } }, source: "oauth" } } });
		assert.equal(bearer.refused, true, `bearer: expected a refusal, got ${JSON.stringify(bearer).slice(0, 300)}`);
		const headerKey = await drive({ base: KEEP, registry: { kind: "pi", result: { auth: { headers: { "x-api-key": "configured-not-real" } }, source: "stored" } } });
		assert.equal(headerKey.refused, true, `x-api-key: expected a refusal, got ${JSON.stringify(headerKey).slice(0, 300)}`);
	});

	it("BRIDGE-004: control — the same registries with no credential pass, so the refusal is keyed on the credential", async () => {
		const prime = await drive({ base: { ...KEEP, ...POISON }, registry: { kind: "prime", result: { ok: false, error: "No API key found" } } });
		assert.equal(prime.ok, true, prime.error);
		assertNoCredential(prime.env, "prime/no credential");
		const pi = await drive({ base: { ...KEEP, ...POISON }, registry: { kind: "pi", result: undefined } });
		assert.equal(pi.ok, true, pi.error);
		assertNoCredential(pi.env, "pi/no credential");
	});
});
