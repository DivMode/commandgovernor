/*
 * DeepSeek Harness architecture-donor adaptation.
 *
 * DSH's append-only typed SessionEvent log and projection seam are useful
 * structural patterns, but Command Governor does not copy DSH's session format
 * or make model transcripts authoritative. This module defines the smaller
 * Governor event spine: exact lifecycle/policy facts are committed first and
 * read models are projections. Lossy model context remains a consumer.
 */

export interface GovernorEventMap {
	"child/message-admitted": {
		readonly taskId: string;
		readonly parentSessionId: string;
		readonly childSessionId: string;
		readonly messageId: string;
		readonly contentDigest: string;
	};
	/** Durable intent precedes transport I/O so a crash leaves visible uncertainty. */
	"child/message-dispatch-started": {
		readonly messageId: string;
		readonly attemptId: string;
		readonly provider: string;
	};
	/** Transport returned, but this is not yet proof that the target durably recorded the message. */
	"child/message-dispatched": {
		readonly messageId: string;
		readonly attemptId: string;
		readonly provider: string;
		readonly providerReceipt?: string;
	};
	/** The transport proved it failed before an external effect could begin; retry remains safe. */
	"child/message-dispatch-rejected": {
		readonly messageId: string;
		readonly attemptId: string;
		readonly provider: string;
		readonly code: string;
	};
	/** Effect timing cannot be proven. This attempt must not be blindly replayed. */
	"child/message-dispatch-uncertain": {
		readonly messageId: string;
		readonly attemptId: string;
		readonly provider: string;
		readonly code: string;
	};
	/** Target/provider durable evidence closed the queued-minus-confirmed mailbox item. */
	"child/message-delivery-confirmed": {
		readonly messageId: string;
		readonly provider: string;
		readonly confirmation: string;
	};
	"child/activation-observed": {
		readonly childSessionId: string;
		readonly activationId: string;
		readonly generation: string;
	};
	"workflow/run-admitted": {
		readonly workflowId: string;
		readonly taskId: string;
		readonly definitionDigest: string;
	};
	"workflow/run-settled": {
		readonly workflowId: string;
		readonly outcome: "completed" | "failed" | "cancelled" | "uncertain";
	};
	"sandbox/policy-resolved": {
		readonly executionId: string;
		readonly profile: string;
		readonly filesystem: "full" | "partial";
		readonly network: "isolated" | "restricted" | "host";
		readonly process: "isolated" | "restricted" | "host";
		readonly credentials: "none" | "brokered" | "ambient";
	};
}

export type GovernorEventType = keyof GovernorEventMap;

export type GovernorEventDraft<K extends GovernorEventType = GovernorEventType> = {
	[P in K]: {
		readonly type: P;
		readonly data: GovernorEventMap[P];
		/**
		 * Purely informational records may opt into forward-compatible skipping.
		 * Required records default to fail-closed reconstruction.
		 */
		readonly ignorable?: true;
	};
}[K];

export type GovernorEvent<K extends GovernorEventType = GovernorEventType> = {
	[P in K]: GovernorEventDraft<P> & {
		readonly seq: number;
		readonly committedAt: string;
	};
}[K];

export interface DurableEventRef {
	readonly seq: number;
	readonly committedAt: string;
}

/**
 * Authority boundary: append() resolves only after the event is durably
 * committed according to the implementation's durability contract.
 *
 * The discriminated union already preserves the type/data correlation, so this
 * method intentionally is not generic. Keeping the sink non-generic makes
 * wrappers/recorders composable without weakening event validation.
 */
export interface DurableEventSink {
	append(event: GovernorEventDraft): Promise<DurableEventRef>;
}

export interface StoredGovernorEvent {
	readonly type: string;
	readonly seq: number;
	readonly committedAt: string;
	readonly data: unknown;
	readonly ignorable?: true;
}

export class UnknownRequiredGovernorEvent extends Error {
	readonly eventType: string;

	constructor(eventType: string) {
		super(`unknown required Governor event: ${eventType}`);
		this.name = "UnknownRequiredGovernorEvent";
		this.eventType = eventType;
	}
}

export type ProjectionHandler<State> = (state: State, event: StoredGovernorEvent) => State;

/**
 * Fold a stored event stream into a read model.
 *
 * Unknown required events stop reconstruction. Unknown explicitly-ignorable
 * events are skipped. This is intentionally conservative: an upgrade may
 * over-refuse, but it must not silently project incomplete authority state.
 */
export function projectStoredEvents<State>(
	events: readonly StoredGovernorEvent[],
	initial: State,
	handlers: Readonly<Record<string, ProjectionHandler<State>>>,
): State {
	let state = initial;
	let expectedSeq = 0;
	for (const event of events) {
		if (!Number.isSafeInteger(event.seq) || event.seq < 0 || event.seq !== expectedSeq) {
			throw new Error(`non-contiguous Governor event sequence: expected ${expectedSeq}, got ${String(event.seq)}`);
		}
		expectedSeq += 1;

		const handler = handlers[event.type];
		if (handler === undefined) {
			if (event.ignorable === true) continue;
			throw new UnknownRequiredGovernorEvent(event.type);
		}
		state = handler(state, event);
	}
	return state;
}
