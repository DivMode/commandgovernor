/**
 * P1-GUARD — the version guard's behaviour, driven directly.
 *
 * The guard is exercised here against real doubles rather than through a live
 * session, because the interesting cases are the ones a live session cannot
 * reach: a drifted runtime, and a runtime that cannot identify itself. Both are
 * states the pinned binary will never be in, so a test that only ran real
 * sessions would leave the entire refusal path unexercised.
 *
 * The doubles are typed, not cast. `GuardRegistrar` and `GuardContext` are the
 * narrowed surfaces the guard declares, and the extension module carries a
 * compile-time proof that Pi's real `ExtensionAPI` is assignable to the former
 * -- so the doubles cannot drift into impersonating an interface Pi does not
 * actually pass. A cast here would have silenced the compiler on exactly the
 * question these tests exist to answer.
 */

import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import {
	VERSION,
	type SessionStartEvent,
	type ToolCallEventResult,
} from "@earendil-works/pi-coding-agent";

import guard, {
	applyVerdict,
	createVersionGuard,
	defaultPinsJsonPath,
	describeVerdict,
	evaluateVersion,
	GUARD_COMMAND_DESCRIPTION,
	GUARD_COMMAND_NAME,
	readPinnedVersion,
	UNKNOWN_VERSION,
	type GuardContext,
	type GuardVerdict,
} from "../../harness/extensions/cg-version-guard.ts";
import { PINS_JSON, readPins } from "../lib/repo.ts";
import { readFileSync } from "node:fs";

const PINNED = readPins().pi.version;

// ---------------------------------------------------------------------------
// Doubles
// ---------------------------------------------------------------------------

interface RecordingContext extends GuardContext {
	readonly notified: readonly { message: string; type?: string }[];
	readonly shutdowns: number;
}

function recordingContext(hasUI: boolean): RecordingContext {
	const notified: { message: string; type?: string }[] = [];
	let shutdowns = 0;
	return {
		hasUI,
		ui: {
			notify(message: string, type?: "info" | "warning" | "error") {
				notified.push({ message, type });
			},
		},
		shutdown() {
			shutdowns += 1;
		},
		get notified() {
			return notified;
		},
		get shutdowns() {
			return shutdowns;
		},
	};
}

type SessionStartHandler = (
	event: SessionStartEvent,
	ctx: GuardContext,
) => Promise<void>;
type ToolCallHandler = () => Promise<ToolCallEventResult | undefined>;
interface CommandOptions {
	description: string;
	handler: (args: string, ctx: GuardContext) => Promise<void>;
}

/**
 * A `GuardRegistrar` that records what the factory registered.
 *
 * Written with real overload signatures rather than a permissive catch-all, so
 * registering an event the guard is not supposed to touch would not typecheck.
 */
class RecordingRegistrar {
	sessionStart: SessionStartHandler | undefined;
	toolCall: ToolCallHandler | undefined;
	readonly commands = new Map<string, CommandOptions>();

	on(event: "session_start", handler: SessionStartHandler): void;
	on(event: "tool_call", handler: ToolCallHandler): void;
	on(
		event: "session_start" | "tool_call",
		handler: SessionStartHandler | ToolCallHandler,
	): void {
		if (event === "session_start") {
			this.sessionStart = handler as SessionStartHandler;
		} else {
			this.toolCall = handler as ToolCallHandler;
		}
	}

	registerCommand(name: string, options: CommandOptions): void {
		this.commands.set(name, options);
	}
}

/** A `session_start` event carrying only what the guard reads from it: nothing. */
function sessionStartEvent(): SessionStartEvent {
	return { type: "session_start", reason: "startup" } satisfies SessionStartEvent;
}

/** Write a pin record with a fabricated version into a throwaway directory. */
function fabricatedPins(version: string): { path: string; dispose: () => void } {
	const dir = mkdtempSync(join(tmpdir(), "cg-guard-pins-"));
	const doc = JSON.parse(readFileSync(PINS_JSON, "utf8")) as {
		pi: Record<string, unknown>;
	};
	doc.pi = { ...doc.pi, version };
	const path = join(dir, "pins.json");
	writeFileSync(path, JSON.stringify(doc, null, 2));
	return { path, dispose: () => rmSync(dir, { recursive: true, force: true }) };
}

