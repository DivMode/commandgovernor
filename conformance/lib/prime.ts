/**
 * An isolated, disposable Prime Agent root driven ONLY through stock clients.
 *
 * Every runtime test in this suite is black-box: it starts the pinned
 * `prime-agent` on a socket of its own and then types the commands a user
 * types. Nothing here constructs a daemon command that a stock client would
 * not send, so an assertion that passes here is a statement about the product,
 * not about a private protocol.
 *
 * Four measured facts shape this file, and each one defeats the obvious
 * approach:
 *
 * 1. **The root must live directly under `/tmp`.** Prime's worker sockets sit
 *    ~50 bytes below `TMPDIR` and macOS caps a Unix socket path at 104 bytes.
 *    The default macOS `TMPDIR` already overflows that.
 * 2. **`prime-agent shutdown` cannot be pointed at a non-default socket.**
 *    Its option parser rejects `--daemon-socket`, and its "shut everything
 *    down" path scans the *default* socket directory only. Teardown therefore
 *    records what the CLI says and then sends the supervisor its own public
 *    `shutdown` command on the socket this fixture owns.
 * 3. **A `ps` command-line sweep cannot see Prime's own processes.** Prime
 *    sets `process.title = "prime-agent"`, which replaces the argv `ps`
 *    reports: a supervisor started with `--daemon-socket /tmp/cg-XXXX/...`
 *    shows up as the bare string `prime-agent`. A sweep for the fixture root
 *    matches only the non-node children (uv, /bin/bash, the kernel python).
 *    `relatedProcesses()` therefore walks the process *tree* from pids we
 *    recorded plus the worker pids Prime persists in its own descriptors under
 *    `<agentDir>/daemon-workers/`, and only then falls back to matching.
 * 4. **`prime-agent --daemon-socket <p> <subcommand>` is silently a chat
 *    message.** Prime rewrites a leading `--daemon-socket` only for `stop` and
 *    `rename`; anything else falls through to the run path. `socketArgs()`
 *    places the flag where Prime's parser actually accepts it.
 */

import { execFileSync, spawn, spawnSync, type ChildProcess, type SpawnSyncReturns } from "node:child_process";
import {
	appendFileSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	openSync,
	readdirSync,
	readFileSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createConnection } from "node:net";
import { join } from "node:path";

import { primeCliEntry, readPins, REPO_ROOT, type PinRecord } from "./repo.ts";

export const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const LIB_DIR = join(REPO_ROOT, "conformance", "lib");
const MOCK_PROVIDER = join(LIB_DIR, "mock-provider.ts");
const PTY_RUNNER = join(LIB_DIR, "ptyrun.py");

/**
 * Where roots created by this process are recorded, so `scripts/conformance.sh`
 * can sweep exactly the roots this run made and leave any other agent's
 * `/tmp/cg-*` fixture alone.
 */
const RUN_MANIFEST = process.env.CG_RUN_MANIFEST;

/**
 * Prime's Python kernel venv (uv + CPython + prime-agent-runtime, ~hundreds of
 * MB, network on first use). It is a toolchain cache, not session state, so
 * every fixture shares one: bootstrapping it per root would multiply the
 * suite's wall time and its network use by the number of roots. Overridable so
 * CI can point it at a cached, version-keyed path.
 */
export function kernelVenv(pins: PinRecord): string {
	return process.env.CG_KERNEL_VENV ?? join(process.env.TMPDIR ?? "/tmp", `cg-kernel-venv-${pins.substrate.version}`);
}

export interface SessionRow {
	readonly sessionId?: string;
	readonly activeSessionId?: string;
	readonly sessionFile?: string;
	readonly workerState?: string;
	readonly workerPid?: number;
	readonly messageCount?: number;
	readonly attachedClients?: number;
	readonly isRunningTools?: boolean;
	readonly unfinishedActionCount?: number;
	readonly activity?: string;
}

export interface DaemonHello {
	readonly type: "daemon_hello";
	readonly protocol: { readonly name: string; readonly version: number };
	readonly schemaId?: string;
	readonly schemaRevision?: number;
	readonly appVersion?: string;
	readonly supervisorPid?: number;
	readonly supervisorGeneration?: number;
	readonly [key: string]: unknown;
}

export interface DaemonResponse {
	readonly type: string;
	readonly id?: string;
	readonly success?: boolean;
	readonly error?: string;
	readonly [key: string]: unknown;
}

