/*
 * DSH donor pattern: behavior lives behind replaceable capability seams.
 * Governor keeps the idea but combines it with the existing one-authority rule:
 * one capability has one active owner unless a higher-level transaction
 * explicitly replaces it. Silent first-wins/last-wins collisions are forbidden.
 */

export type GovernorCapabilityName =
	| "event-spine"
	| "session-projection"
	| "child-agents"
	| "workflow-engine"
	| "sandbox"
	| "memory"
	| "tools"
	| "foreman-transport";

export interface CapabilityBinding<T = unknown> {
	readonly name: GovernorCapabilityName;
	readonly owner: string;
	readonly value: T;
}

export class CapabilityAlreadyOwned extends Error {
	readonly capability: GovernorCapabilityName;
	readonly owner: string;

	constructor(capability: GovernorCapabilityName, owner: string) {
		super(`Governor capability ${capability} is already owned by ${owner}`);
		this.name = "CapabilityAlreadyOwned";
		this.capability = capability;
		this.owner = owner;
	}
}

export class CapabilityRegistry {
	readonly #bindings = new Map<GovernorCapabilityName, CapabilityBinding>();

	register<T>(binding: CapabilityBinding<T>): () => void {
		const existing = this.#bindings.get(binding.name);
		if (existing !== undefined) throw new CapabilityAlreadyOwned(binding.name, existing.owner);
		this.#bindings.set(binding.name, binding as CapabilityBinding);
		let active = true;
		return () => {
			if (!active) return;
			active = false;
			if (this.#bindings.get(binding.name) === binding) this.#bindings.delete(binding.name);
		};
	}

	get<T = unknown>(name: GovernorCapabilityName): CapabilityBinding<T> | undefined {
		return this.#bindings.get(name) as CapabilityBinding<T> | undefined;
	}

	require<T = unknown>(name: GovernorCapabilityName): CapabilityBinding<T> {
		const value = this.get<T>(name);
		if (value === undefined) throw new Error(`required Governor capability is not mounted: ${name}`);
		return value;
	}

	list(): readonly CapabilityBinding[] {
		return [...this.#bindings.values()];
	}
}
