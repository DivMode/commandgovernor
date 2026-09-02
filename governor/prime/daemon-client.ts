/**
 * A minimal client for Prime Agent's public daemon protocol: JSON lines over a
 * Unix socket, one `daemon_hello` on connect, request/response envelopes keyed
 * by a caller-supplied command id, and unsolicited event envelopes.
 *
 * Deliberately independent of Prime's own `DaemonClient`. The Governor's
 * reliability claims are about the wire, and the Issue #15 harness observed
 * the wire directly for the same reason; this is that ~80-line client, typed.
 *
 * Two properties matter more than convenience:
 *
 * - **The command id is the caller's.** Prime keys its mutation journal by
 *   `clientId + commandId`, so the id must come from the durable ledger that
 *   records the dispatch, never from this class.
 * - **Loss of the socket is reported, never converted into a verdict.** A
 *   pending request rejects with {@link TransportLost}; what that means for a
 *   mutation is decided by `governor/mutation/classify.ts`, which does not look
 *   at the error text.
 */

import { appendFileSync } from "node:fs";
import { createConnection, type Socket } from "node:net";

import {
	type DaemonCommand,
	type DaemonCommandEnvelope,
	type DaemonEventCursor,
	type DaemonEventEnvelope,
	type DaemonHello,
	type DaemonProtocolInfo,
	type DaemonResponse,
	isDaemonEvent,
	isDaemonHello,
	isDaemonResponse,
	PRIME_DAEMON_PROTOCOL,
} from "./protocol.ts";

/** The socket closed (or errored) while a request was outstanding. */
export class TransportLost extends Error {
	readonly kind = "transport_lost" as const;
	constructor(message: string, cause?: unknown) {
		super(message, cause === undefined ? undefined : { cause });
		this.name = "TransportLost";
	}
}

/** No response arrived within the caller's budget. The command may still run. */
export class RequestTimeout extends Error {
	readonly kind = "timeout" as const;
	readonly commandType: string;
	readonly commandId: string;
	readonly timeoutMs: number;
	constructor(commandType: string, commandId: string, timeoutMs: number) {
		super(`timed out after ${timeoutMs} ms waiting for ${commandType} ${commandId}`);
		this.name = "RequestTimeout";
		this.commandType = commandType;
		this.commandId = commandId;
		this.timeoutMs = timeoutMs;
	}
}

/** The daemon on the other end is not the one the pin describes. */
export class SubstrateMismatch extends Error {
	readonly hello: DaemonHello;
	constructor(message: string, hello: DaemonHello) {
		super(message);
		this.name = "SubstrateMismatch";
		this.hello = hello;
	}
}

/** What the connecting side requires of the daemon before it will speak to it. */
export interface ExpectedSubstrate {
	readonly protocol: DaemonProtocolInfo;
	readonly appVersion: string;
	readonly schemaRevision?: number;
}

export interface DaemonClientOptions {
	readonly clientId: string;
	readonly expected: ExpectedSubstrate;
	/**
	 * Wire evidence log (JSONL). `launchEnv` and `env` values are NEVER written;
	 * only their key names. Everything else is recorded verbatim.
	 */
	readonly wireLog?: string;
}

interface Pending {
	readonly commandType: string;
	resolve(response: DaemonResponse): void;
	reject(error: Error): void;
	timer: NodeJS.Timeout;
}

type EventListener = (event: DaemonEventEnvelope) => void;

function redactEnvelope(envelope: DaemonCommandEnvelope): unknown {
	const command: Record<string, unknown> = { ...envelope.command };
	for (const key of ["launchEnv", "env"]) {
		const value = command[key];
		if (value && typeof value === "object") {
			command[key] = { $redacted: true, keys: Object.keys(value as object).sort() };
		}
	}
	return { ...envelope, command };
}

export class DaemonClient {
	readonly clientId: string;
	readonly socketPath: string;
	hello: DaemonHello | undefined;
	lastCursor: DaemonEventCursor | undefined;
	closed = false;