export interface MockEntry {
	readonly kind: string;
	readonly id?: string;
	readonly mode?: string;
	readonly lastUser?: string;
	readonly tool?: string;
	readonly [key: string]: unknown;
}

export interface WorkerDescriptor {
	readonly pid?: number;
	readonly rootSessionId?: string;
	readonly sessionFile?: string;
	readonly lifecycle?: string;
	readonly createCommand?: { readonly type?: string; readonly sessionPath?: string };
	readonly [key: string]: unknown;
}

export interface ProcessRow {
	readonly pid: number;
	readonly command: string;
}

/** What teardown found. Anything non-empty or true here is a leak. */
export interface StopReport {
	readonly survivors: ProcessRow[];
	/** A supervisor was STILL answering on this root's socket when we gave up asking it to stop. */
	readonly daemonStillAnswering: boolean;
	/** The root still exists after teardown settled, and CG_KEEP_ROOTS did not ask for that. */
	readonly rootLeaked: boolean;
	/**
	 * Paths that reappeared under the root AFTER it was removed, with the removal
	 * that reclaimed them. A Prime process exiting can rebuild `agent/logs` and
	 * `home/.prime/supervisor-owners` under a root that is already gone, so this
	 * is expected to be non-empty sometimes; it is recorded rather than hidden,
	 * because a root that keeps coming back is a leak and this is the evidence.
	 */
	readonly recreatedAfterRemoval: string[];
}

export interface CliResult {
	readonly status: number | null;
	readonly stdout: string;
	readonly stderr: string;
}

export interface CliOptions {
	readonly timeout?: number;
	readonly input?: string;
	readonly extraEnv?: Record<string, string>;
	/** Run in this directory instead of the fixture's default work tree. */
	readonly cwd?: string;
	/**
	 * Do not pass `--daemon-socket`. Some subcommands (`package install`, for
	 * one) never talk to a daemon and reject the option outright.
	 */
	readonly withoutSocket?: boolean;
}

export interface SpawnOptions {
	readonly extraEnv?: Record<string, string>;
	readonly stdio?: Array<"pipe" | "ignore" | "inherit">;
	readonly cwd?: string;
}

// ---------------------------------------------------------------------------
// Small process/file helpers
// ---------------------------------------------------------------------------

export function alive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch (error) {
		return (error as NodeJS.ErrnoException).code === "EPERM";
	}
}

export function lineCount(path: string): number {
	if (!existsSync(path)) return 0;
	return readFileSync(path, "utf8").split("\n").filter(Boolean).length;
}

export function jsonlFiles(dir: string): string[] {
	if (!existsSync(dir)) return [];
	const found: string[] = [];
	const walk = (current: string): void => {
		for (const name of readdirSync(current)) {
			const path = join(current, name);
			if (statSync(path).isDirectory()) walk(path);
			else if (name.endsWith(".jsonl")) found.push(path);
		}
	};
	walk(dir);
	return found.sort();
}

export async function waitUntil<T>(
	probe: () => T | undefined | Promise<T | undefined>,
	timeoutMs: number,
	everyMs = 200,
	what = "condition",
): Promise<T> {
	const deadline = Date.now() + timeoutMs;
	for (;;) {
		const value = await probe();
		if (value !== undefined && value !== false) return value;
		if (Date.now() > deadline) throw new Error(`${what} not met within ${timeoutMs} ms`);
		await sleep(everyMs);
	}
}

/** Prompt text that makes the mock model ask Prime's own `ipython` tool to append one line. */
export function toolPrompt(target: string, sleepSeconds = 0): string {
	const code =
		`open(${JSON.stringify(target)}, "a").write("effect\\n")` +
		(sleepSeconds > 0 ? `; import time; time.sleep(${sleepSeconds})` : "");
	return `TOOL:ipython|${JSON.stringify({ code })}`;
}

/** Worker descriptors Prime persists under `<agentDir>/daemon-workers`, recursively. */
export function listWorkerDescriptors(agentDir: string): WorkerDescriptor[] {
	const found: WorkerDescriptor[] = [];
	const walk = (dir: string): void => {
		let entries: string[];
		try {
			entries = readdirSync(dir);
		} catch {
			return;
		}
		for (const name of entries) {
			const path = join(dir, name);
			let stats;
			try {
				stats = statSync(path);
			} catch {
				continue;
			}
			if (stats.isDirectory()) {
				walk(path);
				continue;
			}
			if (!name.endsWith(".json")) continue;
			try {
				found.push(JSON.parse(readFileSync(path, "utf8")) as WorkerDescriptor);
			} catch {
				/* a descriptor being rewritten mid-read is not a finding */
			}
		}
	};
	walk(join(agentDir, "daemon-workers"));
	return found;
}

