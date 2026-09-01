/**
 * Driving the pinned Pi from a test, hermetically.
 *
 * Two properties matter and both are easy to lose:
 *
 * **Hermetic.** Every invocation gets its own `PI_CODING_AGENT_DIR`. Pi's whole
 * agent directory -- global settings, saved project-trust decisions, installed
 * packages, credentials -- lives there. Without the override, the P1-LOAD test
 * that asserts an untrusted project's resources are ignored would pass or fail
 * according to whether the developer had ever typed "always trust" on this
 * machine, which is not a property of the distribution.
 *
 * **The pinned binary.** Never a `pi` from `PATH`. This machine has one, it is
 * currently the same version, and relying on that would make the suite silently
 * stop testing the pin the day it diverges.
 */

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { exists, readPins, REPO_ROOT } from "./repo.ts";

/** Ceiling on any single Pi invocation. A hung child must fail, not hang CI. */
export const PI_TIMEOUT_MS = 90_000;

/** Absolute path to the pinned Pi binary produced by scripts/bootstrap.sh. */
export function pinnedPiBinary(): string {
	const pins = readPins();
	return join(REPO_ROOT, pins.pi.installRoot, "node_modules", ".bin", "pi");
}

/**
 * Whether the bootstrap has been run. Tests that need a live Pi skip with a
 * stated reason rather than failing, so a fresh checkout reports "not
 * bootstrapped" instead of a pile of spawn errors -- but scripts/conformance.sh
 * refuses to start without it, so the skip is never how the suite normally ends.
 */
export function pinnedPiAvailable(): boolean {
	return exists(pinnedPiBinary());
}

/**
 * The pinned runtime's own model catalog.
 *
 * Loaded by path rather than as a bare specifier because `@earendil-works/
 * pi-ai` does not export it: the package's `exports` map has no entry for
 * `./dist/models.generated.js`, and `MODELS` is absent from the main entry
 * point. Reading it out of the pinned install is legitimate here precisely
 * because the pinned install is the artifact under test -- but it is an
 * unexported internal, so a re-pin that moves the file must fail loudly rather
 * than degrade into "no models known", which would make the role-model check
 * vacuously pass.
 *
 * Note this is the *catalog*, not the *available* models. Availability depends
 * on credentials; the catalog does not, which is what makes it usable in the
 * credential-free tier.
 */
export async function pinnedModelCatalog(): Promise<
	Record<string, Record<string, unknown>>
> {
	const pins = readPins();
	const catalogPath = join(
		REPO_ROOT,
		pins.pi.installRoot,
		"node_modules",
		"@earendil-works",
		"pi-ai",
		"dist",
		"models.generated.js",
	);
	if (!exists(catalogPath)) {
		throw new Error(
			`the pinned pi-ai model catalog is not at ${catalogPath}. It is an ` +
				`unexported internal, so a re-pin may have moved it; update ` +
				`conformance/lib/pi-runtime.ts rather than skipping the check.`,
		);
	}
	const module = (await import(pathToFileURL(catalogPath).href)) as {
		MODELS?: Record<string, Record<string, unknown>>;
	};
	if (module.MODELS === undefined) {
		throw new Error(`${catalogPath} no longer exports MODELS`);
	}
	return module.MODELS;
}

export interface PiResult {
	readonly code: number | null;
	readonly signal: NodeJS.Signals | null;
	readonly stdout: string;
	readonly stderr: string;
	readonly timedOut: boolean;
}

export interface PiOptions {
	/** Written to the child's stdin, which is then closed. */
	readonly stdin?: string;
	readonly cwd?: string;
	readonly env?: Readonly<Record<string, string>>;
	readonly timeoutMs?: number;
}

/**
 * Run the pinned Pi with an isolated agent directory and collect its output.
 *
 * `PI_OFFLINE=1` is set so a test can never make a provider call by accident;
 * the credential-free tier asserts on refusals and resolved configuration, not
 * on model output.
 */
export async function runPinnedPi(
	args: readonly string[],
	options: PiOptions = {},
): Promise<PiResult> {
	return runIsolated(pinnedPiBinary(), args, options);
}

/** Absolute path to the launcher. */
export function launcherPath(): string {
	return join(REPO_ROOT, "bin", "cg-pi");
}

