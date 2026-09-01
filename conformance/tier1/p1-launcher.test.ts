/**
 * P1-LAUNCHER — the trust grant is confined to this repository.
 *
 * `bin/cg-pi` passes `--approve`, and Pi resolves "the project" from the
 * WORKING DIRECTORY rather than from wherever the launcher lives. A bare
 * `--approve` is therefore not a statement about this repository at all; it is
 * blanket trust of whatever directory the caller was standing in.
 *
 * That was not a hypothesis. Run from an unrelated directory containing a
 * `.pi/extensions/`, the launcher loaded that directory's extension and did not
 * load cg-version-guard -- auto-trusting unreviewed code while dropping its own
 * safety extension. These tests pin the fix, and the negative control is the
 * point of them: it is not enough that the repository loadout resolves, the
 * foreign one must also be refused.
 */

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import {
	pinnedPiAvailable,
	runLauncher,
	type PiResult,
} from "../lib/pi-runtime.ts";
import { readPins, REPO_ROOT } from "../lib/repo.ts";

const skip = pinnedPiAvailable()
	? false
	: "pinned pi is not installed; run scripts/bootstrap.sh";

const PINNED = readPins().pi.version;

/** A directory that is not this repository, carrying a project loadout. */
let foreignDir = "";
/** Keeps PI_SUBAGENTS_TEMP_ROOT out of the developer's real home. */
let stateDir = "";

before(() => {
	foreignDir = mkdtempSync(join(tmpdir(), "cg-conformance-foreign-"));
	stateDir = mkdtempSync(join(tmpdir(), "cg-conformance-state-"));
	mkdirSync(join(foreignDir, ".pi", "extensions"), { recursive: true });
	writeFileSync(
		join(foreignDir, ".pi", "extensions", "foreign.ts"),
		[
			'import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";',
			"export default function (pi: ExtensionAPI) {",
			'  pi.registerCommand("foreign-danger", {',
			'    description: "an extension from a directory nobody reviewed",',
			"    handler: async () => {},",
			"  });",
			"}",
			"",
		].join("\n"),
	);
	writeFileSync(join(foreignDir, ".pi", "settings.json"), '{"quietStartup": false}\n');
});

after(() => {
	for (const dir of [foreignDir, stateDir]) {
		if (dir) rmSync(dir, { recursive: true, force: true });
	}
});

function launcherEnv(): Record<string, string> {
	return { CG_STATE_DIR: stateDir };
}

function resolvedNames(result: PiResult): string[] {
	for (const line of result.stdout.split("\n")) {
		if (line.trim() === "") continue;
		let record: unknown;
		try {
			record = JSON.parse(line);
		} catch {
			continue;
		}
		const message = record as {
			type?: string;
			command?: string;
			data?: { commands?: { name?: string }[] };
		};
		if (message.type === "response" && message.command === "get_commands") {
			return (message.data?.commands ?? []).map((c) => String(c.name));
		}
	}
	throw new Error(`no get_commands response:\n${result.stdout}\n${result.stderr}`);
}

const GET_COMMANDS = '{"type":"get_commands","id":1}\n';
const RPC_ARGS = ["--mode", "rpc", "--no-context-files", "--no-session"];

describe("P1-LAUNCHER: trust is confined to the repository", { skip }, () => {
	it("refuses to run from a directory outside the checkout", async () => {
		const result = await runLauncher(["--version"], {
			cwd: foreignDir,
			env: launcherEnv(),
		});

		assert.notEqual(result.code, 0, "the launcher must refuse, not proceed");
		assert.match(result.stderr, /refusing to run outside/i);
		assert.match(result.stderr, /--approve/);
		assert.doesNotMatch(
			result.stdout,
			new RegExp(PINNED.replace(/\./g, "\\.")),
			"nothing should have been launched",
		);
	});

	it("does not load a foreign directory's project extensions", async () => {
		// The negative control. Refusing is only meaningful if it is also true
		// that the foreign loadout never resolves -- the original defect showed
		// up as a *successful* run with the wrong extensions.
		const result = await runLauncher(RPC_ARGS, {
			cwd: foreignDir,
			env: launcherEnv(),
			stdin: GET_COMMANDS,
		});

		assert.notEqual(result.code, 0, "the launcher must refuse");
		assert.doesNotMatch(
			result.stdout,
			/foreign-danger/,
			"a foreign project extension was loaded",
		);
	});

	it("resolves this repository's loadout from the repository root", async () => {
		const result = await runLauncher(RPC_ARGS, {
			cwd: REPO_ROOT,
			env: launcherEnv(),
			stdin: GET_COMMANDS,
		});

		const names = resolvedNames(result);
		assert.ok(names.includes("cg-version"), `cg-version missing from ${names.join(", ")}`);
		assert.ok(!names.includes("foreign-danger"));
	});

	it("resolves the same loadout from a subdirectory of the repository", async () => {
		// Pi finds the project from the working directory, so without the `cd`
		// to the root a subdirectory would resolve no project resources at all
		// -- silently, and with a successful exit.
		const result = await runLauncher(RPC_ARGS, {
			cwd: join(REPO_ROOT, "harness", "extensions"),
			env: launcherEnv(),
			stdin: GET_COMMANDS,
		});

		const names = resolvedNames(result);
		assert.ok(
			names.includes("cg-version"),
			`the loadout did not follow the launcher into a subdirectory: ${names.join(", ")}`,
		);
	});

	it("reports the pinned version from inside the checkout", async () => {
		const result = await runLauncher(["--version"], {
			cwd: REPO_ROOT,
			env: launcherEnv(),
		});
		assert.equal(result.code, 0, result.stderr);
		assert.equal(result.stdout.trim(), PINNED);
	});

	it("points delegated work at a durable state root, not the system temp dir", async () => {
		// The subagent package selected for Phase C defaults its durable run
		// state to os.tmpdir(). The launcher overrides that, and CG_STATE_DIR is
		// how an operator relocates it.
		const result = await runLauncher(["--version"], {
			cwd: REPO_ROOT,
			env: launcherEnv(),
		});
		assert.equal(result.code, 0);
		// The launcher creates the directory before exec; its existence after a
		// successful run is the observable effect.
		const { existsSync } = await import("node:fs");
		assert.ok(
			existsSync(join(stateDir, "subagents")),
			"PI_SUBAGENTS_TEMP_ROOT was not created under CG_STATE_DIR",
		);
	});
});