/**
 * One raw daemon request on a socket this fixture owns.
 *
 * Used for exactly two things a stock client cannot do for us: reading the
 * `daemon_hello` a live supervisor sends (to check it against the pin record)
 * and stopping a supervisor whose socket `prime-agent shutdown` refuses to
 * accept. Product invariants are never asserted through this path.
 */
export function daemonRequest(
	socketPath: string,
	command: Record<string, unknown> & { type: string },
	options: { clientId?: string; timeoutMs?: number } = {},
): Promise<{ hello: DaemonHello; response: DaemonResponse }> {
	const clientId = options.clientId ?? "cg-conformance-observer";
	const timeoutMs = options.timeoutMs ?? 20_000;
	return new Promise((resolve, reject) => {
		const socket = createConnection(socketPath);
		let buffer = "";
		let hello: DaemonHello | undefined;
		const id = `obs_${Date.now()}_${Math.random().toString(16).slice(2)}`;
		const timer = setTimeout(() => {
			socket.destroy();
			reject(new Error(`daemon request ${command.type} on ${socketPath} timed out after ${timeoutMs} ms`));
		}, timeoutMs);
		socket.on("error", (error) => {
			clearTimeout(timer);
			reject(error);
		});
		socket.on("data", (data: Buffer) => {
			buffer += data.toString("utf8");
			for (;;) {
				const index = buffer.indexOf("\n");
				if (index < 0) break;
				const line = buffer.slice(0, index);
				buffer = buffer.slice(index + 1);
				if (!line.trim()) continue;
				let message: Record<string, unknown>;
				try {
					message = JSON.parse(line) as Record<string, unknown>;
				} catch {
					continue;
				}
				if (message.type === "daemon_hello") {
					hello = message as unknown as DaemonHello;
					socket.write(`${JSON.stringify({ type: "command", id, clientId, protocol: message.protocol, command: { ...command, id } })}\n`);
					continue;
				}
				if (message.type === "response" && message.id === id) {
					clearTimeout(timer);
					socket.end();
					resolve({ hello: hello as DaemonHello, response: message as unknown as DaemonResponse });
					return;
				}
			}
		});
	});
}

/**
 * Does anything accept a connection on this socket?
 *
 * The only way to see a Prime supervisor that a process-tree sweep cannot:
 * Prime retitles its processes, and a supervisor launched by a worker (rather
 * than by this fixture) is neither a recorded pid nor a descendant of one.
 */
export function socketAnswers(path: string, timeoutMs = 3000): Promise<boolean> {
	if (!existsSync(path)) return Promise.resolve(false);
	return new Promise((resolve) => {
		const socket = createConnection(path);
		let settled = false;
		const finish = (answered: boolean): void => {
			if (settled) return;
			settled = true;
			socket.destroy();
			resolve(answered);
		};
		const timer = setTimeout(() => finish(false), timeoutMs);
		socket.on("connect", () => {
			clearTimeout(timer);
			finish(true);
		});
		socket.on("error", () => {
			clearTimeout(timer);
			finish(false);
		});
	});
}

function waitForLine(child: ChildProcess, prefix: string, timeoutMs: number): Promise<string> {
	return new Promise((resolve, reject) => {
		let buffer = "";
		const timer = setTimeout(() => reject(new Error(`no ${prefix} line within ${timeoutMs} ms`)), timeoutMs);
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
			reject(new Error(`mock provider exited ${String(code)} before reporting its port`));
		});
	});
}

