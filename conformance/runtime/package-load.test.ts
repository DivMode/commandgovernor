/**
 * LOAD — every package this distribution pins actually registers on the pinned
 * Prime, and the Command Governor package itself does too.
 *
 * A version in `pins/pins.json` proves that a tarball was downloaded. It does
 * not prove that Prime loaded it: the measured seams are real and specific —
 * `pi-squad` and `pi-subagents` (unscoped) fail on Prime's
 * `@earendil-works/pi-ai/compat` jiti alias, `gentle-pi` on a `createTool`
 * export Prime does not have — and, decisively, **Prime discards extension
 * load failures in headless modes**. A package that throws on load produces no
 * diagnostic on stdout or stderr under `-p`, and none under `--mode json`. So
 * "the package registered nothing" and "the package never loaded" look
 * identical unless the probe is built to tell them apart.
 *
 * Two observation channels, both black-box, both from the pinned binary:
 *
 *   1. the `tools` array the agent advertises on the wire, recorded by the mock
 *      provider — where a package's model-facing tools appear;
 *   2. `prime-agent --mode rpc` `get_commands`, whose every entry carries a
 *      `sourceInfo.source` naming the exact package spec it came from — which
 *      covers prompts, skills and host-gated commands that never reach the
 *      model's tool list.
 *
 * And the control that makes both of them measurements rather than decoration:
 * the same run loads `conformance/lib/probe-extension.ts`, which registers
 * `cg_probe_loaded`, and `conformance/lib/broken-extension.ts`, which throws.
 * The first must be present, the second's `cg_probe_broken` must be absent, and
 * the process must still exit 0. If that pair does not come out that way, every
 * other assertion in this file is unreliable and says so.
 *
 * The Command Governor ROLE FILES are checked through the same channel, and
 * that is the point: `harness/agents/*.md` are configuration in
 * `@gotgenes/pi-subagents`' agent-file format, and this repository ships no
 * code that reads them. The only honest check is whether the package that does
 * read them can see them, so the files are installed exactly as
 * `harness/agents/README.md` documents — `cp harness/agents/*.md .pi/agents/`
 * in the project — and the assertion is made against the `subagent` tool's own
 * schema on the wire.
 *
 * Installs are project-scoped (`package install --local`) into a disposable
 * fixture project with its own HOME. `npm install -g` is never run, and the
 * repository's own `harness/` is COPIED into the fixture before install so npm
 * cannot write into the checkout.
 *
 * This test needs the network: `package install` runs npm.
 */

import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";

import { sleep, startRoot, waitUntil, type PrimeRoot } from "../lib/prime.ts";
import { HARNESS_DIR, readPins, REPO_ROOT } from "../lib/repo.ts";
import { assertCleanTeardown } from "../lib/teardown.ts";

/**
 * What each pinned package must be observed to register, measured on Prime
 * 0.9.1 on 2026-09-04 rather than read from a README.
 *
 * `tools` are model-facing and must appear on the wire. `commands` must appear
 * in `get_commands` attributed to that exact package spec. A package with no
 * `tools` entry is not a package with no tools — `pi-pr-review` registers four
 * and Prime advertises none of them at idle, because its own loop enables them
 * only once a `/pr-review` invocation binds. That host gating is the package's
 * design, so the command channel is the right probe for it.
 *
 * Every entry in `pins.json` `packages[]` must appear here, so admitting a
 * package without measuring what it registers fails this file.
 */
const EXPECTED: Record<string, { tools: string[]; commands: string[]; note?: string }> = {
	"npm:pi-tasks@0.2.5": {
		tools: ["task_plan", "task_evidence", "task_complete"],
		commands: ["tasks"],
	},
	"npm:@gotgenes/pi-subagents@21.4.0": {
		tools: ["subagent", "get_subagent_result", "steer_subagent"],
		commands: ["subagents:sessions"],
	},
	"npm:pi-pr-review@1.17.10": {
		tools: [],
		commands: ["pr-review", "pr-review-publish"],
		note: "Its review tools are host-gated: registered, but advertised to the model only once a /pr-review invocation binds, so the wire tool list at idle is the wrong channel for it.",
	},
	"npm:pi-gpt@0.4.3": {
		tools: ["gpt_account_status", "gpt_list_models", "gpt_chat", "gpt_list_chats", "gpt_get_conversation", "gpt_get_message"],
		commands: ["gpt-observer"],
		note: "The foreman transport. Its ChatGPT client is constructed lazily on first tool use, so registration needs no Codex login; the fixture has none. The observer extension registers only its command and is off by default.",
	},
};

/** What the Command Governor package itself must register. It ships no extensions. */
const LOCAL_EXPECTED = { commands: ["cg-review", "skill:cg-conformance"] };

