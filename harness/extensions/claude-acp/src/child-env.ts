/**
 * The environment of the ACP adapter child, and the subscription-only boundary
 * it enforces.
 *
 * Command Governor runs Claude ONLY on Claude Code's own login (the user's
 * subscription, plan-billed). Never an API key, never a harness-held OAuth
 * token, never "extra usage". This module is the single place that decides what
 * the adapter — and therefore the Claude Code process underneath it — may see,
 * so it is also the single place the conformance suite has to run to prove the
 * boundary holds.
 *
 * Two rules, and they are independent:
 *
 *   1. STRIP. Every inherited Anthropic/Claude credential or alternate-backend
 *      variable is removed from the child's environment, so a variable that
 *      happens to be exported in the user's shell cannot silently redirect
 *      billing or the endpoint. HOME, USER and PATH are deliberately kept:
 *      Claude Code finds its own login through them (macOS Keychain, keyed on
 *      the logged-in user), and without them there is no login to use.
 *
 *   2. REFUSE. If the harness itself resolves an Anthropic credential — an API
 *      key, an OAuth token, or a bearer header — the child is never spawned at
 *      all. The message names the rule so the failure is actionable rather than
 *      a mysterious 401 several seconds later.
 *
 * The adapter passes its own `process.env` into the Claude Code subprocess it
 * runs (`env: { ...process.env, ... }` in claude-agent-acp's query options), so
 * what this function returns is what the CLI ends up with.
 */

/** Resolved credential shape, as Pi-family model registries report one. */
export interface AuthResult {
	readonly auth: { readonly apiKey?: string; readonly headers?: Record<string, string | undefined> };
	readonly env?: Record<string, string>;
	readonly source?: string;
}

/** The subset of a model registry this module needs, in either family's shape. */
export interface AnthropicAuthRegistry {
	getProviderAuth?(provider: string): Promise<AuthResult | undefined>;
	getAll?(): { provider: string }[];
	getApiKeyAndHeaders?(model: { provider: string }): Promise<{ ok: boolean; apiKey?: string; headers?: Record<string, string>; env?: Record<string, string> }>;
}

/**
 * Every inherited Anthropic/Claude variable the child must never see.
 *
 * Three groups, and they are stripped for three different reasons:
 *
 *   credentials and endpoints — the subscription-only rule above;
 *   `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CLAUDE_CODE_SSE_PORT` — Command
 *     Governor's own agents frequently run INSIDE Claude Code, and leaving these
 *     set makes the child think it is a nested session;
 *   `ANTHROPIC_MODEL` — an inherited default would silently win over the model
 *     Prime selected, so model choice stays with `session/set_config_option`.
 */
export const STRIPPED_ENV_KEYS = [
	"ANTHROPIC_API_KEY",
	"ANTHROPIC_AUTH_TOKEN",
	"ANTHROPIC_OAUTH_TOKEN",
	"ANTHROPIC_IDENTITY_TOKEN",
	"ANTHROPIC_IDENTITY_TOKEN_FILE",
	"ANTHROPIC_BASE_URL",
	"ANTHROPIC_CUSTOM_HEADERS",
	"ANTHROPIC_BEDROCK_BASE_URL",
	"ANTHROPIC_VERTEX_BASE_URL",
	"CLAUDE_CODE_OAUTH_TOKEN",
	"CLAUDE_CODE_CUSTOM_OAUTH_URL",
	"CLAUDE_CODE_OAUTH_CLIENT_ID",
	"CLAUDE_CODE_USE_BEDROCK",
	"CLAUDE_CODE_USE_FOUNDRY",
	"CLAUDE_CODE_USE_VERTEX",
	"CLAUDECODE",
	"CLAUDE_CODE_ENTRYPOINT",
	"CLAUDE_CODE_SSE_PORT",
	"ANTHROPIC_MODEL",
] as const;

/**
 * Settings forced on every adapter child.
 *
 * Only one, and it is not about billing: it stops the child making update
 * checks, MCP-registry lookups and telemetry calls that a governed run has no
 * use for.
 */
export const FORCED_CHILD_ENV: Readonly<Record<string, string>> = Object.freeze({
	CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
});

