import type { DurableEventRef, DurableEventSink } from "./events.ts";
import type { DurableSessionRef, GovernorMessageId, GovernorSessionId } from "./lifecycle.ts";

export type ChildStartCapability =
	| "agent-options"
	| "structured-output"
	| "depth-limit"
	| "tool-filter"
	| "persona"
	| "continuation";

export interface ChildProviderDescriptor {
	readonly name: string;
	readonly capabilities: readonly ChildStartCapability[];
}

export interface ChildRequirements {
	readonly capabilities?: readonly ChildStartCapability[];
}

export class UnsupportedChildCapability extends Error {
	readonly provider: string;
	readonly capability: ChildStartCapability;

	constructor(provider: string, capability: ChildStartCapability) {
		super(`child provider ${provider} does not support required capability ${capability}`);
		this.name = "UnsupportedChildCapability";
		this.provider = provider;
		this.capability = capability;
	}
}

export function assertChildProviderSupports(descriptor: ChildProviderDescriptor, requirements: ChildRequirements): void {
	const supported = new Set(descriptor.capabilities);
	for (const capability of requirements.capabilities ?? []) {
		if (!supported.has(capability)) throw new UnsupportedChildCapability(descriptor.name, capability);
	}
}

export interface ChildStartRequest {
	readonly parent: DurableSessionRef;
	readonly taskId: string;
	readonly promptDigest: string;
	readonly mode: "one-shot" | "continuable";
	readonly maxDepth?: number;
	readonly persona?: string;
	readonly allowedTools?: readonly string[];
	readonly outputSchemaDigest?: string;
}

export interface ChildStartResult {
	/** Durable child identity. Never use a process-local activation id here. */
	readonly childSessionId: GovernorSessionId;
	readonly provider: string;
	readonly providerSessionId?: string;
}

export interface ChildProvider {
	readonly descriptor: ChildProviderDescriptor;
	start(request: ChildStartRequest, signal: AbortSignal): Promise<ChildStartResult>;
}

export interface ChildMessageAdmission {
	readonly taskId: string;
	readonly parentSessionId: GovernorSessionId;
	readonly childSessionId: GovernorSessionId;
	readonly messageId: GovernorMessageId;
	/** Digest of the bounded payload/artifact reference, not necessarily raw text. */
	readonly contentDigest: string;
}

export interface AcceptedChildMessage extends ChildMessageAdmission {
	/** Proof that admission crossed the durable boundary before acceptance returned. */
	readonly durable: DurableEventRef;
}

/**
 * Commit-before-acceptance mailbox boundary.
 *
 * A provider/runtime may have its own process-local inbox, but Governor does
 * not acknowledge the message until its durable obligation/event exists.
 * Dispatch and target delivery are later state transitions.
 */
export async function admitChildMessage(
	sink: DurableEventSink,
	message: ChildMessageAdmission,
): Promise<AcceptedChildMessage> {
	const durable = await sink.append({
		type: "child/message-admitted",
		data: {
			taskId: message.taskId,
			parentSessionId: message.parentSessionId,
			childSessionId: message.childSessionId,
			messageId: message.messageId,
			contentDigest: message.contentDigest,
		},
	});
	return { ...message, durable };
}

export type ChildTransportFailureClass =
	| { readonly effect: "not-started"; readonly code: string }
	| { readonly effect: "unknown"; readonly code: string };

export interface ChildMessageTransport {
	readonly provider: string;
	send(message: AcceptedChildMessage, signal: AbortSignal): Promise<{ readonly providerReceipt?: string }>;
	/**
	 * Optional conservative classifier for errors thrown by `send()` only.
	 * Absence, a throw, or an invalid code is treated as unknown effect timing.
	 * Persistence failures are never passed here because a successful send may
	 * already have produced an external effect.
	 */
	classifyFailure?(error: unknown): ChildTransportFailureClass;
}

