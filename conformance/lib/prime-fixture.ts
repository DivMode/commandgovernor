/**
 * An isolated, disposable Prime Agent supervisor for the runtime tier.
 *
 * Every fixture gets its own HOME, agent directory, session directory,
 * socket, mock model provider and Governor state directory, so runtime tests
 * cannot see each other or the developer's real `~/.prime`. Nothing here
 * touches the default `$TMPDIR/prime-agent-<uid>/daemon.sock`.
 *
 * The root lives under `/tmp` and nowhere else, for one measured reason:
 * Prime's worker sockets sit 50 bytes below `TMPDIR`, and macOS caps a Unix
 * socket path at 104 bytes. The session scratchpad and the default macOS
 * `TMPDIR` both overflow that (the S0 bake-off hit the same `listen EINVAL`).
 * `governor/prime/substrate.ts` checks the length before spawning.
 *
 * Teardown is a check, not a courtesy: `stop()` asks the supervisor to shut
 * down, then sweeps the process table for anything still referencing the
 * fixture root and FAILS if it finds any, after killing them so they cannot
 * outlive the test run.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { Governor, type GovernorOptions } from "../../governor/governor.ts";
import { DEFAULT_LAUNCH_ENV_ALLOWLIST } from "../../governor/prime/env.ts";
import { assertTmpDirFitsSocketPath, isProcessAlive, processesReferencing, shutdownSupervisor, spawnSupervisor, type SpawnedSupervisor } from "../../governor/prime/substrate.ts";
import { REPO_ROOT } from "./repo.ts";

export interface FixtureOptions {
	/** Extra names the fixture may forward to the supervisor (beyond the default allowlist). */
	readonly grant?: readonly string[];
	/** The environment the fixture filters; defaults to `process.env`. */
	readonly sourceEnv?: Readonly<Record<string, string | undefined>>;
}

export interface PrimeFixture {
	readonly root: string;
	readonly home: string;
	readonly agentDir: string;
	readonly sessionDir: string;
	readonly tmpDir: string;
	readonly work: string;
	readonly socketPath: string;
	readonly mockLog: string;
	readonly mockPort: number;
	readonly supervisor: SpawnedSupervisor;
	/** Options a Governor over this fixture is built from. `stateDir` is per call. */
	governorOptions(stateDir?: string, extra?: Partial<GovernorOptions>): GovernorOptions;
	governor(name?: string, extra?: Partial<GovernorOptions>): Promise<Governor>;
	/** Mock provider requests recorded so far. */
	mockRequests(): { kind: string; lastUser?: string; id?: string }[];
	/** Shut down, sweep, remove. Throws if anything survived. */
	stop(): Promise<void>;
}