/** macOS caps a Unix socket path at 104 bytes; Prime's worker sockets sit well below TMPDIR. */
export function assertTmpDirFitsSocketPath(tmpDir: string): void {
	const worst = "/prime-agent-99999/worker-000000000000-000000000000.sock";
	const total = Buffer.byteLength(tmpDir) + worst.length;
	if (total >= 104) {
		throw new Error(`TMPDIR ${tmpDir} is ${total} bytes with a worker socket name; the platform cap is 104. Roots must live directly under /tmp.`);
	}
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

export interface PtyClient {
	readonly child: ChildProcess;
	readonly logPath: string;
	/** The pty log with escape sequences stripped, i.e. roughly what a user sees. */
	screen(): string;
	type(text: string): void;
	submit(text: string, settleMs?: number): Promise<void>;
	kill(): void;
}

export interface ReopenResult {
	readonly row: SessionRow;
	readonly client: PtyClient;
	readonly attempts: number;
	readonly crashes: { attempt: number; tail: string }[];
}

export interface PrimeRoot {
	readonly pins: PinRecord;
	readonly root: string;
	readonly home: string;
	readonly agentDir: string;
	readonly sessionDir: string;
	readonly tmpDir: string;
	readonly work: string;
	readonly socketPath: string;
	readonly mockPort: number;
	readonly supervisorPid: number;
	readonly env: Record<string, string>;
	/** Whatever the stock `prime-agent shutdown` said during teardown. */
	shutdownCliOutput: string;
	note(...parts: unknown[]): void;
	socketArgs(args: readonly string[]): string[];
	cli(args: readonly string[], options?: CliOptions): CliResult;
	cliSpawn(args: readonly string[], options?: SpawnOptions): ChildProcess;
	/** Register a pid this fixture is responsible for cleaning up. */
	adopt(pid: number | undefined): void;
	mockRequests(): MockEntry[];
	modelCalls(): MockEntry[];
	toolCalls(): MockEntry[];
	commandJournal(): unknown[];
	workerPidsFromDescriptors(): number[];
	relatedProcesses(): ProcessRow[];
	/**
	 * Shut the supervisor down, sweep survivors (killing them), remove the root.
	 * Idempotent, so a test can assert on the report and an `after` hook can
	 * still guarantee teardown ran.
	 */
	stop(): Promise<StopReport>;
}

export interface StartRootOptions {
	readonly label?: string;
	readonly dumpTools?: boolean;
	/** Extra environment for every Prime process under this root. */
	readonly extraEnv?: Record<string, string>;
}

export async function startRoot(options: StartRootOptions = {}): Promise<PrimeRoot> {
	const label = options.label ?? "cg";
	const pins = readPins();
	const cliEntry = primeCliEntry(pins);

	const root = mkdtempSync("/tmp/cg-");
	if (RUN_MANIFEST) appendFileSync(RUN_MANIFEST, `${root}\n`);
	const home = join(root, "home");
	const agentDir = join(root, "agent");
	const sessionDir = join(root, "sessions");
	const tmpDir = join(root, "tmp");
	const work = join(root, "work");
	for (const dir of [home, agentDir, sessionDir, tmpDir, work]) mkdirSync(dir, { recursive: true, mode: 0o700 });
	assertTmpDirFitsSocketPath(tmpDir);
	const socketPath = join(tmpDir, "d.sock");

	const mockLog = join(root, "mock-requests.jsonl");
	writeFileSync(mockLog, "");
	const mock = spawn(process.execPath, [MOCK_PROVIDER], {
		env: {
			PATH: process.env.PATH ?? "",
			MOCK_PORT: "0",
			MOCK_LOG: mockLog,
			...(options.dumpTools ? { MOCK_DUMP_TOOLS: "1" } : {}),
		},
		stdio: ["ignore", "pipe", "inherit"],
	});
	const mockPort = Number(await waitForLine(mock, "MOCK_PORT=", 20_000));

	writeFileSync(
		join(agentDir, "models.json"),
		JSON.stringify({
			providers: {
				mock: {
					baseUrl: `http://127.0.0.1:${mockPort}/v1`,
					api: "openai-completions",
					apiKey: "mock-key",
					models: [
						{
							id: "mock-1",
							name: "Mock 1",
							reasoning: false,
							input: ["text"],
							cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
							contextWindow: 128000,
							maxTokens: 8192,
						},
					],
				},
			},
		}),
	);

	const env: Record<string, string> = {
		PATH: process.env.PATH ?? "",
		HOME: home,
		TMPDIR: tmpDir,
		PRIME_AGENT_CODING_AGENT_DIR: agentDir,
		PRIME_AGENT_TELEMETRY: "0",
		// uv is a prerequisite of the runtime tier, installed by the developer or
		// by CI. `0` forbids Prime from curl-piping its own installer into the
		// fixture HOME, which would be an unpinned network install inside a test.
		PRIME_AGENT_INSTALL_UV: "0",
		PRIME_AGENT_KERNEL_VENV: kernelVenv(pins),
		NO_COLOR: "1",
		TERM: "dumb",
		...(options.extraEnv ?? {}),
	};

	// Kept inside the root rather than exposed as an accessor: it is diagnostic
	// material for a failing run inspected with CG_KEEP_ROOTS=1, not something an
	// assertion should read — the supervisor's log format is not a contract.
	const supervisorLogPath = join(root, "supervisor.log");
	writeFileSync(supervisorLogPath, "");
	const supervisorOut = openSync(supervisorLogPath, "a");
	const supervisor = spawn(process.execPath, [cliEntry, "--mode", "daemon", "--daemon-socket", socketPath], {
		cwd: work,
		env,
		detached: true,
		stdio: ["ignore", supervisorOut, supervisorOut],
	});
	supervisor.unref();
	const supervisorPid = supervisor.pid;
	if (supervisorPid === undefined) throw new Error("the pinned prime-agent supervisor did not start");
	const spawned: number[] = [supervisorPid];
	let stopped = false;
	let lastReport: StopReport = { survivors: [], daemonStillAnswering: false, rootLeaked: false, recreatedAfterRemoval: [] };

	await waitUntil(() => existsSync(socketPath), 60_000, 100, `daemon socket ${socketPath}`);

	const readMock = (): MockEntry[] => {
		if (!existsSync(mockLog)) return [];
		return readFileSync(mockLog, "utf8")
			.split("\n")
			.filter(Boolean)
			.map((line) => JSON.parse(line) as MockEntry);
	};

	const workerPidsFromDescriptors = (): number[] =>
		listWorkerDescriptors(agentDir)
			.map((descriptor) => descriptor.pid)
			.filter((pid): pid is number => typeof pid === "number");

	const relatedProcesses = (): ProcessRow[] => {
		const output = execFileSync("ps", ["-axww", "-o", "pid=,ppid=,command="], { encoding: "utf8" });
		const rows: { pid: number; ppid: number; command: string }[] = [];
		for (const line of output.split("\n")) {
			const match = /^\s*(\d+)\s+(\d+)\s+(.*)$/.exec(line);
			if (match) rows.push({ pid: Number(match[1]), ppid: Number(match[2]), command: match[3] ?? "" });
		}
		const children = new Map<number, number[]>();
		for (const row of rows) children.set(row.ppid, [...(children.get(row.ppid) ?? []), row.pid]);
		const byPid = new Map(rows.map((row) => [row.pid, row]));
		const seeds = new Set<number>([...spawned, ...workerPidsFromDescriptors()]);
		for (const row of rows) if (row.command.includes(root)) seeds.add(row.pid);
		const found = new Set<number>();
		const visit = (pid: number): void => {
			if (found.has(pid) || pid === process.pid || pid <= 1) return;
			if (!byPid.has(pid)) return;
			found.add(pid);
			for (const child of children.get(pid) ?? []) visit(child);
		};
		for (const pid of seeds) visit(pid);
		return [...found].map((pid) => ({ pid, command: byPid.get(pid)?.command ?? "" }));
	};

	const fixture: PrimeRoot = {
		pins,
		root,
		home,
		agentDir,
		sessionDir,
		tmpDir,
		work,
		socketPath,
		mockPort,
		supervisorPid,
		env,
		shutdownCliOutput: "",

		/** Record an observation. Never throws: a note must not be able to fail a test. */
		note(...parts: unknown[]): void {
			const line = parts.map((part) => (typeof part === "string" ? part : JSON.stringify(part))).join(" ");
			try {
				appendFileSync(join(root, "notes.log"), `${line}\n`);
			} catch {
				/* the root is gone (teardown ran); CG_VERBOSE still shows the line */
			}
			if (process.env.CG_VERBOSE) console.log(`[${label}] ${line}`);
		},

		socketArgs(args: readonly string[]): string[] {
			if (args.length === 0 || args[0].startsWith("-")) return ["--daemon-socket", socketPath, ...args];
			const separator = args.indexOf("--");
			if (separator === -1) return [...args, "--daemon-socket", socketPath];
			return [...args.slice(0, separator), "--daemon-socket", socketPath, ...args.slice(separator)];
		},

		cli(args: readonly string[], cliOptions: CliOptions = {}): CliResult {
			const argv = cliOptions.withoutSocket ? [...args] : fixture.socketArgs(args);
			const result: SpawnSyncReturns<string> = spawnSync(process.execPath, [cliEntry, ...argv], {
				cwd: cliOptions.cwd ?? work,
				env: { ...env, ...(cliOptions.extraEnv ?? {}) },
				encoding: "utf8",
				timeout: cliOptions.timeout ?? 120_000,
				...(cliOptions.input === undefined ? {} : { input: cliOptions.input }),
			});
			return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
		},

		cliSpawn(args: readonly string[], spawnOptions: SpawnOptions = {}): ChildProcess {
			const child = spawn(process.execPath, [cliEntry, ...fixture.socketArgs(args)], {
				cwd: spawnOptions.cwd ?? work,
				env: { ...env, ...(spawnOptions.extraEnv ?? {}) },
				stdio: spawnOptions.stdio ?? ["pipe", "pipe", "pipe"],
			});
			fixture.adopt(child.pid);
			child.stdin?.on("error", () => {});
			child.on("error", () => {});
			return child;
		},

		adopt(pid: number | undefined): void {
			if (typeof pid === "number") spawned.push(pid);
		},

		mockRequests: readMock,
		modelCalls: () => readMock().filter((entry) => entry.kind === "request"),
		toolCalls: () => readMock().filter((entry) => entry.kind === "response" && entry.mode === "tool_call"),

		commandJournal(): unknown[] {
			const entries: unknown[] = [];
			const walk = (dir: string): void => {
				let names: string[];
				try {
					names = readdirSync(dir);
				} catch {
					return;
				}
				for (const name of names) {
					const path = join(dir, name);
					if (statSync(path).isDirectory()) {
						walk(path);
						continue;
					}
					if (name !== "command-journal.jsonl") continue;
					for (const line of readFileSync(path, "utf8").split("\n").filter(Boolean)) {
						try {
							entries.push(JSON.parse(line));
						} catch {
							/* a partially written line is not a finding */
						}
					}
				}
			};
			walk(join(agentDir, "daemon-workers"));
			return entries;
		},

		workerPidsFromDescriptors,
		relatedProcesses,

		async stop(): Promise<StopReport> {
			if (stopped) return lastReport;
			stopped = true;
			// Recorded, not relied on: the stock command cannot target this socket.
			const cliShutdown = fixture.cli(["shutdown", "--force", "--json"], { timeout: 60_000 });
			fixture.shutdownCliOutput = `${cliShutdown.stdout.trim()}${cliShutdown.stderr.trim()}`.slice(0, 300);

			// One shutdown is not necessarily the last one. A live worker launches a
			// REPLACEMENT supervisor on the same socket when the current one exits --
			// that is exactly the recovery this suite asserts elsewhere -- and the
			// replacement is neither a pid we recorded nor a descendant of one, so
			// the process-tree sweep is blind to it. Measured consequence of not
			// doing this: the root was removed and then partially recreated by a
			// supervisor still starting up underneath it. So ask until the socket
			// stops answering, and only then take the root away.
			let daemonStillAnswering = true;
			for (let attempt = 0; attempt < 16; attempt += 1) {
				if (!(await socketAnswers(socketPath))) {
					daemonStillAnswering = false;
					break;
				}
				try {
					await daemonRequest(socketPath, { type: "shutdown", force: true }, { clientId: `${label}-teardown`, timeoutMs: 20_000 });
				} catch {
					/* it may have gone away mid-request; the next probe decides */
				}
				await sleep(750);
			}
			mock.kill("SIGKILL");

			let survivors = relatedProcesses();
			for (let attempt = 0; attempt < 80 && survivors.length > 0; attempt += 1) {
				await sleep(250);
				survivors = relatedProcesses().filter((row) => alive(row.pid));
			}
			for (const survivor of survivors) {
				try {
					process.kill(survivor.pid, "SIGKILL");
				} catch {
					/* already gone between the sweep and the kill */
				}
			}

			let rootLeaked = false;
			const recreatedAfterRemoval: string[] = [];
			if (!process.env.CG_KEEP_ROOTS) {
				// A Prime process that is still exiting writes its log directory back
				// under a root that has just been removed (measured: `agent/logs/` and
				// `home/.prime/supervisor-owners/` reappear, empty of anything else).
				// So removal is a loop that settles, not a single call: take the root
				// away, and require it to STAY away for two consecutive checks. What
				// came back in between is recorded, because a root that keeps
				// reappearing is a real leak and this names what is rebuilding it.
				let absentInARow = 0;
				for (let attempt = 0; attempt < 24 && absentInARow < 2; attempt += 1) {
					if (existsSync(root)) {
						if (attempt > 0) {
							try {
								for (const name of readdirSync(root)) recreatedAfterRemoval.push(name);
							} catch {
								/* it vanished under us, which is the outcome we want */
							}
						}
						try {
							rmSync(root, { recursive: true, force: true });
						} catch {
							/* the existsSync checks are the verdict, not the throw */
						}
						absentInARow = 0;
					} else {
						absentInARow += 1;
					}
					if (absentInARow < 2) await sleep(250);
				}
				rootLeaked = existsSync(root);
			}
			lastReport = { survivors, daemonStillAnswering, rootLeaked, recreatedAfterRemoval: [...new Set(recreatedAfterRemoval)] };
			return lastReport;
		},
	};

	return fixture;
}

// ---------------------------------------------------------------------------
// Stock client surfaces
// ---------------------------------------------------------------------------

/**
 * Flags every fixture client carries: the mock provider, and every discovery
 * mechanism off, so a developer's own extensions, skills, themes, prompt
 * templates or AGENTS.md cannot change what a test measures.
 *
 * `conformance/runtime/package-load.test.ts` deliberately does NOT use these:
 * its whole subject is what discovery finds, and `-ne`/`-ns`/`-np` would turn
 * it off. It is safe there because the fixture's HOME and project are its own.
 */
export const STOCK_CLIENT_FLAGS: readonly string[] = ["--provider", "mock", "--model", "mock-1", "-ne", "-ns", "-nc", "-np", "--no-themes"];

/**
 * A stock `prime-agent` client on a real pty.
 *
 * The interactive TUI is the only stock client that creates a RESIDENT session
 * (print/json/rpc create client-owned ones), and it refuses to run without a
 * tty, so it gets one.
 */
export function ptyCli(fixture: PrimeRoot, cliArgs: readonly string[], options: { name?: string } = {}): PtyClient {
	const name = options.name ?? "tui";
	const logPath = join(fixture.root, `${name}.pty.log`);
	const child = spawn(
		"python3",
		[PTY_RUNNER, logPath, process.execPath, primeCliEntry(fixture.pins), ...fixture.socketArgs(cliArgs)],
		{
			cwd: fixture.work,
			env: { ...fixture.env, TERM: "xterm-256color", COLUMNS: "140", LINES: "45" },
			stdio: ["pipe", "pipe", "pipe"],
		},
	);
	fixture.adopt(child.pid);
	// A pty client that has exited turns the next keystroke into EPIPE; the test
	// must observe that, not die of it.
	child.stdin?.on("error", () => {});
	child.on("error", () => {});
	let ptyStdout = "";
	child.stdout?.on("data", (data: Buffer) => {
		ptyStdout += data.toString("utf8");
		const match = /PTY_PID=(\d+)/.exec(ptyStdout);
		if (match) fixture.adopt(Number(match[1]));
	});
	child.stderr?.on("data", (data: Buffer) => {
		ptyStdout += `[err]${data.toString("utf8")}`;
	});

	return {
		child,
		logPath,
		screen(): string {
			if (!existsSync(logPath)) return "";
			return readFileSync(logPath, "utf8")
				.replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "")
				.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "")
				.replace(/\x1b[()][AB0]/g, "");
		},
		type(text: string): void {
			try {
				child.stdin?.write(text);
			} catch {
				/* the client has exited; screen() carries the reason */
			}
		},
		async submit(text: string, settleMs = 900): Promise<void> {
			try {
				child.stdin?.write(text);
				await sleep(settleMs);
				child.stdin?.write("\r");
			} catch {
				/* as above */
			}
		},
		kill(): void {
			try {
				child.stdin?.end();
			} catch {
				/* already closed */
			}
			try {
				child.kill("SIGKILL");
			} catch {
				/* already gone */
			}
		},
	};
}

