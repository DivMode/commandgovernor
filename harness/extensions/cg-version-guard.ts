/**
 * cg-version-guard — refuse to work against an unpinned Pi runtime.
 *
 * Pi has no built-in "this configuration requires pi >= X" mechanism. The `pi`
 * manifest parser accepts exactly four resource arrays and silently drops
 * anything else, so a declared minimum version would be ignored rather than
 * enforced. The check therefore has to be built, and this is it.
 *
 * Three things about Pi shape this file, and each one is load-bearing:
 *
 *   1. Extension factories run in invocations that never start a session
 *      (`pi list`, `pi config`, `pi update`). Failing in the factory would
 *      break package management, so the version comparison happens in
 *      `session_start`.
 *
 *   2. `VERSION` falls back to the string "0.0.0" when the CLI cannot read its
 *      own package.json. That is "unknown", not "ancient", and it gets its own
 *      refusal so the operator is not sent looking for a downgrade that never
 *      happened.
 *
 *   3. `ctx.shutdown()` is deferred to idle in interactive and RPC modes and is
 *      a **no-op in print mode**. It is a request, not a kill. So the guard
 *      also blocks every tool call: a print-mode session that ignores the
 *      shutdown still cannot touch the filesystem, the network, or a shell.
 *      `bin/cg-pi` performs the same check before the process starts, which is
 *      the only place a refusal can be genuinely fatal.
 *
 * The required version comes from `pins/pins.json` and nowhere else. A second
 * hardcoded copy is a second thing to forget to update.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
	VERSION,
	type ExtensionAPI,
	type SessionStartEvent,
	type ToolCallEventResult,
} from "@earendil-works/pi-coding-agent";

/** The string `VERSION` carries when the CLI could not read its own manifest. */
export const UNKNOWN_VERSION = "0.0.0";

/** Stable refusal codes. Conformance asserts on these, not on prose. */
export type GuardRefusalCode =
	| "runtime-version-unknown"
	| "runtime-version-drift"
	| "pin-unreadable";

export type GuardVerdict =
	| { readonly ok: true; readonly version: string }
	| {
			readonly ok: false;
			readonly code: GuardRefusalCode;
			readonly message: string;
	  };

/**
 * Compare the runtime's reported version against the pinned one.
 *
 * Pure, total, and exported so the conformance suite can drive it directly
 * rather than inferring it from a session that happened to survive.
 */
export function evaluateVersion(
	actual: string,
	required: string,
): GuardVerdict {
	if (actual === UNKNOWN_VERSION) {
		return {
			ok: false,
			code: "runtime-version-unknown",
			message:
				`Command Governor requires pi ${required}, but the running pi reports ` +
				`version ${UNKNOWN_VERSION}, which is Pi's fallback for failing to read ` +
				`its own package.json. The runtime is unidentifiable, so the pin cannot ` +
				`be checked and the distribution refuses to run.`,
		};
	}
	if (actual !== required) {
		return {
			ok: false,
			code: "runtime-version-drift",
			message:
				`Command Governor requires pi ${required}; the running pi is ${actual}. ` +
				`Run scripts/bootstrap.sh and launch through bin/cg-pi, or re-pin the ` +
				`distribution deliberately (see docs/pi-distribution.md).`,
		};
	}
	return { ok: true, version: actual };
}

/**
 * Read the pinned version out of `pins/pins.json`.
 *
 * Throws rather than defaulting. A guard that invents a required version when
 * it cannot find one is not a guard.
 */
export function readPinnedVersion(pinsJsonPath: string): string {
	let raw: string;
	try {
		raw = readFileSync(pinsJsonPath, "utf8");
	} catch (cause) {
		throw new Error(
			`cg-version-guard: cannot read the pin record at ${pinsJsonPath}`,
			{ cause },
		);
	}

	let parsed: unknown;
	try {
		parsed = JSON.parse(raw);
	} catch (cause) {
		throw new Error(
			`cg-version-guard: ${pinsJsonPath} is not valid JSON`,
			{ cause },
		);
	}

	const version = extractPiVersion(parsed);
	if (version === undefined) {
		throw new Error(
			`cg-version-guard: ${pinsJsonPath} has no string field pi.version`,
		);
	}
	return version;
}

function extractPiVersion(doc: unknown): string | undefined {
	if (typeof doc !== "object" || doc === null) return undefined;
	const pi = (doc as Record<string, unknown>).pi;
	if (typeof pi !== "object" || pi === null) return undefined;
	const version = (pi as Record<string, unknown>).version;
	return typeof version === "string" ? version : undefined;
}

/** `pins/pins.json`, resolved from this file's own location. */
export function defaultPinsJsonPath(): string {
	return join(dirname(fileURLToPath(import.meta.url)), "..", "..", "pins", "pins.json");
}

/**
 * The parts of `ExtensionContext` this guard actually uses.
 *
 * Narrowing it here is what lets the conformance suite hand the guard a fake
 * and observe every branch, without the fake having to impersonate the whole
 * runtime. `ExtensionContext` satisfies this structurally.
 */
export interface GuardContext {
	readonly hasUI: boolean;
	readonly ui: { notify(message: string, type?: "info" | "warning" | "error"): void };
	shutdown(): void;
}

/**
 * Apply a verdict: report it everywhere it can be seen, then ask the session to
 * stop. Returns the verdict so a caller can record it.
 */
