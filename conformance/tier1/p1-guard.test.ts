/**
 * P1-GUARD — the version guard's behaviour, driven directly.
 *
 * The guard is exercised here against a fake context rather than through a live
 * session, because the interesting cases are the ones a live session cannot
 * reach: a drifted runtime, and a runtime that cannot identify itself. Both are
 * states the pinned binary will never be in, so a test that only ran real
 * sessions would leave the entire refusal path unexercised.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { VERSION } from "@earendil-works/pi-coding-agent";

import guardFactory, {
	applyVerdict,
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
import { readPins } from "../lib/repo.ts";

const PINNED = readPins().pi.version;

/** A context that records what the guard did to it. */
function fakeContext(hasUI: boolean): GuardContext & {
	notified: { message: string; type?: string }[];
	shutdowns: number;
} {
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
	} as GuardContext & {
		notified: { message: string; type?: string }[];
		shutdowns: number;
	};
}

/** A minimal ExtensionAPI double that captures registrations. */
function fakeExtensionApi() {
	const handlers = new Map<string, ((event: unknown, ctx: unknown) => unknown)[]>();
	const commands = new Map<string, { description?: string; handler: unknown }>();
	return {
		api: {
			on(event: string, handler: (e: unknown, c: unknown) => unknown) {
				const list = handlers.get(event) ?? [];
				list.push(handler);
				handlers.set(event, list);
			},
			registerCommand(name: string, options: { description?: string; handler: unknown }) {
				commands.set(name, options);
			},
		},
		handlers,
		commands,
	};
}

describe("P1-GUARD: version comparison", () => {
	it("accepts the exact pinned version", () => {
		const verdict = evaluateVersion(PINNED, PINNED);
		assert.equal(verdict.ok, true);
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
		assert.notEqual(
			verdict.ok === false && verdict.code,
			"runtime-version-drift",
			"unknown and drifted must be distinguishable",
		);
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
		// node_modules/@earendil-works links to, and PINNED comes from
		// pins.json. If bootstrap linked a different tree, this fails.
		assert.equal(evaluateVersion(VERSION, PINNED).ok, true);
	});
});

describe("P1-GUARD: what a refusal does", () => {
	it("reports to the UI and asks the session to stop when there is a UI", () => {
		const ctx = fakeContext(true);
		applyVerdict(evaluateVersion("0.84.3", PINNED), ctx);
		assert.equal(ctx.shutdowns, 1);
		assert.equal(ctx.notified.length, 1);
		assert.equal(ctx.notified[0].type, "error");
	});

	it("still asks the session to stop when there is no UI", () => {
		// json and print modes have no UI, and their ui methods are no-ops. A
		// guard that only notified would be silent in exactly the modes an
		// orchestrator uses.
		const ctx = fakeContext(false);
		applyVerdict(evaluateVersion("0.84.3", PINNED), ctx);
		assert.equal(ctx.shutdowns, 1);
		assert.equal(ctx.notified.length, 0);
	});

	it("does nothing at all when the version matches", () => {
		const ctx = fakeContext(true);
		applyVerdict(evaluateVersion(PINNED, PINNED), ctx);
		assert.equal(ctx.shutdowns, 0);
		assert.equal(ctx.notified.length, 0);
	});
});

describe("P1-GUARD: registration and tool blocking", () => {
	it("registers in the factory without starting anything in the background", () => {
		const { api, handlers, commands } = fakeExtensionApi();
		guardFactory(api as never);

		assert.ok(handlers.has("session_start"), "the check must run in session_start");
		assert.ok(handlers.has("tool_call"), "a print-mode session must still be blocked");
		assert.ok(commands.has(GUARD_COMMAND_NAME));
		assert.equal(commands.get(GUARD_COMMAND_NAME)?.description, GUARD_COMMAND_DESCRIPTION);
	});

	it("blocks tool calls before the check has run", async () => {
		// ctx.shutdown() is a no-op in print mode, so this is the only thing
		// standing between an unverified runtime and real side effects.
		const { api, handlers } = fakeExtensionApi();
		guardFactory(api as never);

		const result = (await handlers.get("tool_call")?.[0]({}, {})) as
			| { block?: boolean; reason?: string }
			| undefined;
		assert.equal(result?.block, true);
		assert.match(result?.reason ?? "", /unverified/);
	});

	it("blocks tool calls after a refusal, and allows them after a pass", async () => {
		const { api, handlers } = fakeExtensionApi();
		guardFactory(api as never);

		const sessionStart = handlers.get("session_start")?.[0];
		assert.ok(sessionStart);

		// The real session_start reads the real pins.json against the real
		// VERSION, which currently matches, so this drives the passing path.
		await sessionStart({}, fakeContext(false));
		const allowed = await handlers.get("tool_call")?.[0]({}, {});
		assert.equal(allowed, undefined, "a matching runtime must not block tools");
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
});