/**
 * Run `bin/cg-pi` under the same isolation as {@link runPinnedPi}.
 *
 * Used to prove the launcher's trust confinement, which cannot be checked by
 * reading the script: the property under test is what Pi resolves as "the
 * project", and that depends on the working directory the launcher hands it.
 */
export async function runLauncher(
	args: readonly string[],
	options: PiOptions = {},
): Promise<PiResult> {
	return runIsolated(launcherPath(), args, options);
}

async function runIsolated(
	command: string,
	args: readonly string[],
	options: PiOptions,
): Promise<PiResult> {
	const agentDir = mkdtempSync(join(tmpdir(), "cg-conformance-agent-"));
	try {
		return await new Promise<PiResult>((resolve, reject) => {
			const child = spawn(command, [...args], {
				cwd: options.cwd ?? REPO_ROOT,
				env: {
					...process.env,
					...options.env,
					PI_CODING_AGENT_DIR: agentDir,
					PI_OFFLINE: "1",
				},
				stdio: ["pipe", "pipe", "pipe"],
			});

			let stdout = "";
			let stderr = "";
			let timedOut = false;

			child.stdout.setEncoding("utf8");
			child.stderr.setEncoding("utf8");
			child.stdout.on("data", (chunk: string) => {
				stdout += chunk;
			});
			child.stderr.on("data", (chunk: string) => {
				stderr += chunk;
			});

			const timer = setTimeout(() => {
				timedOut = true;
				child.kill("SIGKILL");
			}, options.timeoutMs ?? PI_TIMEOUT_MS);

			child.on("error", (error) => {
				clearTimeout(timer);
				reject(error);
			});
			child.on("close", (code, signal) => {
				clearTimeout(timer);
				resolve({ code, signal, stdout, stderr, timedOut });
			});

			// Always close stdin. `pi -p` reads piped stdin and merges it into the
			// prompt, so a child whose stdin stays open simply waits forever.
			child.stdin.end(options.stdin ?? "");
		});
	} finally {
		rmSync(agentDir, { recursive: true, force: true });
	}
}

// ---------------------------------------------------------------------------
// RPC
// ---------------------------------------------------------------------------

export interface ResolvedCommandInfo {
	readonly name: string;
	readonly description?: string;
	readonly source: string;
	readonly sourceInfo?: {
		readonly path?: string;
		readonly scope?: string;
		readonly origin?: string;
	};
}

/**
 * Ask a running Pi what it actually resolved.
 *
 * This is the read-back oracle. Asserting that `.pi/settings.json` names an
 * extension proves only that the file says so; `get_commands` reports the
 * inventory the runtime built, with each entry's real source path and scope. Pi
 * 0.84.4 offers no other way to see this from outside: there is no
 * extension-enumeration API, and no startup manifest is printed in print, json
 * or rpc mode -- not even under `--verbose`, which was checked.
 *
 * Framing note carried from Pi's RPC contract: records are split on `\n` only.
 * `node:readline` is not protocol-compliant here because it also splits on
 * U+2028/U+2029, which are legal inside JSON strings.
 */
export async function resolvedCommands(
	extraArgs: readonly string[] = [],
): Promise<ResolvedCommandInfo[]> {
	const result = await runPinnedPi(
		["--mode", "rpc", "--no-context-files", "--no-session", ...extraArgs],
		{ stdin: '{"type":"get_commands","id":1}\n' },
	);

	if (result.timedOut) {
		throw new Error("pi --mode rpc timed out before answering get_commands");
	}

	for (const line of result.stdout.split("\n")) {
		if (line.trim() === "") continue;
		let record: unknown;
		try {
			record = JSON.parse(line);
		} catch {
			continue; // not a protocol record
		}
		const message = record as {
			type?: string;
			command?: string;
			success?: boolean;
			data?: { commands?: ResolvedCommandInfo[] };
		};
		if (message.type === "response" && message.command === "get_commands") {
			if (message.success !== true) {
				throw new Error(`get_commands failed: ${line}`);
			}
			return message.data?.commands ?? [];
		}
	}

	throw new Error(
		`no get_commands response in pi output.\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
	);
}