export function applyVerdict(verdict: GuardVerdict, ctx: GuardContext): GuardVerdict {
	if (verdict.ok) return verdict;

	// console.error always: in json and print modes the UI methods are no-ops,
	// and a silent refusal is indistinguishable from a working session.
	console.error(`[cg-version-guard] ${verdict.code}: ${verdict.message}`);
	if (ctx.hasUI) {
		ctx.ui.notify(`[cg-version-guard] ${verdict.message}`, "error");
	}
	ctx.shutdown();
	return verdict;
}

/** Stable command name and description. Conformance asserts on both. */
export const GUARD_COMMAND_NAME = "cg-version";
export const GUARD_COMMAND_DESCRIPTION =
	"Report the pinned pi version, the running pi version, and the guard verdict";

/** Render the guard's current state for an operator. */
export function describeVerdict(verdict: GuardVerdict | null, running: string): string {
	if (verdict === null) {
		return `[cg-version-guard] running pi ${running}; the pin check has not run yet.`;
	}
	if (verdict.ok) {
		return `[cg-version-guard] ok: running pi ${running} matches the pin.`;
	}
	return `[cg-version-guard] ${verdict.code}: ${verdict.message}`;
}

/**
 * The registration surface this guard uses, and nothing more.
 *
 * Narrowed for the same reason as {@link GuardContext}: it lets the conformance
 * suite hand the factory a real, fully typed double instead of casting a stub
 * to `ExtensionAPI`. A cast there would silence the compiler on exactly the
 * question the test exists to answer.
 *
 * {@link ExtensionApiSatisfiesRegistrar} below proves at compile time that Pi's
 * real `ExtensionAPI` is assignable to this, so narrowing cannot drift away
 * from the interface Pi actually passes.
 */
export interface GuardRegistrar {
	on(
		event: "session_start",
		handler: (event: SessionStartEvent, ctx: GuardContext) => Promise<void>,
	): void;
	on(
		event: "tool_call",
		handler: () => Promise<ToolCallEventResult | undefined>,
	): void;
	registerCommand(
		name: string,
		options: {
			description: string;
			handler: (args: string, ctx: GuardContext) => Promise<void>;
		},
	): void;
}

/** Compile-time proof that {@link GuardRegistrar} is a subset of the real API. */
type AssertAssignable<A extends B, B> = A extends B ? true : never;
export type ExtensionApiSatisfiesRegistrar = AssertAssignable<
	ExtensionAPI,
	GuardRegistrar
>;

/**
 * Build the guard around a specific pin record.
 *
 * The path is a parameter so the conformance suite can drive the refusal
 * branches -- drifted version, unreadable pin -- which the real pinned runtime
 * will never reach on its own. Reading the pin at `session_start` rather than
 * here keeps the factory free of I/O that could fail in `pi list`.
 */
export function createVersionGuard(
	pinsJsonPath: string = defaultPinsJsonPath(),
): (pi: GuardRegistrar) => void {
	return function cgVersionGuard(pi: GuardRegistrar): void {
		// No background resources here: no timers, no watchers, no sockets. The
		// factory may run in an invocation that never starts a session.
		let verdict: GuardVerdict | null = null;

		// Registering a command is not a background resource, so the factory is the
		// right place for it. It also gives the distribution something Pi otherwise
		// does not: an observable statement of the resolved runtime. Pi 0.84.4
		// prints no extension manifest at startup in print, json or rpc mode -- not
		// even under `--verbose` -- and exposes no API for enumerating loaded
		// extensions. `get_commands` over RPC does report the resolved command
		// inventory with its source path and scope, so registering here is what
		// lets the conformance suite read the loaded configuration back from the
		// runtime rather than assuming the settings file was honoured.
		pi.registerCommand(GUARD_COMMAND_NAME, {
			description: GUARD_COMMAND_DESCRIPTION,
			handler: async (_args, ctx) => {
				const line = describeVerdict(verdict, VERSION);
				console.log(line);
				if (ctx.hasUI) {
					ctx.ui.notify(line, verdict === null || verdict.ok ? "info" : "error");
				}
			},
		});

		pi.on("session_start", async (_event, ctx) => {
			try {
				const required = readPinnedVersion(pinsJsonPath);
				verdict = evaluateVersion(VERSION, required);
			} catch (error) {
				verdict = {
					ok: false,
					code: "pin-unreadable",
					message:
						`Command Governor cannot read its own pin record, so it cannot tell ` +
						`whether pi ${VERSION} is the pinned runtime. ` +
						`${error instanceof Error ? error.message : String(error)}`,
				};
			}
			applyVerdict(verdict, ctx);
		});

		// Registered unconditionally, because `ctx.shutdown()` cannot be relied on
		// to stop anything. `verdict === null` means session_start has not run or
		// did not complete, and that is a refusal too — the guard fails closed.
		pi.on("tool_call", async (): Promise<ToolCallEventResult | undefined> => {
			if (verdict === null) {
				return {
					block: true,
					reason:
						"[cg-version-guard] blocked: the pi version check has not completed, " +
						"so the runtime is unverified.",
				};
			}
			if (verdict.ok) return undefined;
			return { block: true, reason: `[cg-version-guard] blocked: ${verdict.message}` };
		});
	};
}

/** What Pi loads: the guard bound to this distribution's own pin record. */
export default createVersionGuard();