export type ChildDispatchOutcome =
	| {
		readonly state: "dispatched";
		readonly attemptId: string;
		readonly started: DurableEventRef;
		readonly recorded: DurableEventRef;
		readonly providerReceipt?: string;
	}
	| {
		readonly state: "rejected";
		readonly attemptId: string;
		readonly started: DurableEventRef;
		readonly recorded: DurableEventRef;
		readonly code: string;
	}
	| {
		readonly state: "uncertain";
		readonly attemptId: string;
		readonly started: DurableEventRef;
		readonly recorded: DurableEventRef;
		readonly code: string;
	};

function requireOpaqueToken(value: string, field: string): string {
	if (value.length === 0 || value.trim() !== value || /[\u0000-\u001f\u007f]/u.test(value)) {
		throw new TypeError(`${field} must be a non-empty opaque token without control characters or surrounding whitespace`);
	}
	return value;
}

function classifyTransportFailure(transport: ChildMessageTransport, error: unknown): ChildTransportFailureClass {
	try {
		const classified = transport.classifyFailure?.(error);
		if (classified !== undefined) {
			requireOpaqueToken(classified.code, "child transport failure code");
			return classified;
		}
	} catch {
		// A broken classifier must never turn an unknown external effect into a
		// retryable failure.
	}
	return { effect: "unknown", code: "transport_effect_unknown" };
}

/**
 * Journal-before-dispatch boundary.
 *
 * `child/message-dispatch-started` is durably committed before transport I/O.
 * If the process dies after that commit and before a terminal dispatch event,
 * recovery sees an interrupted attempt and MUST reconcile/quarantine it rather
 * than blindly replaying the send.
 */
export async function dispatchAcceptedChildMessage(
	sink: DurableEventSink,
	transport: ChildMessageTransport,
	message: AcceptedChildMessage,
	attemptId: string,
	signal: AbortSignal,
): Promise<ChildDispatchOutcome> {
	requireOpaqueToken(attemptId, "child dispatch attemptId");
	requireOpaqueToken(transport.provider, "child transport provider");
	const started = await sink.append({
		type: "child/message-dispatch-started",
		data: {
			messageId: message.messageId,
			attemptId,
			provider: transport.provider,
		},
	});

	let receipt: { readonly providerReceipt?: string };
	try {
		receipt = await transport.send(message, signal);
	} catch (error) {
		const failure = classifyTransportFailure(transport, error);
		if (failure.effect === "not-started") {
			const recorded = await sink.append({
				type: "child/message-dispatch-rejected",
				data: {
					messageId: message.messageId,
					attemptId,
					provider: transport.provider,
					code: failure.code,
				},
			});
			return { state: "rejected", attemptId, started, recorded, code: failure.code };
		}

		const recorded = await sink.append({
			type: "child/message-dispatch-uncertain",
			data: {
				messageId: message.messageId,
				attemptId,
				provider: transport.provider,
				code: failure.code,
			},
		});
		return { state: "uncertain", attemptId, started, recorded, code: failure.code };
	}

	// IMPORTANT: this persistence append is intentionally outside the transport
	// try/catch. If the send returned but recording that fact fails, the durable
	// log remains `dispatch-started` only. Recovery must therefore treat the
	// attempt as ambiguous. Passing this storage error to classifyFailure() could
	// incorrectly label an already-effectful send as a pre-effect rejection.
	const recorded = await sink.append({
		type: "child/message-dispatched",
		data: {
			messageId: message.messageId,
			attemptId,
			provider: transport.provider,
			providerReceipt: receipt.providerReceipt,
		},
	});
	return { state: "dispatched", attemptId, started, recorded, providerReceipt: receipt.providerReceipt };
}

/**
 * Close a queued-minus-confirmed mailbox item only with explicit durable-target
 * evidence or an equivalent reconciliation proof supplied by the provider
 * adapter. A mere successful transport return is not enough by definition.
 */
export async function confirmChildMessageDelivery(
	sink: DurableEventSink,
	message: AcceptedChildMessage,
	provider: string,
	confirmation: string,
): Promise<DurableEventRef> {
	requireOpaqueToken(provider, "child delivery provider");
	requireOpaqueToken(confirmation, "child delivery confirmation");
	return sink.append({
		type: "child/message-delivery-confirmed",
		data: {
			messageId: message.messageId,
			provider,
			confirmation,
		},
	});
}
