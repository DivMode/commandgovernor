/**
 * The pinned Prime Agent substrate: where its binary is, how to start a
 * supervisor on a socket of our choosing, how to stop it, and how to find
 * anything it left behind.
 *
 * The pin is read from pins/pins.json and nowhere else. Every version string
 * the Governor compares against comes from here.
 */

import { spawn } from "node:child_process";
import { execFileSync } from "node:child_process";
import { existsSync, openSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildLaunchEnv, type LaunchEnvOptions } from "./env.ts";
import { DaemonClient, type ExpectedSubstrate } from "./daemon-client.ts";
import type { DaemonProtocolInfo } from "./protocol.ts";

export const REPO_ROOT: string = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
export const PINS_JSON: string = join(REPO_ROOT, "pins", "pins.json");

export interface PinnedAsset {
	readonly name: string;
	readonly role: "wrapper" | "sibling";
	readonly npmName: string;
	readonly upstreamSpec?: string;
	readonly sha256: string;
	readonly sha512: string;
	readonly bytes: number;
}

export interface SubstratePin {
	readonly name: string;
	readonly version: string;
	readonly tag: string;
	readonly repository: string;
	readonly commit: string;
	readonly license: string;
	readonly releaseBaseUrl: string;
	readonly installRoot: string;
	readonly vendorDir: string;
	readonly binary: string;
	readonly daemonProtocol: DaemonProtocolInfo & { readonly schemaRevision: number };
	readonly engines: { readonly node: string };
	readonly assets: readonly PinnedAsset[];
	readonly checksumAsset: string;
}

export interface PinRecord {
	readonly schemaVersion: number;
	readonly substrate: SubstratePin;
	readonly fallback?: unknown;
	readonly packages: readonly unknown[];
	readonly authorities: string;
}

let cachedPins: PinRecord | undefined;

export function readPins(): PinRecord {
	if (!cachedPins) {
		cachedPins = JSON.parse(readFileSync(PINS_JSON, "utf8")) as PinRecord;
	}
	return cachedPins;
}

export function substratePin(): SubstratePin {
	return readPins().substrate;
}

/** What every Governor connection demands of a daemon, derived from the pin. */
export function expectedSubstrate(): ExpectedSubstrate {
	const pin = substratePin();
	return {
		protocol: { name: pin.daemonProtocol.name, version: pin.daemonProtocol.version },
		appVersion: pin.version,
		schemaRevision: pin.daemonProtocol.schemaRevision,
	};
}

/** Absolute path to the pinned `prime-agent` entrypoint (the bundled CLI). */
export function primeCliEntrypoint(): string {
	const pin = substratePin();
	const entry = join(REPO_ROOT, pin.installRoot, "node_modules", "prime-agent", "dist", "bundle", "cli.js");
	if (!existsSync(entry)) {
		throw new Error(`pinned Prime Agent is not installed at ${pin.installRoot}; run scripts/bootstrap.sh`);
	}
	return entry;
}

/**
 * macOS limits a Unix socket path to 104 bytes (`sun_path`), Linux to 108.
 * Prime derives its worker sockets as
 * `<TMPDIR>/prime-agent-<uid>/worker-<12 hex>-<12 hex>.sock`, 50 bytes past
 * `TMPDIR`, so a `TMPDIR` longer than ~50 bytes makes every worker fail with a
 * bare `EINVAL`. Measured during Issue #17; checked here so the failure is
 * named before a supervisor is started.
 */
export const SUN_PATH_LIMIT = 104;
export const PRIME_WORKER_SOCKET_SUFFIX_BYTES = "/prime-agent-99999/worker-000000000000-000000000000.sock".length;

export function assertTmpDirFitsSocketPath(tmpDir: string): void {
	const worst = Buffer.byteLength(tmpDir) + PRIME_WORKER_SOCKET_SUFFIX_BYTES;
	if (worst >= SUN_PATH_LIMIT) {
		throw new Error(
			`TMPDIR ${tmpDir} is too long for Prime's worker sockets (${worst} bytes >= ${SUN_PATH_LIMIT} sun_path limit); use a shorter directory`,
		);
	}
}