	readonly #expected: ExpectedSubstrate;
	readonly #wireLog: string | undefined;
	readonly #pending = new Map<string, Pending>();
	readonly #listeners = new Set<EventListener>();
	readonly #closeListeners = new Set<(error: Error) => void>();
	readonly #events: DaemonEventEnvelope[] = [];
	#socket: Socket | undefined;
	#buffer = "";

	constructor(socketPath: string, options: DaemonClientOptions) {
		this.socketPath = socketPath;
		this.clientId = options.clientId;
		this.#expected = options.expected;
		this.#wireLog = options.wireLog;
	}

	/** Events received so far, oldest first. */
	get events(): readonly DaemonEventEnvelope[] {
		return this.#events;
	}

	/**
	 * Connect and wait for `daemon_hello`. Refuses -- closing the socket -- if
	 * the hello does not match {@link ExpectedSubstrate}. This is the Prime
	 * equivalent of PR #16's version guard: the check is made before any
	 * command can be sent, so an unpinned daemon can never receive work.
	 */
	connect(timeoutMs = 5000): Promise<DaemonHello> {
		return new Promise<DaemonHello>((resolve, reject) => {
			const socket = createConnection(this.socketPath);
			this.#socket = socket;
			let settled = false;
			const timer = setTimeout(() => {
				if (settled) return;
				settled = true;
				socket.destroy();
				reject(new TransportLost(`no daemon_hello within ${timeoutMs} ms on ${this.socketPath}`));
			}, timeoutMs);
			const onHello = (hello: DaemonHello) => {
				if (settled) return;
				settled = true;
				clearTimeout(timer);
				const mismatch = describeMismatch(hello, this.#expected);
				if (mismatch) {
					socket.destroy();
					reject(new SubstrateMismatch(mismatch, hello));
					return;
				}
				this.hello = hello;
				resolve(hello);
			};
			socket.once("error", (error: Error) => {
				if (!settled) {
					settled = true;
					clearTimeout(timer);
					reject(new TransportLost(`connect failed: ${error.message}`, error));
				}
				this.#failAll(new TransportLost(`socket error: ${error.message}`, error));
			});
			socket.on("close", () => {
				this.closed = true;
				const lost = new TransportLost("socket closed");
				this.#failAll(lost);
				for (const listener of this.#closeListeners) listener(lost);
				if (!settled) {
					settled = true;
					clearTimeout(timer);
					reject(lost);
				}
			});
			socket.on("data", (chunk: Buffer) => {
				this.#buffer += chunk.toString("utf8");
				let newline = this.#buffer.indexOf("\n");
				while (newline >= 0) {
					const line = this.#buffer.slice(0, newline);
					this.#buffer = this.#buffer.slice(newline + 1);
					if (line.trim()) this.#handleLine(line, onHello);
					newline = this.#buffer.indexOf("\n");
				}
			});
		});
	}

	#handleLine(line: string, onHello: (hello: DaemonHello) => void): void {
		let message: unknown;
		try {
			message = JSON.parse(line);
		} catch {
			this.#log("in-unparseable", { line: line.slice(0, 200) });
			return;
		}
		this.#log("in", { message });
		if (isDaemonHello(message)) {
			onHello(message);
			return;
		}
		if (isDaemonResponse(message) && message.id !== undefined) {
			const pending = this.#pending.get(message.id);
			if (pending) {
				this.#pending.delete(message.id);
				clearTimeout(pending.timer);
				pending.resolve(message);
				return;
			}
		}
		if (isDaemonEvent(message)) {
			if (message.cursor) this.lastCursor = message.cursor;
			this.#events.push(message);
			for (const listener of this.#listeners) listener(message);
		}
	}

	#failAll(error: Error): void {
		for (const [, pending] of this.#pending) {
			clearTimeout(pending.timer);
			pending.reject(error);
		}
		this.#pending.clear();
	}

	#log(direction: string, payload: Record<string, unknown>): void {
		if (!this.#wireLog) return;
		appendFileSync(
			this.#wireLog,
			`${JSON.stringify({ at: new Date().toISOString(), direction, client: this.clientId, ...payload })}\n`,
		);
	}

	onEvent(listener: EventListener): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	onClose(listener: (error: Error) => void): () => void {
		this.#closeListeners.add(listener);
		return () => this.#closeListeners.delete(listener);
	}

	/**
	 * Send a command envelope under the caller's id and wait for its response.
	 *
	 * Rejects with {@link TransportLost} or {@link RequestTimeout}; never
	 * resolves with a fabricated response.
	 */
	request(command: DaemonCommand, commandId: string, timeoutMs = 60_000): Promise<DaemonResponse> {
		const socket = this.#socket;
		if (!socket || this.closed || socket.destroyed) {
			return Promise.reject(new TransportLost("not connected"));
		}
		if (this.#pending.has(commandId)) {
			return Promise.reject(new Error(`command id ${commandId} is already in flight on this connection`));
		}
		const envelope: DaemonCommandEnvelope = {
			type: "command",
			id: commandId,
			protocol: PRIME_DAEMON_PROTOCOL,
			clientId: this.clientId,
			command,
		};
		return new Promise<DaemonResponse>((resolve, reject) => {
			const timer = setTimeout(() => {
				this.#pending.delete(commandId);
				reject(new RequestTimeout(command.type, commandId, timeoutMs));
			}, timeoutMs);
			this.#pending.set(commandId, { commandType: command.type, resolve, reject, timer });
			this.#log("out", { message: redactEnvelope(envelope) });
			socket.write(`${JSON.stringify(envelope)}\n`, (error) => {
				if (error) {
					const pending = this.#pending.get(commandId);
					if (pending) {
						this.#pending.delete(commandId);
						clearTimeout(pending.timer);
						pending.reject(new TransportLost(`write failed: ${error.message}`, error));
					}
				}
			});
		});
	}

	/** Wait for an event satisfying `predicate`, including ones already received. */
	waitForEvent(predicate: (event: DaemonEventEnvelope) => boolean, timeoutMs = 30_000): Promise<DaemonEventEnvelope> {
		const already = this.#events.find(predicate);
		if (already) return Promise.resolve(already);
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				off();
				reject(new Error(`no matching event within ${timeoutMs} ms`));
			}, timeoutMs);
			const off = this.onEvent((event) => {
				if (predicate(event)) {
					clearTimeout(timer);
					off();
					resolve(event);
				}
			});
		});
	}

	/** Destroy the socket. Pending requests reject with {@link TransportLost}. */
	close(): void {
		this.#socket?.destroy();
	}
}

