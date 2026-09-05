/**
 * The Claude models this provider offers, and the rule that no model id ever
 * asks for a 1M window.
 *
 * Claude Code decides the context window; the adapter child is launched with
 * `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` (see `child-env.ts`), so a `[1m]` suffix
 * on a model id would be asking for a window this product has already switched
 * off — and, on a subscription, 1M turns bill as Extra Usage on top of the plan.
 * `assertNoLongContextSuffix` is the one-line invariant, and the conformance
 * suite runs it against the ids actually registered.
 *
 * Metadata is projected from Prime's own catalogue where Prime has the entry,
 * so names and thinking-level maps stay whatever the substrate says they are.
 * `cost` is zeroed: these turns are plan-billed, and a per-token price in the
 * footer would be fiction.
 */

/** Selection and display order. A partial match resolves to the first listed. */
export const MODEL_IDS_IN_ORDER = [
	"claude-opus-5",
	"claude-opus-4-8",
	"claude-opus-4-7",
	"claude-opus-4-6",
	"claude-sonnet-5",
	"claude-sonnet-4-6",
	"claude-haiku-4-5",
] as const;

/**
 * The window every model is registered with.
 *
 * Not a guess: it is the window Claude Code serves with the long-context switch
 * off. Registering anything larger would misreport Prime's own context gauge.
 */
export const REGISTERED_CONTEXT_WINDOW = 200_000;

export interface CatalogueModel {
	readonly id: string;
	readonly name: string;
	readonly reasoning: boolean;
	readonly input: ("text" | "image")[];
	readonly contextWindow: number;
	readonly maxTokens: number;
	readonly thinkingLevelMap?: Record<string, string | null>;
	readonly cost: { input: number; output: number; cacheRead: number; cacheWrite: number };
}

/** Names used when the substrate's catalogue has no entry for an id. */
const FALLBACK_NAMES: Record<string, string> = {
	"claude-opus-5": "Claude Opus 5",
	"claude-opus-4-8": "Claude Opus 4.8",
	"claude-opus-4-7": "Claude Opus 4.7",
	"claude-opus-4-6": "Claude Opus 4.6",
	"claude-sonnet-5": "Claude Sonnet 5",
	"claude-sonnet-4-6": "Claude Sonnet 4.6",
	"claude-haiku-4-5": "Claude Haiku 4.5",
};

/**
 * Throws if any id would request an extended context window.
 *
 * Exported and called at registration time rather than left as a comment: this
 * is a spending rule, and the only way it stays true through an edit is if a
 * violation fails loudly.
 */
export function assertNoLongContextSuffix(ids: readonly string[]): void {
	const offenders = ids.filter((id) => /\[\s*\d+\s*m\s*\]/i.test(id));
	if (offenders.length > 0) {
		throw new Error(
			`claude-acp: model id(s) ${offenders.join(", ")} request an extended context window. ` +
				"Command Governor runs Claude Code with CLAUDE_CODE_DISABLE_1M_CONTEXT=1 and never spends Extra Usage; the window is Claude Code's decision.",
		);
	}
}

/** Project the substrate's catalogue down to the models and fields Prime needs. */
export function buildModels<T extends { id: string; name?: string; reasoning?: boolean; input?: unknown; maxTokens?: number; thinkingLevelMap?: Record<string, string | null> }>(
	substrateModels: readonly T[],
): CatalogueModel[] {
	const models = MODEL_IDS_IN_ORDER.map((id) => {
		const found = substrateModels.find((entry) => entry.id === id);
		return {
			id,
			name: found?.name ?? FALLBACK_NAMES[id] ?? id,
			reasoning: found?.reasoning ?? true,
			input: ["text", "image"] as ("text" | "image")[],
			contextWindow: REGISTERED_CONTEXT_WINDOW,
			maxTokens: found?.maxTokens ?? 64_000,
			...(found?.thinkingLevelMap ? { thinkingLevelMap: found.thinkingLevelMap } : {}),
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		} satisfies CatalogueModel;
	});
	assertNoLongContextSuffix(models.map((model) => model.id));
	return models;
}