// ---------------------------------------------------------------------------

describe("P1-GUARD: version comparison", () => {
	it("accepts the exact pinned version", () => {
		assert.equal(evaluateVersion(PINNED, PINNED).ok, true);
	});

	it("refuses a drifted version with a stable code", () => {
		const verdict = evaluateVersion("0.84.3", PINNED);
		assert.equal(verdict.ok, false);
		assert.equal(verdict.ok === false && verdict.code, "runtime-version-drift");
		assert.ok(verdict.ok === false && verdict.message.includes("0.84.3"));
		assert.ok(verdict.ok === false && verdict.message.includes(PINNED));
	});

	it("refuses a newer version too — a pin is not a floor", () => {
		const verdict = evaluateVersion("0.85.0", PINNED);
		assert.equal(verdict.ok, false);
		assert.equal(verdict.ok === false && verdict.code, "runtime-version-drift");
	});

	it('treats "0.0.0" as unknown, not as ancient', () => {
		// Pi's VERSION falls back to "0.0.0" when the CLI cannot read its own
		// package.json. Classifying that as an old version would send an
		// operator looking for a downgrade that never happened.
		const verdict = evaluateVersion(UNKNOWN_VERSION, PINNED);
		assert.equal(verdict.ok, false);
		assert.equal(verdict.ok === false && verdict.code, "runtime-version-unknown");
	});

	it("reads the required version from pins.json and nowhere else", () => {
		assert.equal(readPinnedVersion(defaultPinsJsonPath()), PINNED);
	});

	it("fails rather than inventing a version when the pin cannot be read", () => {
		assert.throws(
			() => readPinnedVersion("/nonexistent/pins.json"),
			/cannot read the pin record/,
		);
	});

	it("agrees with the runtime it is actually running against", () => {
		// Not a tautology: VERSION comes from the pinned pi package that
		// node_modules links to, and PINNED comes from pins.json. If bootstrap
		// linked a different tree, this fails.
		assert.equal(evaluateVersion(VERSION, PINNED).ok, true);
	});
});

describe("P1-GUARD: what a refusal does", () => {
	it("reports to the UI and asks the session to stop when there is a UI", () => {
		const ctx = recordingContext(true);
		applyVerdict(evaluateVersion("0.84.3", PINNED), ctx);
		assert.equal(ctx.shutdowns, 1);
		assert.equal(ctx.notified.length, 1);
		assert.equal(ctx.notified[0].type, "error");
	});

	it("still asks the session to stop when there is no UI", () => {
		// json and print modes have no UI and their ui methods are no-ops. A
		// guard that only notified would be silent in exactly the modes an
		// orchestrator uses.
		const ctx = recordingContext(false);
		applyVerdict(evaluateVersion("0.84.3", PINNED), ctx);
		assert.equal(ctx.shutdowns, 1);
		assert.equal(ctx.notified.length, 0);
	});

	it("does nothing at all when the version matches", () => {
		const ctx = recordingContext(true);
		applyVerdict(evaluateVersion(PINNED, PINNED), ctx);
		assert.equal(ctx.shutdowns, 0);
		assert.equal(ctx.notified.length, 0);
	});
});

describe("P1-GUARD: registration", () => {
	it("registers in the factory without starting anything in the background", () => {
		const registrar = new RecordingRegistrar();
		guard(registrar);

		assert.ok(registrar.sessionStart, "the check must run in session_start");
		assert.ok(registrar.toolCall, "a print-mode session must still be blocked");
		assert.equal(
			registrar.commands.get(GUARD_COMMAND_NAME)?.description,
			GUARD_COMMAND_DESCRIPTION,
		);
	});
});