export interface SupervisorSpec {
	/** Socket the supervisor listens on. Unique per Governor instance. */
	readonly socketPath: string;
	/** Prime agent directory (state root): sessions, journals, leases, logs. */
	readonly agentDir: string;
	/** `HOME` for the supervisor and its workers. */
	readonly home: string;
	/** `TMPDIR` for the supervisor; worker sockets are created under it. */
	readonly tmpDir: string;
	/** Working directory for the supervisor process. */
	readonly cwd: string;
	/** Where the supervisor's stdout/stderr go (a log file path). */
	readonly logFile: string;
	/** Env grants beyond the default allowlist, if a profile needs them. */
	readonly env?: LaunchEnvOptions;
	/** The source environment to filter. Defaults to `process.env`. */
	readonly sourceEnv?: Readonly<Record<string, string | undefined>>;
}

export interface SpawnedSupervisor {
	readonly pid: number;
	readonly socketPath: string;
	/** Names of variables that were in the source env and NOT forwarded. */
	readonly withheldEnv: readonly string[];
	/** The exact environment handed to the process. */
	readonly env: Readonly<Record<string, string>>;
}

/**
 * Start a detached Prime supervisor on `socketPath` with an allowlisted
 * environment. The process outlives this one by design (that is what a
 * supervisor is for); stop it with {@link shutdownSupervisor}.
 */
export function spawnSupervisor(spec: SupervisorSpec): SpawnedSupervisor {
	assertTmpDirFitsSocketPath(spec.tmpDir);
	const built = buildLaunchEnv(spec.sourceEnv ?? process.env, {
		...spec.env,
		overrides: {
			HOME: spec.home,
			TMPDIR: spec.tmpDir,
			PRIME_AGENT_CODING_AGENT_DIR: spec.agentDir,
			PRIME_AGENT_TELEMETRY: "0",
			PRIME_AGENT_INSTALL_UV: "0",
			...spec.env?.overrides,
		},
	});
	const out = openSync(spec.logFile, "a");
	const child = spawn(process.execPath, [primeCliEntrypoint(), "--mode", "daemon", "--daemon-socket", spec.socketPath], {
		cwd: spec.cwd,
		env: built.env,
		detached: true,
		stdio: ["ignore", out, out],
	});
	child.unref();
	if (child.pid === undefined) {
		throw new Error("failed to spawn the Prime supervisor");
	}
	return { pid: child.pid, socketPath: spec.socketPath, withheldEnv: built.withheld, env: built.env };
}

/** Ask a supervisor to shut down everything it owns. Returns once the socket is gone. */
export async function shutdownSupervisor(socketPath: string, clientId: string, timeoutMs = 20_000): Promise<void> {
	const client = new DaemonClient(socketPath, { clientId, expected: expectedSubstrate() });
	try {
		await client.connect(3000);
		await client.request({ type: "shutdown", force: true }, `${clientId}-shutdown-${Date.now()}`, timeoutMs);
	} catch {
		// Either nothing is listening or it died mid-shutdown; the sweep below is the check.
	} finally {
		client.close();
	}
	const deadline = Date.now() + timeoutMs;
	while (existsSync(socketPath) && Date.now() < deadline) {
		await new Promise((resolve) => setTimeout(resolve, 100));
	}
}

export interface ProcessRecord {
	readonly pid: number;
	readonly command: string;
}

/**
 * Every live process whose command line mentions `marker`. Read-only: `ps`
 * output, nothing signalled. Used to prove a fixture left nothing behind.
 */
export function processesReferencing(marker: string): ProcessRecord[] {
	const output = execFileSync("ps", ["-axo", "pid=,command="], { encoding: "utf8" });
	const found: ProcessRecord[] = [];
	for (const line of output.split("\n")) {
		const match = /^\s*(\d+)\s+(.*)$/.exec(line);
		if (!match) continue;
		const pid = Number(match[1]);
		const command = match[2] ?? "";
		if (pid === process.pid) continue;
		if (command.includes(marker)) found.push({ pid, command });
	}
	return found;
}

export function isProcessAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch (error) {
		return (error as NodeJS.ErrnoException).code === "EPERM";
	}
}