function describeMismatch(hello: DaemonHello, expected: ExpectedSubstrate): string | undefined {
	if (hello.protocol.name !== expected.protocol.name || hello.protocol.version !== expected.protocol.version) {
		return `daemon speaks ${hello.protocol.name} v${hello.protocol.version}; pin requires ${expected.protocol.name} v${expected.protocol.version}`;
	}
	if (hello.appVersion !== expected.appVersion) {
		return `daemon reports appVersion ${String(hello.appVersion)}; pin requires ${expected.appVersion}`;
	}
	if (expected.schemaRevision !== undefined && hello.schemaRevision !== expected.schemaRevision) {
		return `daemon reports schemaRevision ${String(hello.schemaRevision)}; pin requires ${expected.schemaRevision}`;
	}
	return undefined;
}

/** Connect, retrying until the daemon answers or the budget is spent. */
export async function connectWithRetry(
	socketPath: string,
	options: DaemonClientOptions,
	budgetMs = 20_000,
): Promise<DaemonClient> {
	const deadline = Date.now() + budgetMs;
	let last: unknown;
	while (Date.now() < deadline) {
		const client = new DaemonClient(socketPath, options);
		try {
			await client.connect(1500);
			return client;
		} catch (error) {
			if (error instanceof SubstrateMismatch) throw error;
			last = error;
			client.close();
			await new Promise((resolve) => setTimeout(resolve, 150));
		}
	}
	throw new TransportLost(`no daemon answered on ${socketPath} within ${budgetMs} ms: ${String(last)}`);
}