describe("P1-GUARD: tool blocking", () => {
	it("blocks before the check has run", async () => {
		// ctx.shutdown() is a no-op in print mode, so this is the only thing
		// standing between an unverified runtime and real side effects.
		const registrar = new RecordingRegistrar();
		guard(registrar);

		const result = await registrar.toolCall?.();
		assert.equal(result?.block, true);
		assert.match(result?.reason ?? "", /unverified/);
	});

	it("allows tools once the pinned runtime is confirmed", async () => {
		const registrar = new RecordingRegistrar();
		guard(registrar);

		await registrar.sessionStart?.(sessionStartEvent(), recordingContext(false));
		assert.equal(
			await registrar.toolCall?.(),
			undefined,
			"a matching runtime must not block tools",
		);
	});

	it("blocks every tool call after a drift refusal", async () => {
		// The branch that actually protects a print-mode session: shutdown is
		// ignored there, so the tool gate is the whole defence. Driving it needs
		// a pin record that disagrees with the running runtime, which is what
		// the injectable pins path is for.
		const { path, dispose } = fabricatedPins("0.99.99");
		try {
			const registrar = new RecordingRegistrar();
			createVersionGuard(path)(registrar);

			const ctx = recordingContext(true);
			await registrar.sessionStart?.(sessionStartEvent(), ctx);

			assert.equal(ctx.shutdowns, 1, "a drifted runtime must be asked to stop");
			assert.equal(ctx.notified.length, 1);

			const result = await registrar.toolCall?.();
			assert.equal(result?.block, true, "a drifted runtime must block tools");
			assert.match(result?.reason ?? "", /runtime-version-drift|requires pi 0\.99\.99/);
			assert.match(result?.reason ?? "", /blocked/);
		} finally {
			dispose();
		}
	});

	it("blocks every tool call when the pin record cannot be read", async () => {
		// Fail closed: a guard that cannot find its pin does not know whether the
		// runtime is right, and "I could not check" must not be treated as "fine".
		const registrar = new RecordingRegistrar();
		createVersionGuard("/nonexistent/pins.json")(registrar);

		const ctx = recordingContext(false);
		await registrar.sessionStart?.(sessionStartEvent(), ctx);

		assert.equal(ctx.shutdowns, 1);
		const result = await registrar.toolCall?.();
		assert.equal(result?.block, true);
		assert.match(result?.reason ?? "", /cannot read its own pin record/);
	});

	it("carries the refusal reason into the block, not a generic message", async () => {
		// The reason reaches the model as the tool-call rejection. A generic
		// "blocked" would leave an operator with no idea which runtime was wrong.
		const { path, dispose } = fabricatedPins("0.77.7");
		try {
			const registrar = new RecordingRegistrar();
			createVersionGuard(path)(registrar);
			await registrar.sessionStart?.(sessionStartEvent(), recordingContext(false));

			const result = await registrar.toolCall?.();
			assert.ok(result?.reason?.includes("0.77.7"), result?.reason);
			assert.ok(result?.reason?.includes(VERSION), result?.reason);
		} finally {
			dispose();
		}
	});
});

describe("P1-GUARD: operator readout", () => {
	it("distinguishes not-yet-checked from ok from refused", () => {
		assert.match(describeVerdict(null, PINNED), /has not run yet/);
		assert.match(describeVerdict({ ok: true, version: PINNED }, PINNED), /ok:/);

		const refusal: GuardVerdict = {
			ok: false,
			code: "runtime-version-drift",
			message: "drifted",
		};
		assert.match(describeVerdict(refusal, "0.84.3"), /runtime-version-drift/);
	});

	it("reports the refusal through the command as well", async () => {
		const { path, dispose } = fabricatedPins("0.99.99");
		try {
			const registrar = new RecordingRegistrar();
			createVersionGuard(path)(registrar);
			await registrar.sessionStart?.(sessionStartEvent(), recordingContext(false));

			const ctx = recordingContext(true);
			await registrar.commands.get(GUARD_COMMAND_NAME)?.handler("", ctx);

			assert.equal(ctx.notified.length, 1);
			assert.equal(ctx.notified[0].type, "error");
			assert.match(ctx.notified[0].message, /runtime-version-drift/);
		} finally {
			dispose();
		}
	});
});