/** The role files that must be visible to the delegation package. */
const EXPECTED_ROLES = ["implementer", "reviewer", "scout", "researcher"];

/**
 * A name that is deliberately NOT a file in `.pi/agents/`.
 *
 * The negative control for the role check: if this appeared, the agent list on
 * the wire would not be derived from the directory at all and the positive
 * assertions below would be meaningless.
 */
const ABSENT_ROLE = "cg-absent-control";

/**
 * Extension, skill and prompt discovery stay ON: they are this file's whole
 * subject, and `-ne`/`-ns`/`-np` would switch off the thing being measured.
 * Discovery is still hermetic because the fixture owns HOME and the project.
 * `-nc` is kept because context-file discovery walks UP from the working
 * directory, out of the fixture, and no package registers a context file.
 */
const FLAGS = ["--provider", "mock", "--model", "mock-1", "-nc", "--no-themes"];

interface ToolDefinition {
	readonly function?: {
		readonly name?: string;
		readonly description?: string;
		readonly parameters?: { readonly properties?: Record<string, { readonly description?: string; readonly enum?: unknown[] }> };
	};
}

interface CommandEntry {
	readonly name?: string;
	readonly source?: string;
	readonly sourceInfo?: { readonly source?: string; readonly baseDir?: string; readonly path?: string; readonly origin?: string };
}

let fixture: PrimeRoot;
let project = "";
let localPackageDir = "";
const installs: { spec: string; command: string; cwd: string; status: number | null; output: string }[] = [];
let packageList = "";
let probeStatus: number | null = null;
let wireTools: string[] = [];
let commands: CommandEntry[] = [];
let subagentTool: ToolDefinition | undefined;
let installedRoleFiles: string[] = [];
/** Each installed role file's own `description:` frontmatter line, by name. */
const roleDescriptions = new Map<string, string>();

const pins = readPins();
const specs = pins.packages.map((entry) => String(entry.source));

/**
 * The `description:` line from a role file's YAML frontmatter.
 *
 * A deliberately tiny reader for one key in a four-line fence. Its only job is
 * to give the assertions a string that exists ONLY in this repository's file,
 * so finding it on the wire proves the package read the file rather than
 * echoing a name it was handed.
 */
function frontmatterDescription(path: string): string | undefined {
	const text = readFileSync(path, "utf8").replace(/\r\n/g, "\n");
	if (!text.startsWith("---\n")) return undefined;
	const end = text.indexOf("\n---\n", 3);
	if (end === -1) return undefined;
	const match = /^description:[ \t]*(.+)$/m.exec(text.slice(4, end + 1));
	return match ? match[1].trim() : undefined;
}

/** Everything the `subagent` tool says about the agent types it can launch. */
function subagentAgentText(): string {
	const fn = subagentTool?.function;
	return `${fn?.description ?? ""}\n${fn?.parameters?.properties?.subagent_type?.description ?? ""}`;
}

/** Every command `get_commands` attributes to this exact package spec. */
function commandsFrom(spec: string): CommandEntry[] {
	return commands.filter((entry) => entry.sourceInfo?.source === spec);
}

/** Every command whose files live inside this directory (the local package). */
function commandsUnder(dir: string): CommandEntry[] {
	const real = realpathSync(dir);
	return commands.filter((entry) => {
		const base = entry.sourceInfo?.baseDir;
		if (!base || !existsSync(base)) return false;
		return realpathSync(base) === real;
	});
}