/**
 * The long-context cap, which is OPT-IN and off by default.
 *
 * `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` clamps Claude Code to a 200K window. It
 * was on by default in an earlier draft of this provider on the theory that a
 * 1M turn spends the plan faster; that theory is unmeasured, and the cap has a
 * measured cost — a smaller window compacts more often, and each compaction is
 * itself a summarisation turn. So it is a setting a user turns on, not a
 * default this code imposes.
 *
 * Measured on Claude Code 2.1.261 (2026-09-05, clean environment, no
 * credential): the flag takes effect even when the model id carries an explicit
 * `[1m]` suffix — `claude-sonnet-4-6[1m]` ran and reported `contextWindow:
 * 200000` with `canonicalModel: claude-sonnet-4-6`. Without it, the same id
 * failed the turn outright ("Usage credits required for 1M context"), spending
 * zero tokens. So on an account with no usage credits the flag is what makes a
 * `[1m]` id work at all; it is never silently more expensive.
 *
 * This provider never appends `[1m]` (see `models.ts`), so with the default off
 * Claude Code simply chooses its own window.
 */
export const LONG_CONTEXT_DISABLE_KEY = "CLAUDE_CODE_DISABLE_1M_CONTEXT";

export interface ChildEnvOptions {
	/** Opt in to clamping Claude Code to a 200K window. Default: false. */
	readonly disableLongContext?: boolean;
}

export const CREDENTIAL_REFUSAL =
	"claude-acp: an Anthropic credential is configured in the harness. Command Governor runs Claude only on Claude Code's own login " +
	"(subscription, plan-billed); remove the `anthropic` auth entry and any Anthropic credential from models.json, then retry.";

/**
 * Build the adapter child's environment from `base`, refusing outright if the
 * harness resolved a credential.
 *
 * Throwing is the point: the caller must not have a child process at all in
 * that case, so there is nothing to clean up and nothing that could reach the
 * API on the wrong billing path.
 */
export function buildAcpChildEnv(base: NodeJS.ProcessEnv, resolved?: AuthResult, options: ChildEnvOptions = {}): NodeJS.ProcessEnv {
	const headers = Object.entries(resolved?.auth.headers ?? {}).filter((entry): entry is [string, string] => entry[1] != null);
	const authorization = headers.find(([name]) => name.toLowerCase() === "authorization")?.[1];
	const bearerToken = authorization?.match(/^Bearer\s+(.+)$/i)?.[1];
	const headerApiKey = headers.find(([name]) => name.toLowerCase() === "x-api-key")?.[1];
	if (resolved?.auth.apiKey ?? headerApiKey ?? bearerToken) throw new Error(CREDENTIAL_REFUSAL);

	// `resolved.env` is not merged in. Upstream registries use it to carry
	// credential and endpoint variables, which is exactly what must not reach a
	// child here; there is no credential left to carry once the check above
	// passed, so merging it could only reintroduce a backend override.
	const env: NodeJS.ProcessEnv = { ...base, ...FORCED_CHILD_ENV };
	for (const key of STRIPPED_ENV_KEYS) delete env[key];
	// Off unless asked for, and stripped rather than left to an inherited value:
	// whether the window is capped is this configuration's decision, not the
	// shell's.
	delete env[LONG_CONTEXT_DISABLE_KEY];
	if (options.disableLongContext) env[LONG_CONTEXT_DISABLE_KEY] = "1";
	return env;
}

/**
 * Resolve the `anthropic` provider's credential through whichever shape the
 * registry offers, then build the environment.
 *
 * Prime's `ModelRegistry` has no `getProviderAuth()`; its resolver is
 * `getApiKeyAndHeaders(model)`. Upstream Pi has the former. Both are handled so
 * the refusal is keyed on the credential rather than on which harness is
 * running, and a registry that answers neither is treated as "no credential" —
 * which is the normal, correct state for this product.
 */
export async function resolveAcpChildEnv(
	registry: AnthropicAuthRegistry | null | undefined,
	base: NodeJS.ProcessEnv = process.env,
	options: ChildEnvOptions = {},
): Promise<NodeJS.ProcessEnv> {
	return buildAcpChildEnv(base, registry ? await resolveAnthropicAuth(registry) : undefined, options);
}

async function resolveAnthropicAuth(registry: AnthropicAuthRegistry): Promise<AuthResult | undefined> {
	if (typeof registry.getProviderAuth === "function") return registry.getProviderAuth("anthropic");
	if (typeof registry.getAll !== "function" || typeof registry.getApiKeyAndHeaders !== "function") return undefined;
	const model = registry.getAll().find((entry) => entry.provider === "anthropic");
	if (!model) return undefined;
	const result = await registry.getApiKeyAndHeaders(model);
	if (!result?.ok || !result.apiKey) return undefined;
	return {
		auth: { apiKey: result.apiKey, headers: result.headers },
		env: result.env,
		source: result.apiKey.startsWith("sk-ant-oat") ? "prime:oauth" : "prime:stored",
	};
}
