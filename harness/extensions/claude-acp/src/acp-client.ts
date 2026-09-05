/**
 * A minimal Agent Client Protocol client over stdio.
 *
 * ACP (https://agentclientprotocol.com) is JSON-RPC 2.0, newline-delimited,
 * bidirectional: the client drives `initialize` / `session/new` /
 * `session/prompt`, and the agent sends requests back — permission prompts
 * above all — that must be answered or the turn hangs.
 *
 * Written against the wire rather than against `@agentclientprotocol/sdk` on
 * purpose. Prime exposes only its own `pi-*` and `typebox` modules to
 * extensions (`core/extensions/bundled-modules.js`), so an SDK import would
 * have to be resolved from a `node_modules` tree an extension installed for
 * itself. The protocol surface this client needs is four requests, one
 * notification and two inbound request kinds; a dependency-free implementation
 * of that is smaller than the machinery required to install one.
 *
 * This file knows nothing about Claude. It is the transport; `index.ts` is the
 * policy.
 */

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface, type Interface } from "node:readline";

/** How long the handshake may take before the child is declared unusable. */
const HANDSHAKE_TIMEOUT_MS = 60_000;

export const ACP_PROTOCOL_VERSION = 1;

export interface AcpPermissionOption {
	readonly optionId: string;
	readonly name?: string;
	readonly kind?: string;
}

export interface AcpPermissionRequest {
	readonly sessionId: string;
	readonly toolCall?: { readonly title?: string; readonly kind?: string; readonly rawInput?: unknown };
	readonly options: readonly AcpPermissionOption[];
}

export type AcpPermissionOutcome = { readonly outcome: "selected"; readonly optionId: string } | { readonly outcome: "cancelled" };

export interface AcpClientHandlers {
	/** Every `session/update` notification, replayed history included. */
	onUpdate(sessionId: string, update: Record<string, unknown>): void;
	/** Every other notification, by method name. `_auth/status_update` above all. */
	onNotification?(method: string, params: unknown): void;
	/** Answer an agent-initiated permission prompt. */
	onPermission(request: AcpPermissionRequest): Promise<AcpPermissionOutcome>;
	/** The child exited. `stderr` is the tail kept for diagnostics. */
	onExit(code: number | null, stderr: string): void;
}

interface Pending {
	readonly resolve: (value: unknown) => void;
	readonly reject: (error: Error) => void;
	timer?: NodeJS.Timeout;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * One adapter process and the JSON-RPC conversation with it.
 *
 * The process is long-lived by design: an ACP session outlives a single prompt,
 * and keeping it is what lets the agent keep its own conversation, its own
 * compaction and its own prompt cache across turns. It exits on stdin EOF, so
 * `close()` is a clean shutdown rather than a kill.
 */
export class AcpClient {
	private readonly child: ChildProcessWithoutNullStreams;
	private readonly reader: Interface;
	private readonly pending = new Map<number, Pending>();
	private readonly handlers: AcpClientHandlers;
	private nextId = 1;
	private stderrTail = "";
	private exited = false;

	private constructor(child: ChildProcessWithoutNullStreams, handlers: AcpClientHandlers) {
		this.child = child;
		this.handlers = handlers;
		this.reader = createInterface({ input: child.stdout });
		this.reader.on("line", (line) => this.receive(line));

		child.stderr.on("data", (chunk: Buffer) => {
			this.stderrTail = `${this.stderrTail}${chunk.toString("utf8")}`.slice(-4000);
		});
		child.on("close", (code) => {
			this.exited = true;
			this.rejectAll(new Error(`the ACP adapter exited (code ${code})${this.stderrTail ? `: ${this.stderrTail.trim().slice(-600)}` : ""}`));
			handlers.onExit(code, this.stderrTail);
		});
		child.on("error", (error) => {
			this.exited = true;
			this.rejectAll(new Error(`the ACP adapter could not start: ${error.message}`));
		});
	}

	static start(options: { command: string; args: readonly string[]; cwd: string; env: NodeJS.ProcessEnv }, handlers: AcpClientHandlers): AcpClient {
		const child = spawn(options.command, [...options.args], {
			cwd: options.cwd,
			env: options.env,
			stdio: ["pipe", "pipe", "pipe"],
		}) as ChildProcessWithoutNullStreams;
		return new AcpClient(child, handlers);
	}

	get alive(): boolean {
		return !this.exited;
	}

	/** The child's process id, for diagnostics and for tests that inspect it. */
	get pid(): number | undefined {
		return this.child.pid;
	}

