/**
 * P1-LOAD — the distribution actually loads, and the trust footgun is pinned.
 *
 * Gate P1 asks for "package loading" and "project/global resource precedence
 * characterized". The important word is *characterized*: asserting that
 * `.pi/settings.json` names an extension proves the file says so, not that the
 * runtime honoured it. So every assertion here reads the resolved inventory
 * back out of a live Pi.
 *
 * How that read-back works, and why it is this and not something cheaper: Pi
 * 0.84.4 exposes no extension-enumeration API to an extension, and prints no
 * startup manifest in print, json or rpc mode -- `--verbose` was checked
 * against the pinned binary and adds nothing in those modes. The RPC
 * `get_commands` response, however, reports every resolved command with its
 * real source path, scope and origin. That is why cg-version-guard registers a
 * `/cg-version` command: it gives the loaded configuration an observable
 * surface.
 *
 * Every invocation runs against a throwaway `PI_CODING_AGENT_DIR`. Without
 * that, the "untrusted project is ignored" assertion would depend on whether
 * this machine's `~/.pi/agent/trust.json` had ever been told to trust this
 * directory, which is not a property of the distribution.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
	pinnedPiAvailable,
	resolvedCommands,
	runPinnedPi,
	type ResolvedCommandInfo,
} from "../lib/pi-runtime.ts";
import { repoRelative } from "../lib/repo.ts";

const skip = pinnedPiAvailable()
	? false
	: "pinned pi is not installed; run scripts/bootstrap.sh";

function byName(commands: ResolvedCommandInfo[], name: string): ResolvedCommandInfo | undefined {
	return commands.find((command) => command.name === name);
}

describe("P1-LOAD: resource loading against the pinned runtime", { skip }, () => {
	it("gets past resource loading and stops on missing credentials", async () => {
		// The interesting part is the exit code and where it comes from. A load
		// failure and a credential failure both end the process; only the second
		// proves the extensions, skills and prompts were resolved first.
		const result = await runPinnedPi([
			"--approve",
			"--no-context-files",
			"--no-session",
			"-p",
			"hello",
		]);

		assert.equal(result.timedOut, false, "pi -p hung instead of exiting");
		assert.equal(
			result.code,
			1,
			`expected exit 1 with no credentials, got ${result.code}\n${result.stdout}\n${result.stderr}`,
		);
		const output = `${result.stdout}${result.stderr}`;
		assert.match(output, /No API key found/i, "expected the no-credentials refusal");

		// If the version guard had refused, it would have said so on stderr, and
		// every tool call in that session would have been blocked.
		assert.doesNotMatch(
			output,
			/\[cg-version-guard\]/,
			"the version guard refused against the pinned runtime",
		);
	});

	it("resolves the Command Governor loadout when the project is approved", async () => {
		const commands = await resolvedCommands(["--approve"]);

		const guard = byName(commands, "cg-version");
		assert.ok(guard, "cg-version-guard did not register its command; the extension did not load");
		assert.equal(guard.source, "extension");
		assert.equal(guard.sourceInfo?.scope, "project");
		assert.equal(
			guard.sourceInfo?.origin,
			"package",
			"the extension should arrive through the pi package manifest, not as a loose project file",
		);
		assert.equal(
			repoRelative(guard.sourceInfo?.path ?? ""),
			"harness/extensions/cg-version-guard.ts",
		);

		const prompt = byName(commands, "cg-review");
		assert.ok(prompt, "the cg-review prompt template did not resolve");
		assert.equal(prompt.source, "prompt");
		assert.equal(
			repoRelative(prompt.sourceInfo?.path ?? ""),
			"harness/prompts/cg-review.md",
		);

		const skill = byName(commands, "skill:cg-conformance");
		assert.ok(skill, "the cg-conformance skill did not resolve");
		assert.equal(skill.source, "skill");
		assert.equal(
			repoRelative(skill.sourceInfo?.path ?? ""),
			"harness/skills/cg-conformance/SKILL.md",
		);
	});

	it("loads the types-only foreman directory as nothing at all", async () => {
		// harness/extensions/cg-foreman/ has no index.ts, so Pi must skip it. If
		// a future edit turns transport.ts into a loadable extension without a
		// default export, this is where it surfaces.
		const result = await runPinnedPi(
			["--mode", "rpc", "--approve", "--no-context-files", "--no-session"],
			{ stdin: '{"type":"get_commands","id":1}\n' },
		);
		assert.equal(
			result.stderr.trim(),
			"",
			`pi reported an error while loading extensions:\n${result.stderr}`,
		);
	});

	it("silently ignores project resources when the project is NOT approved", async () => {
		// This is the documented default and it is a footgun, so it is pinned by
		// a test rather than by a paragraph. Headless Pi never prompts for
		// project trust; under the default `defaultProjectTrust` it drops every
		// project resource and exits successfully with an empty loadout. Nothing
		// errors. This is precisely why bin/cg-pi passes --approve.
		const commands = await resolvedCommands([]);

		assert.equal(byName(commands, "cg-version"), undefined);
		assert.equal(byName(commands, "cg-review"), undefined);
		assert.equal(byName(commands, "skill:cg-conformance"), undefined);

		for (const command of commands) {
			assert.notEqual(
				command.sourceInfo?.scope,
				"project",
				`${command.name} resolved with project scope without --approve`,
			);
		}
	});

	it("ignores project resources under an explicit --no-approve too", async () => {
		const commands = await resolvedCommands(["--no-approve"]);
		assert.equal(byName(commands, "cg-version"), undefined);
	});
});
