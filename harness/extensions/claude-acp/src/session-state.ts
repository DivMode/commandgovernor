/**
 * What Prime remembers about a Claude session, and the rule that decides what
 * a turn sends.
 *
 * The ownership boundary this whole package exists for is here in two
 * functions:
 *
 *   `newPromptSince` — Prime sends the NEW prompt and nothing else. Claude Code
 *   holds the conversation, so replaying earlier turns would not be "context",
 *   it would be a second copy of a transcript the agent already has, and it
 *   would move the cache-breaking prefix on every turn. The watermark is the
 *   newest user-message timestamp already delivered; a turn sends exactly the
 *   user messages after it. A Prime compaction rewrites Prime's own history and
 *   leaves those timestamps alone, so it cannot make this function resend
 *   anything — which is what "Prime never rebuilds Claude's history" means in
 *   code.
 *
 *   `readRecord` / `writeRecord` — the Claude session id, stored beside Prime's
 *   own session file and keyed by Prime's session id. A restarted Prime loads
 *   the same Claude session natively (`session/load`) instead of reconstructing
 *   one, which is what keeps the resume cursor and the cache lineage Claude's.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

/** Minimal view of a Prime message; only user messages matter here. */
export interface TimestampedMessage {
	readonly role: string;
	readonly content?: unknown;
	readonly timestamp?: number;
}

export interface NextPrompt {
	/** The text to send this turn. Empty means there is nothing new to send. */
	readonly text: string;
	/** The watermark to store once the turn has been sent. */
	readonly watermark: number;
	/** How many user messages went into `text`. */
	readonly count: number;
}

function messageText(content: unknown): string {
	if (typeof content === "string") return content;
	if (!Array.isArray(content)) return "";
	return content
		.filter((part): part is { type: string; text: string } => typeof part === "object" && part !== null && (part as { type?: unknown }).type === "text" && typeof (part as { text?: unknown }).text === "string")
		.map((part) => part.text)
		.join("\n");
}

/**
 * The user text this turn must send, given everything already delivered.
 *
 * Ordinarily exactly one message. More than one happens when Prime queues
 * several prompts while a turn is running; none happens when Prime re-enters
 * the provider without new user input, and the caller treats that as an error
 * rather than sending a blank prompt.
 */
export function newPromptSince(messages: readonly TimestampedMessage[], watermark: number): NextPrompt {
	const fresh = messages.filter((message) => message.role === "user" && typeof message.timestamp === "number" && message.timestamp > watermark);
	const texts = fresh.map((message) => messageText(message.content)).filter((text) => text.length > 0);
	const highest = fresh.reduce((max, message) => Math.max(max, message.timestamp ?? 0), watermark);
	return { text: texts.join("\n\n"), watermark: highest, count: fresh.length };
}

/**
 * The starting watermark for a NEWLY created Claude session: everything in
 * Prime's history except the turn being sent right now.
 *
 * This is the rule that makes "Prime never rebuilds Claude's history" hold in
 * the one case where it is tempting to break it. A fresh Claude session is
 * minted when Prime branches its tree, and would also be minted the first time
 * this provider runs inside an older Prime conversation. In both cases Prime's
 * message array is full of earlier user turns that the new Claude session has
 * never seen — and replaying them would be exactly the rebuild the previous
 * provider did. So the new session starts caught up: only the prompt the user
 * just typed is sent, and the earlier turns stay where they are, in the Claude
 * session that was left untouched.
 *
 * With zero or one user message it returns 0, so a genuinely new conversation
 * sends its first prompt normally.
 */
export function watermarkBeforeLatest(messages: readonly TimestampedMessage[]): number {
	const stamps = messages
		.filter((message) => message.role === "user" && typeof message.timestamp === "number")
		.map((message) => message.timestamp as number)
		.sort((a, b) => a - b);
	return stamps.length < 2 ? 0 : stamps[stamps.length - 2];
}

export interface SessionRecord {
	/** The Claude Code session id, as `session/new` returned it. */
	readonly claudeSessionId: string;
	/** Newest user-message timestamp already delivered to that session. */
	readonly watermark: number;
	/** The model the session was created with, so a change can be detected. */
	readonly model: string;
	readonly updatedAt: string;
}

/**
 * Where the record lives: beside Prime's session JSONL, named for it.
 *
 * A sidecar rather than an entry inside Prime's transcript because the
 * extension API hands extensions a READ-ONLY session manager
 * (`ReadonlySessionManager` has no append), so there is no supported way to
 * write a custom entry from here. Keyed by Prime's session id, so a fork —
 * which gets its own id and its own file — cannot inherit another branch's
 * Claude session by accident.
 */
export function recordPath(sessionDir: string, primeSessionId: string): string {
	return join(sessionDir, `${primeSessionId}.claude-acp.json`);
}

export function readRecord(path: string): SessionRecord | undefined {
	if (!existsSync(path)) return undefined;
	try {
		const parsed = JSON.parse(readFileSync(path, "utf8")) as Partial<SessionRecord>;
		if (typeof parsed.claudeSessionId !== "string" || parsed.claudeSessionId.length === 0) return undefined;
		return {
			claudeSessionId: parsed.claudeSessionId,
			watermark: typeof parsed.watermark === "number" ? parsed.watermark : 0,
			model: typeof parsed.model === "string" ? parsed.model : "",
			updatedAt: typeof parsed.updatedAt === "string" ? parsed.updatedAt : "",
		};
	} catch {
		// A corrupt sidecar must not stop the session; it costs one fresh Claude
		// session, which is recoverable, where a throw here would not be.
		return undefined;
	}
}

export function writeRecord(path: string, record: Omit<SessionRecord, "updatedAt">): void {
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, `${JSON.stringify({ ...record, updatedAt: new Date().toISOString() }, null, 2)}\n`, "utf8");
}
