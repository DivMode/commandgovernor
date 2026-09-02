/*
 * DeepSeek Harness documents each model-facing component in terms of what the
 * model sees, token cost, and KV-cache effect. Governor adopts that as an
 * admission requirement alongside its existing authority/security manifest.
 */

export type ModelSurface =
	| "none"
	| "system-prompt"
	| "tool-schema"
	| "session-context"
	| "tool-result"
	| "dynamic";

export interface ComponentExperienceDescriptor {
	readonly component: string;
	readonly authority: "none" | "advisory" | "policy" | "lifecycle" | "external-effect";
	readonly modelSurface: ModelSurface;
	/** Human- and benchmark-readable description of steady-state/token-growth cost. */
	readonly tokenEffect: string;
	/** Which changes invalidate reusable request-prefix/KV-cache state. */
	readonly cacheEffect: string;
	/** Whether the component can execute code or invoke tools/processes. */
	readonly executable: boolean;
}

export class ComponentExperienceError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "ComponentExperienceError";
	}
}

function requireText(value: string, field: string): void {
	if (value.length === 0 || value.trim() !== value) {
		throw new ComponentExperienceError(`${field} must be non-empty and trimmed`);
	}
}

/**
 * Keep admission metadata honest enough for Governor Bench and review.
 * "none" still needs token/cache explanations because absence should be an
 * explicit measured claim rather than an omitted field.
 */
export function validateComponentExperience(descriptor: ComponentExperienceDescriptor): void {
	requireText(descriptor.component, "component");
	requireText(descriptor.tokenEffect, "tokenEffect");
	requireText(descriptor.cacheEffect, "cacheEffect");
	if (descriptor.authority === "external-effect" && !descriptor.executable) {
		throw new ComponentExperienceError("an external-effect authority must be declared executable");
	}
}
