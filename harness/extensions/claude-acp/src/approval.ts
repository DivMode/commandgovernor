/**
 * User-owned approval of high-risk actions — the requirement ADR 0008 §4 lists
 * and the README documents as unenforceable on Prime.
 *
 * It was unenforceable because Prime's only built-in tool is `ipython` and its
 * Python kernel runs shell commands below every extension hook, so no Prime
 * extension could sit between the model and the effect. Running Claude over ACP
 * moves the tools into Claude Code, which asks its client for permission before
 * every one of them — and this file is the client's answer. The enforcement
 * point is the ACP `session/request_permission` request, not a convention.
 *
 * Three policies, and the default is the conservative one:
 *
 *   ask    the user chooses, per request. Without a UI there is no user, so
 *          `ask` denies rather than silently allowing — a headless run must not
 *          be a way to get an unattended yes.
 *   allow  the first allow-shaped option is selected automatically. An explicit
 *          opt-in, written down in the project's own settings file.
 *   deny   every request is rejected. Useful for a read-only lane.
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import type { AcpPermissionOption, AcpPermissionOutcome, AcpPermissionRequest } from "./acp-client.ts";

export type ApprovalPolicy = "ask" | "allow" | "deny";

/** The project-scoped settings file, next to Prime's own project config. */
export const CONFIG_RELATIVE_PATH = join(".prime", "agent", "claude-acp.json");

export interface ClaudeAcpConfig {
	readonly approval: ApprovalPolicy;
	/** Override the adapter command; otherwise the pinned one is used. */
	readonly adapterCommand?: string;
}

function isPolicy(value: unknown): value is ApprovalPolicy {
	return value === "ask" || value === "allow" || value === "deny";
}

/**
 * Read the project's configuration.
 *
 * A missing or unreadable file is the default configuration, never an error:
 * this runs at extension load, and a syntax error in an optional settings file
 * must not stop Prime from starting. An INVALID `approval` value is different —
 * it is a spending- and safety-relevant setting that someone meant to set, so
 * it falls back to `ask` and says so.
 */
export function readConfig(cwd: string, warn: (message: string) => void = () => {}): ClaudeAcpConfig {
	const path = join(cwd, CONFIG_RELATIVE_PATH);
	if (!existsSync(path)) return { approval: "ask" };
	try {
		const parsed = JSON.parse(readFileSync(path, "utf8")) as Partial<ClaudeAcpConfig>;
		if (parsed.approval !== undefined && !isPolicy(parsed.approval)) {
			warn(`claude-acp: ignoring approval "${String(parsed.approval)}" in ${CONFIG_RELATIVE_PATH}; expected "ask", "allow" or "deny". Using "ask".`);
		}
		return {
			approval: isPolicy(parsed.approval) ? parsed.approval : "ask",
			...(typeof parsed.adapterCommand === "string" && parsed.adapterCommand.length > 0 ? { adapterCommand: parsed.adapterCommand } : {}),
		};
	} catch (error) {
		warn(`claude-acp: could not read ${CONFIG_RELATIVE_PATH} (${String((error as Error).message)}); using the default approval policy "ask".`);
		return { approval: "ask" };
	}
}

/** The first option whose kind starts with `prefix`, then any option at all. */
function optionOfKind(options: readonly AcpPermissionOption[], prefix: string): AcpPermissionOption | undefined {
	return options.find((option) => typeof option.kind === "string" && option.kind.startsWith(prefix));
}

/**
 * A one-line description of what is being asked, for the approval prompt.
 *
 * The tool call's own title if it has one, since that is what the agent chose
 * to show a human; the tool kind otherwise.
 */
export function describeRequest(request: AcpPermissionRequest): string {
	const title = request.toolCall?.title;
	if (typeof title === "string" && title.trim().length > 0) return title.trim();
	const kind = request.toolCall?.kind;
	return typeof kind === "string" && kind.length > 0 ? `${kind} (no title given)` : "an unnamed action";
}

/**
 * Decide a permission request.
 *
 * `ask` is a function rather than a UI object so the decision logic is testable
 * without a terminal: it is handed the option names and returns the chosen
 * index, or undefined for "the user did not choose". Undefined is a rejection,
 * because a dismissed prompt is not consent.
 */
export async function resolveOutcome(
	request: AcpPermissionRequest,
	policy: ApprovalPolicy,
	ask: ((prompt: string, optionNames: string[]) => Promise<number | undefined>) | undefined,
): Promise<AcpPermissionOutcome> {
	const options = request.options ?? [];
	if (options.length === 0) return { outcome: "cancelled" };

	const reject = optionOfKind(options, "reject");
	const rejection: AcpPermissionOutcome = reject ? { outcome: "selected", optionId: reject.optionId } : { outcome: "cancelled" };

	if (policy === "deny") return rejection;
	if (policy === "allow") {
		const allow = optionOfKind(options, "allow");
		return allow ? { outcome: "selected", optionId: allow.optionId } : rejection;
	}
	// policy === "ask": no interactive surface means no user, and no user means no.
	if (!ask) return rejection;
	const names = options.map((option, index) => option.name ?? option.kind ?? `option ${index + 1}`);
	const chosen = await ask(describeRequest(request), names);
	if (chosen === undefined || chosen < 0 || chosen >= options.length) return rejection;
	return { outcome: "selected", optionId: options[chosen].optionId };
}