/** A fresh resident session in the stock TUI. */
export function startTui(fixture: PrimeRoot, options: { extraArgs?: readonly string[]; name?: string } = {}): PtyClient {
	return ptyCli(fixture, [...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir, ...(options.extraArgs ?? [])], {
		name: options.name ?? "tui",
	});
}

/**
 * A stock TUI holding a fresh RESIDENT session that the daemon reports READY,
 * with a live worker pid.
 *
 * Waiting for a row to merely appear is not enough, and the reason is measured:
 * a supervisor that has just lost its last session begins an idle shutdown, and
 * a TUI that connects during that window gets a session which is archived one
 * turn later — the client prints "The daemon stopped this agent session" and
 * the row loses its worker. Requiring `ready` plus a pid up front, and retrying
 * the whole client if it never gets there, is what a user does when a session
 * dies under them. Tests that need a clean supervisor lifecycle should also use
 * a root per experiment rather than sharing one.
 */
export async function startResidentSession(
	fixture: PrimeRoot,
	options: { name: string; known?: ReadonlySet<string>; attempts?: number; timeoutMs?: number },
): Promise<{ client: PtyClient; row: SessionRow; attempts: number }> {
	const known = options.known ?? new Set<string>();
	const attempts = options.attempts ?? 3;
	const timeoutMs = options.timeoutMs ?? 90_000;
	const failures: string[] = [];
	for (let attempt = 1; attempt <= attempts; attempt += 1) {
		const client = startTui(fixture, { name: `${options.name}-${attempt}` });
		try {
			const row = await waitUntil(
				() =>
					listAgents(fixture).sessions.find(
						(candidate) => candidate.sessionId && !known.has(candidate.sessionId) && candidate.workerState === "ready" && candidate.workerPid,
					),
				timeoutMs,
				400,
				`${options.name}: a ready resident session`,
			);
			return { client, row, attempts: attempt };
		} catch (error) {
			failures.push(`attempt ${attempt}: ${String(error)} :: ${client.screen().slice(-240).replace(/\s+/g, " ")}`);
			client.kill();
			await sleep(2500);
		}
	}
	throw new Error(`${options.name}: no resident session became ready in ${attempts} attempts: ${JSON.stringify(failures)}`);
}