function waitForLine(child: ChildProcess, prefix: string, timeoutMs: number): Promise<string> {
	return new Promise((resolve, reject) => {
		let buffer = "";
		const timer = setTimeout(() => reject(new Error(`no ${prefix} line from mock provider within ${timeoutMs} ms`)), timeoutMs);
		child.stdout?.on("data", (data: Buffer) => {
			buffer += data.toString("utf8");
			const line = buffer.split("\n").find((candidate) => candidate.startsWith(prefix));
			if (line) {
				clearTimeout(timer);
				resolve(line.slice(prefix.length));
			}
		});
		child.once("exit", (code) => {
			clearTimeout(timer);
			reject(new Error(`mock provider exited with ${String(code)} before reporting its port`));
		});
	});
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export async function startPrimeFixture(options: FixtureOptions = {}): Promise<PrimeFixture> {
	const root = mkdtempSync("/tmp/cg-");
	const home = join(root, "home");
	const agentDir = join(root, "agent");
	const sessionDir = join(root, "sessions");
	const tmpDir = join(root, "tmp");
	const work = join(root, "work");
	const stateRoot = join(root, "governor");
	for (const dir of [home, agentDir, sessionDir, tmpDir, work, stateRoot]) mkdirSync(dir, { recursive: true, mode: 0o700 });
	assertTmpDirFitsSocketPath(tmpDir);
	const socketPath = join(tmpDir, "d.sock");
	const mockLog = join(root, "mock-requests.jsonl");
	writeFileSync(mockLog, "");

	const sourceEnv = options.sourceEnv ?? process.env;
	const mock = spawn(process.execPath, [join(REPO_ROOT, "conformance", "lib", "mock-provider.ts")], {
		env: { PATH: sourceEnv.PATH ?? "", MOCK_PORT: "0", MOCK_LOG: mockLog },
		stdio: ["ignore", "pipe", "inherit"],
	});
	const mockPort = Number(await waitForLine(mock, "MOCK_PORT=", 10_000));
	writeFileSync(
		join(agentDir, "models.json"),
		JSON.stringify({
			providers: {
				mock: {
					baseUrl: `http://127.0.0.1:${mockPort}/v1`,
					api: "openai-completions",
					apiKey: "mock-key",
					models: [{ id: "mock-1", name: "Mock 1", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 128000, maxTokens: 8192 }],
				},
			},
		}),
	);

	const supervisor = spawnSupervisor({
		socketPath,
		agentDir,
		home,
		tmpDir,
		cwd: work,
		logFile: join(root, "supervisor.log"),
		sourceEnv,
		env: { grant: options.grant },
	});

	const fixture: PrimeFixture = {
		root,
		home,
		agentDir,
		sessionDir,
		tmpDir,
		work,
		socketPath,
		mockLog,
		mockPort,
		supervisor,
		governorOptions(stateDir = join(stateRoot, "default"), extra = {}) {
			return {
				stateDir,
				socketPath,
				agentDir,
				home,
				tmpDir,
				sessionDir,
				cwd: work,
				provider: "mock",
				model: "mock-1",
				sourceEnv,
				env: { allowlist: DEFAULT_LAUNCH_ENV_ALLOWLIST, grant: options.grant },
				wireLog: join(root, "wire.jsonl"),
				...extra,
			};
		},
		async governor(name = "default", extra = {}) {
			const governor = new Governor(fixture.governorOptions(join(stateRoot, name), extra));
			await governor.connect(30_000);
			return governor;
		},
		mockRequests() {
			if (!existsSync(mockLog)) return [];
			return readFileSync(mockLog, "utf8")
				.split("\n")
				.filter(Boolean)
				.map((line) => JSON.parse(line) as { kind: string; lastUser?: string; id?: string })
				.filter((entry) => entry.kind === "request");
		},
		async stop() {
			await shutdownSupervisor(socketPath, "cg-fixture-stop");
			mock.kill("SIGKILL");
			// Give the supervisor's own shutdown a moment to reap its workers before we look.
			let survivors = processesReferencing(root);
			for (let attempt = 0; attempt < 30 && survivors.length > 0; attempt += 1) {
				await sleep(200);
				survivors = processesReferencing(root).filter((record) => isProcessAlive(record.pid));
			}
			if (survivors.length > 0) {
				for (const record of survivors) {
					try {
						process.kill(record.pid, "SIGKILL");
					} catch {
						// Already gone.
					}
				}
				throw new Error(`fixture ${root}: ${survivors.length} process(es) survived shutdown and were killed:\n${survivors.map((r) => `  ${r.pid} ${r.command.slice(0, 160)}`).join("\n")}`);
			}
			if (process.env.CG_KEEP_FIXTURE) {
				console.error(`fixture kept at ${root}`);
				return;
			}
			// The supervisor may still be flushing its log for a few hundred ms after its socket is gone.
			rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
		},
	};
	return fixture;
}

/** Lines in a file, for "did the effect happen exactly once" assertions. */
export function lineCount(path: string): number {
	if (!existsSync(path)) return 0;
	return readFileSync(path, "utf8").split("\n").filter(Boolean).length;
}

export async function waitUntil<T>(probe: () => Promise<T | undefined | false> | T | undefined | false, timeoutMs: number, everyMs = 100): Promise<T> {
	const deadline = Date.now() + timeoutMs;
	for (;;) {
		const value = await probe();
		if (value) return value;
		if (Date.now() > deadline) throw new Error(`condition not met within ${timeoutMs} ms`);
		await sleep(everyMs);
	}
}

export { sleep };