describe("LOAD: every pinned package registers on the pinned Prime", () => {
	before(async () => {
		// dumpTools records the FULL tools array of the first model request, which
		// is where the `subagent` tool's own schema (and the agent types it read
		// out of .pi/agents/) can be inspected.
		fixture = await startRoot({ label: "package-load", dumpTools: true });

		project = join(fixture.root, "project");
		mkdirSync(project, { recursive: true });
		writeFileSync(join(project, "README.md"), "# conformance scratch project\n");

		// Copied, never installed from the checkout: `package install` runs npm,
		// and npm writes.
		localPackageDir = join(fixture.root, "cg-package");
		cpSync(HARNESS_DIR, localPackageDir, { recursive: true });

		// The role files, installed exactly as harness/agents/README.md documents:
		// `mkdir -p .pi/agents && cp harness/agents/*.md .pi/agents/`. They are read
		// by @gotgenes/pi-subagents, not by anything in this repository.
		const agentsSource = join(HARNESS_DIR, "agents");
		const agentsTarget = join(project, ".pi", "agents");
		mkdirSync(agentsTarget, { recursive: true });
		for (const name of readdirSync(agentsSource)) {
			if (!name.endsWith(".md")) continue;
			cpSync(join(agentsSource, name), join(agentsTarget, name));
			installedRoleFiles.push(name);
			const description = frontmatterDescription(join(agentsSource, name));
			if (description) roleDescriptions.set(name.replace(/\.md$/, ""), description);
		}
		fixture.note("installed role files:", JSON.stringify(installedRoleFiles));

		for (const spec of [...specs, localPackageDir]) {
			const args = ["package", "install", "--local", spec];
			const result = fixture.cli(args, { timeout: 900_000, cwd: project, withoutSocket: true });
			installs.push({
				spec,
				command: `prime-agent ${args.join(" ")}`,
				cwd: project,
				status: result.status,
				output: `${result.stdout}${result.stderr}`.slice(0, 2000),
			});
			fixture.note("install:", `prime-agent ${args.join(" ")}`, "(cwd", `${project})`, "rc", String(result.status));
		}

		const list = fixture.cli(["package", "list"], { timeout: 120_000, cwd: project, withoutSocket: true });
		packageList = `${list.stdout}${list.stderr}`;

		// Channel 1: the wire tool list, with the load-probe control alongside.
		const seen = fixture.mockRequests().length;
		const probe = fixture.cli(
			[
				"-p",
				...FLAGS,
				"--no-session",
				"-e",
				join(REPO_ROOT, "conformance", "lib", "probe-extension.ts"),
				"-e",
				join(REPO_ROOT, "conformance", "lib", "broken-extension.ts"),
				"ECHO:package-load-probe",
			],
			{ timeout: 600_000, cwd: project },
		);
		probeStatus = probe.status;
		const request = fixture.mockRequests().slice(seen).find((entry) => entry.kind === "request");
		wireTools = (request?.toolNames as string[] | undefined) ?? [];
		const dump = fixture.mockRequests().find((entry) => entry.kind === "tools-dump");
		subagentTool = ((dump?.tools as ToolDefinition[] | undefined) ?? []).find((tool) => tool.function?.name === "subagent");
		fixture.note("wire tools:", JSON.stringify(wireTools));

		// Channel 2: `get_commands` over the stock RPC client.
		{
			const rpc = fixture.cliSpawn(["--mode", "rpc", ...FLAGS, "--no-session"], { cwd: project });
			let out = "";
			rpc.stdout?.on("data", (data: Buffer) => {
				out += data.toString("utf8");
			});
			rpc.stderr?.on("data", (data: Buffer) => {
				out += data.toString("utf8");
			});
			try {
				await sleep(4000);
				rpc.stdin?.write(`${JSON.stringify({ id: "cmds", type: "get_commands" })}\n`);
				await waitUntil(() => (out.includes('"id":"cmds"') ? true : undefined), 180_000, 300, "the get_commands response");
				const response = out
					.split("\n")
					.filter(Boolean)
					.map((line) => {
						try {
							return JSON.parse(line) as { id?: string; success?: boolean; data?: { commands?: CommandEntry[] } };
						} catch {
							return undefined;
						}
					})
					.find((line) => line?.id === "cmds");
				assert.ok(response?.success, `get_commands failed: ${JSON.stringify(response)}`);
				commands = response.data?.commands ?? [];
				fixture.note("commands:", JSON.stringify(commands.map((entry) => `${entry.name}<-${entry.sourceInfo?.source ?? "?"}`)));
			} finally {
				try {
					rpc.kill("SIGKILL");
				} catch {
					/* already gone */
				}
				await sleep(1500);
			}
		}
	});

	after(async () => {
		if (fixture) await fixture.stop();
	});

	it("the load probe can tell a loaded extension from one that failed to load", () => {
		// The run exits 0 with a broken extension aboard, which is exactly why the
		// exit status and stderr cannot be used as evidence that a package loaded,
		// and why the tool list is. If a future Prime made a load failure fatal,
		// this assertion would fail — and that failure is the signal to rebuild
		// this file around the exit code, which would be a better control.
		assert.equal(probeStatus, 0, "the probe run itself failed");
		assert.ok(
			wireTools.includes("cg_probe_loaded"),
			`the working control extension did not reach the wire, so every registration result in this file is unreliable: ${JSON.stringify(wireTools)}`,
		);
		assert.ok(
			!wireTools.includes("cg_probe_broken"),
			"a tool from an extension that throws at load reached the wire, which cannot happen and means the probe is measuring something else",
		);
	});

	it("every pinned package installed project-scoped, and none globally", () => {
		assert.equal(installs.length, specs.length + 1, JSON.stringify(installs.map((entry) => entry.spec)));
		for (const entry of installs) {
			assert.equal(entry.status, 0, `${entry.command} (cwd ${entry.cwd}) exited ${entry.status}: ${entry.output}`);
			assert.match(entry.command, /package install --local /, "installs must be project-scoped");
		}
		assert.match(packageList, /Project packages:/, packageList.slice(0, 400));
		for (const spec of specs) assert.ok(packageList.includes(spec), `${spec} is not in \`prime-agent package list\`: ${packageList.slice(0, 600)}`);
		assert.ok(
			existsSync(join(project, ".prime", "agent", "settings.json")),
			"a project-scoped install must write the project's own settings.json",
		);
	});

	it("every pinned package has a measured expectation, so a new pin cannot be admitted unmeasured", () => {
		for (const spec of specs) assert.ok(EXPECTED[spec], `${spec} is pinned but this test does not say what it must register`);
		for (const spec of Object.keys(EXPECTED)) assert.ok(specs.includes(spec), `${spec} is expected here but is no longer pinned`);
	});

	for (const spec of specs) {
		it(`${spec} registered on the pinned Prime`, () => {
			const expected = EXPECTED[spec];
			assert.ok(expected, `${spec} has no measured expectation`);
			for (const tool of expected.tools) {
				assert.ok(wireTools.includes(tool), `${spec}: tool ${tool} is not on the wire: ${JSON.stringify(wireTools)}`);
			}
			const mine = commandsFrom(spec);
			assert.ok(mine.length > 0, `${spec}: nothing in \`get_commands\` is attributed to it`);
			const names = mine.map((entry) => entry.name);
			for (const command of expected.commands) {
				assert.ok(names.includes(command), `${spec}: command ${command} is missing; it registered ${JSON.stringify(names)}`);
			}
		});
	}

	it("the Command Governor package registers its own skills and prompts", () => {
		const mine = commandsUnder(localPackageDir);
		assert.ok(mine.length > 0, `nothing in \`get_commands\` came from ${localPackageDir}`);
		const names = mine.map((entry) => entry.name);
		for (const command of LOCAL_EXPECTED.commands) {
			assert.ok(names.includes(command), `the Command Governor package did not register ${command}; it registered ${JSON.stringify(names)}`);
		}
		assert.ok(
			mine.some((entry) => entry.source === "skill"),
			`its skills did not register: ${JSON.stringify(mine.map((entry) => `${entry.name}:${entry.source}`))}`,
		);
		assert.ok(
			mine.some((entry) => entry.source === "prompt"),
			`its prompts did not register: ${JSON.stringify(mine.map((entry) => `${entry.name}:${entry.source}`))}`,
		);
	});

	it("the Command Governor package ships no extension, and registers none", () => {
		// The copy under the fixture is byte-identical to the checkout's.
		const manifest = JSON.parse(readFileSync(join(localPackageDir, "package.json"), "utf8")) as { pi?: { extensions?: unknown[] } };
		assert.deepEqual(manifest.pi?.extensions ?? [], [], "the distribution package must contain no runtime code");
		assert.ok(
			commandsUnder(localPackageDir).every((entry) => entry.source !== "extension"),
			"the distribution package registered an extension command",
		);
	});

	it("the delegation package can see the Command Governor role files", () => {
		assert.ok(subagentTool, `@gotgenes/pi-subagents did not put a \`subagent\` tool on the wire: ${JSON.stringify(wireTools)}`);
		const text = subagentAgentText();
		assert.ok(text.length > 0, "the subagent tool carries no agent-type text to read");
		for (const role of EXPECTED_ROLES) {
			assert.ok(
				installedRoleFiles.includes(`${role}.md`),
				`harness/agents/${role}.md is missing from the repository, so it could not be installed`,
			);
			assert.ok(
				new RegExp(`\\b${role}\\b`).test(text),
				`the subagent tool does not offer the ${role} agent type; it said: ${text.slice(0, 900)}`,
			);
		}
	});

	it("it read the files, not just their names", () => {
		// Each role's own `description:` frontmatter is a sentence that exists only
		// in this repository. Finding it on the wire is what separates "the package
		// listed the directory" from "the package parsed the file".
		const text = subagentAgentText();
		for (const role of EXPECTED_ROLES) {
			const description = roleDescriptions.get(role);
			assert.ok(description, `harness/agents/${role}.md has no description: frontmatter for the package to read`);
			assert.ok(
				text.includes(description),
				`the subagent tool does not carry ${role}'s own description (${JSON.stringify(description.slice(0, 60))}); it said: ${text.slice(0, 900)}`,
			);
		}
	});

	it("negative control: an agent type with no file is not offered", () => {
		// If this appeared, the agent list would not be derived from .pi/agents/
		// at all and the two assertions above would prove nothing.
		assert.ok(!installedRoleFiles.includes(`${ABSENT_ROLE}.md`), "the control name must not be a real role file");
		assert.ok(
			!subagentAgentText().includes(ABSENT_ROLE),
			`the subagent tool offered ${ABSENT_ROLE}, which has no file in .pi/agents/`,
		);
	});

	it("nothing survived teardown", async () => {
		assertCleanTeardown(await fixture.stop());
	});
});
