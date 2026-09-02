/*
 * A durable Session is not a live process residency epoch.
 *
 * DeepSeek Harness names this split Session vs Activation. Command Governor
 * adopts the distinction but strengthens it with substrate generation/cursor
 * fencing and durable admission requirements outside process-local state.
 */

type Brand<T, Name extends string> = T & { readonly __brand: Name };

export type GovernorSessionId = Brand<string, "GovernorSessionId">;
export type GovernorActivationId = Brand<string, "GovernorActivationId">;
export type GovernorMessageId = Brand<string, "GovernorMessageId">;

function admitOpaqueId<Name extends string>(value: string, name: Name): Brand<string, Name> {
	if (value.length === 0 || value.trim() !== value || /[\u0000-\u001f\u007f]/u.test(value)) {
		throw new TypeError(`${name} must be a non-empty opaque id without control characters or surrounding whitespace`);
	}
	return value as Brand<string, Name>;
}

export function governorSessionId(value: string): GovernorSessionId {
	return admitOpaqueId(value, "GovernorSessionId");
}

export function governorActivationId(value: string): GovernorActivationId {
	return admitOpaqueId(value, "GovernorActivationId");
}

export function governorMessageId(value: string): GovernorMessageId {
	return admitOpaqueId(value, "GovernorMessageId");
}

export interface DurableSessionRef {
	/** Governor-stable identity used for obligations and lineage. */
	readonly sessionId: GovernorSessionId;
	/** Substrate-stable persisted session identity/path token. */
	readonly substrateSessionId: string;
	/** Exact immutable launch/loadout digest when known. */
	readonly loadoutDigest?: string;
}

export interface LiveActivationRef {
	/** Process-lifetime identity; never a durable session identity. */
	readonly activationId: GovernorActivationId;
	readonly sessionId: GovernorSessionId;
	/** Substrate worker/supervisor generation fence. */
	readonly generation: string;
	/** Replay/event cursor captured when the activation was observed. */
	readonly cursor?: string;
	readonly processLocal: true;
}

export function activationMatchesSession(activation: LiveActivationRef, session: DurableSessionRef): boolean {
	return activation.sessionId === session.sessionId;
}

export function assertActivationMatchesSession(activation: LiveActivationRef, session: DurableSessionRef): void {
	if (!activationMatchesSession(activation, session)) {
		throw new Error(`activation ${activation.activationId} does not belong to durable session ${session.sessionId}`);
	}
}