/** Stock `prime-agent list --json` (subcommand form). */
export function listAgents(fixture: PrimeRoot, options: { all?: boolean } = {}): { sessions: SessionRow[]; error?: string } {
	const result = fixture.cli([...(options.all ? ["list", "--all"] : ["list"]), "--json"], { timeout: 60_000 });
	if (result.status !== 0) return { sessions: [], error: (result.stderr || result.stdout).slice(0, 400) };
	try {
		const parsed = JSON.parse(result.stdout) as { sessions?: SessionRow[] } | SessionRow[];
		return { sessions: Array.isArray(parsed) ? parsed : (parsed.sessions ?? []) };
	} catch {
		return { sessions: [], error: `unparseable list output: ${result.stdout.slice(0, 300)}` };
	}
}

export function sessionRow(fixture: PrimeRoot, sessionId: string, options: { all?: boolean } = {}): SessionRow | undefined {
	return listAgents(fixture, options).sessions.find((row) => row.sessionId === sessionId);
}

/**
 * The stock way back into a root whose worker died: `prime-agent -r <sessionFile>`.
 *
 * Retried, and the retries are part of the measurement rather than a way to
 * hide one. The stock interactive client is not robust here: when the now-idle
 * supervisor exits between connect and attach it dies with an UNHANDLED
 * `DaemonSocketClosedError` (or `Error: Session worker is stopping`) and a raw
 * Node stack trace instead of a handled error or a fresh daemon. A user retypes
 * the command; so does this. Every crash is captured in `crashes` so a test can
 * report what it saw rather than silently passing on the second try.
 */
