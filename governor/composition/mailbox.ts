import type { GovernorEvent } from "./events.ts";

export type ChildMailboxState = "queued" | "dispatching" | "dispatched" | "uncertain" | "confirmed";

export interface ChildMailboxAttempt {
	readonly attemptId: string;
	readonly provider: string;
	readonly state: "dispatching" | "dispatched" | "rejected" | "uncertain";
	readonly providerReceipt?: string;
	readonly code?: string;
}

export interface ChildMailboxItem {
	readonly messageId: string;
	readonly taskId: string;
	readonly parentSessionId: string;
	readonly childSessionId: string;
	readonly contentDigest: string;
	readonly state: ChildMailboxState;
	readonly lastAttempt?: ChildMailboxAttempt;
	readonly confirmation?: { readonly provider: string; readonly value: string };
}

export class ChildMailboxProjectionError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "ChildMailboxProjectionError";
	}
}

function cloneItem(item: ChildMailboxItem, patch: Partial<ChildMailboxItem>): ChildMailboxItem {
	return { ...item, ...patch };
}

/**
 * Fold the Governor child-message event vocabulary into queued-minus-confirmed
 * mailbox state. Unrelated known Governor events are ignored by design.
 *
 * A terminal `dispatching` state means the process/log ended after durable
 * dispatch intent but before an outcome was durably recorded. Recovery must
 * treat that as uncertainty and must not automatically submit another send.
 */
export function projectChildMailbox(events: readonly GovernorEvent[]): ReadonlyMap<string, ChildMailboxItem> {
	const items = new Map<string, ChildMailboxItem>();

	for (const event of events) {
		switch (event.type) {
			case "child/message-admitted": {
				const d = event.data;
				if (items.has(d.messageId)) throw new ChildMailboxProjectionError(`duplicate child message admission: ${d.messageId}`);
				items.set(d.messageId, {
					messageId: d.messageId,
					taskId: d.taskId,
					parentSessionId: d.parentSessionId,
					childSessionId: d.childSessionId,
					contentDigest: d.contentDigest,
					state: "queued",
				});
				break;
			}
			case "child/message-dispatch-started": {
				const d = event.data;
				const item = items.get(d.messageId);
				if (item === undefined) throw new ChildMailboxProjectionError(`dispatch before child message admission: ${d.messageId}`);
				if (!childMessageIsAutoRetryable(item)) {
					throw new ChildMailboxProjectionError(`child message is not safely retryable before dispatch: ${d.messageId}/${item.state}`);
				}
				items.set(d.messageId, cloneItem(item, {
					state: "dispatching",
					lastAttempt: { attemptId: d.attemptId, provider: d.provider, state: "dispatching" },
				}));
				break;
			}
			case "child/message-dispatched": {
				const d = event.data;
				const item = items.get(d.messageId);
				if (item === undefined || item.lastAttempt?.attemptId !== d.attemptId || item.lastAttempt.provider !== d.provider || item.lastAttempt.state !== "dispatching") {
					throw new ChildMailboxProjectionError(`child dispatch result does not match the open attempt: ${d.messageId}/${d.attemptId}`);
				}
				items.set(d.messageId, cloneItem(item, {
					state: "dispatched",
					lastAttempt: {
						attemptId: d.attemptId,
						provider: d.provider,
						state: "dispatched",
						providerReceipt: d.providerReceipt,
					},
				}));
				break;
			}
			case "child/message-dispatch-rejected": {
				const d = event.data;
				const item = items.get(d.messageId);
				if (item === undefined || item.lastAttempt?.attemptId !== d.attemptId || item.lastAttempt.provider !== d.provider || item.lastAttempt.state !== "dispatching") {
					throw new ChildMailboxProjectionError(`child dispatch rejection does not match the open attempt: ${d.messageId}/${d.attemptId}`);
				}
				items.set(d.messageId, cloneItem(item, {
					state: "queued",
					lastAttempt: { attemptId: d.attemptId, provider: d.provider, state: "rejected", code: d.code },
				}));
				break;
			}
			case "child/message-dispatch-uncertain": {
				const d = event.data;
				const item = items.get(d.messageId);
				if (item === undefined || item.lastAttempt?.attemptId !== d.attemptId || item.lastAttempt.provider !== d.provider || item.lastAttempt.state !== "dispatching") {
					throw new ChildMailboxProjectionError(`child dispatch uncertainty does not match the open attempt: ${d.messageId}/${d.attemptId}`);
				}
				items.set(d.messageId, cloneItem(item, {
					state: "uncertain",
					lastAttempt: { attemptId: d.attemptId, provider: d.provider, state: "uncertain", code: d.code },
				}));
				break;
			}
			case "child/message-delivery-confirmed": {
				const d = event.data;
				const item = items.get(d.messageId);
				if (item === undefined) throw new ChildMailboxProjectionError(`child delivery confirmation before admission: ${d.messageId}`);
				if (item.state === "queued") throw new ChildMailboxProjectionError(`child delivery confirmation without any dispatch evidence: ${d.messageId}`);
				if (item.state === "confirmed") {
					if (item.confirmation?.provider === d.provider && item.confirmation.value === d.confirmation) break;
					throw new ChildMailboxProjectionError(`conflicting duplicate child delivery confirmation: ${d.messageId}`);
				}
				items.set(d.messageId, cloneItem(item, {
					state: "confirmed",
					confirmation: { provider: d.provider, value: d.confirmation },
				}));
				break;
			}
			default:
				break;
		}
	}

	return items;
}

/** Items that still represent Governor obligations. */
export function pendingChildMessages(mailbox: ReadonlyMap<string, ChildMailboxItem>): readonly ChildMailboxItem[] {
	return [...mailbox.values()].filter((item) => item.state !== "confirmed");
}

/**
 * Recovery fence: an open dispatch attempt has unknown effect timing after a
 * process/replay boundary. The caller must append an uncertainty/reconciliation
 * fact before any retry.
 */
export function interruptedChildDispatches(mailbox: ReadonlyMap<string, ChildMailboxItem>): readonly ChildMailboxItem[] {
	return [...mailbox.values()].filter((item) => item.state === "dispatching");
}

export function childMessageIsAutoRetryable(item: ChildMailboxItem): boolean {
	return item.state === "queued" && (item.lastAttempt === undefined || item.lastAttempt.state === "rejected");
}