	/**
	 * Send a request and await its response.
	 *
	 * `timeoutMs` bounds handshake steps only. `session/prompt` is deliberately
	 * unbounded here: it is the work, and its cancellation path is
	 * `session/cancel`, not a timer that would leave the agent running.
	 */
	request<T = unknown>(method: string, params: unknown, timeoutMs?: number): Promise<T> {
		if (this.exited) return Promise.reject(new Error(`the ACP adapter is not running (${method})`));
		const id = this.nextId++;
		return new Promise<T>((resolve, reject) => {
			const entry: Pending = { resolve: resolve as (value: unknown) => void, reject };
			if (timeoutMs !== undefined) {
				entry.timer = setTimeout(() => {
					this.pending.delete(id);
					reject(new Error(`${method} timed out after ${timeoutMs}ms`));
				}, timeoutMs);
			}
			this.pending.set(id, entry);
			this.write({ jsonrpc: "2.0", id, method, params });
		});
	}

	notify(method: string, params: unknown): void {
		if (this.exited) return;
		this.write({ jsonrpc: "2.0", method, params });
	}

	/** Close stdin, which is how an ACP agent is asked to exit. */
	close(): void {
		this.reader.close();
		try {
			this.child.stdin.end();
		} catch {
			/* already closed */
		}
	}

	/** Last resort when the child ignores stdin EOF. */
	kill(): void {
		this.close();
		try {
			this.child.kill("SIGKILL");
		} catch {
			/* already gone */
		}
	}

	private write(message: Record<string, unknown>): void {
		try {
			this.child.stdin.write(`${JSON.stringify(message)}\n`);
		} catch {
			/* the close handler rejects everything in flight */
		}
	}

	private rejectAll(error: Error): void {
		for (const entry of this.pending.values()) {
			clearTimeout(entry.timer);
			entry.reject(error);
		}
		this.pending.clear();
	}

	private receive(line: string): void {
		let message: unknown;
		try {
			message = JSON.parse(line);
		} catch {
			return; // an ACP agent may print non-JSON on stdout only by mistake; ignore it
		}
		if (!isRecord(message)) return;

		if (typeof message.method === "string" && message.id !== undefined) {
			void this.handleServerRequest(message.id, message.method, message.params);
			return;
		}
		if (typeof message.method === "string") {
			if (message.method === "session/update" && isRecord(message.params)) {
				const sessionId = typeof message.params.sessionId === "string" ? message.params.sessionId : "";
				if (isRecord(message.params.update)) this.handlers.onUpdate(sessionId, message.params.update);
			} else {
				this.handlers.onNotification?.(message.method, message.params);
			}
			return;
		}
		if (message.id === undefined) return;

		const entry = this.pending.get(message.id as number);
		if (!entry) return;
		this.pending.delete(message.id as number);
		clearTimeout(entry.timer);
		if ("error" in message) {
			const error = isRecord(message.error) ? message.error : {};
			entry.reject(new Error(typeof error.message === "string" ? error.message : `request ${String(message.id)} failed`));
		} else {
			entry.resolve((message as { result?: unknown }).result);
		}
	}

	/**
	 * Answer a request the agent sent us.
	 *
	 * Everything must be answered — an unanswered request wedges the turn. The
	 * only one this client implements is the permission prompt, which is the
	 * whole reason it exists; every other method is declined explicitly, and the
	 * client's `initialize` declares no filesystem or terminal capability so a
	 * well-behaved agent never asks for those in the first place.
	 */
	private async handleServerRequest(id: unknown, method: string, params: unknown): Promise<void> {
		if (method === "session/request_permission" && isRecord(params) && Array.isArray(params.options)) {
			let outcome: AcpPermissionOutcome = { outcome: "cancelled" };
			try {
				outcome = await this.handlers.onPermission({
					sessionId: String(params.sessionId ?? ""),
					toolCall: isRecord(params.toolCall) ? (params.toolCall as AcpPermissionRequest["toolCall"]) : undefined,
					options: params.options as AcpPermissionOption[],
				});
			} catch {
				outcome = { outcome: "cancelled" };
			}
			this.write({ jsonrpc: "2.0", id, result: { outcome } });
			return;
		}
		this.write({ jsonrpc: "2.0", id, error: { code: -32601, message: `${method} is not supported by this client` } });
	}
}

/** The `initialize` request body this client sends. */
export function initializeParams(): Record<string, unknown> {
	return {
		protocolVersion: ACP_PROTOCOL_VERSION,
		// No filesystem or terminal proxying: the agent runs on this machine and
		// uses its own. Declaring them false is what keeps `handleServerRequest`
		// honest — the only inbound request left is the permission prompt.
		clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
	};
}

export { HANDSHAKE_TIMEOUT_MS };