export async function reopenSaved(
	fixture: PrimeRoot,
	sessionFile: string,
	sessionId: string,
	options: { name?: string; excludePid?: number; attempts?: number; timeoutMs?: number } = {},
): Promise<ReopenResult> {
	const name = options.name ?? "reopen";
	const attempts = options.attempts ?? 3;
	const timeoutMs = options.timeoutMs ?? 90_000;
	const crashes: { attempt: number; tail: string }[] = [];
	for (let attempt = 1; attempt <= attempts; attempt += 1) {
		const client = ptyCli(fixture, [...STOCK_CLIENT_FLAGS, "--session-dir", fixture.sessionDir, "-r", sessionFile], {
			name: `${name}-${attempt}`,
		});
		try {
			const row = await waitUntil(
				() => {
					const candidate = sessionRow(fixture, sessionId);
					return candidate && candidate.workerState === "ready" && candidate.workerPid !== options.excludePid ? candidate : undefined;
				},
				timeoutMs,
				500,
				`${name} attempt ${attempt}`,
			);
			return { row, client, attempts: attempt, crashes };
		} catch {
			crashes.push({ attempt, tail: client.screen().slice(-400).replace(/\s+/g, " ") });
			client.kill();
			await sleep(2500);
		}
	}
	throw new Error(`${name}: the stock resume never reopened ${sessionFile} in ${attempts} attempts: ${JSON.stringify(crashes)}`);
}
