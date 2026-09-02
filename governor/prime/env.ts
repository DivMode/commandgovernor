/**
 * The environment boundary between Command Governor and the Prime daemon.
 *
 * Gate S0 (Issue #15, finding 4) observed that Prime's own client forwards the
 * ENTIRE process environment as `launchEnv` on `create`/`attach`, minus only
 * `PRIME_AGENT_INTERNAL_*`. The supervisor also hands its own process
 * environment down to every worker it spawns (measured during Issue #17: a
 * variable set only on the supervisor appeared in a worker's `env`). So there
 * are two edges, and both are governed here:
 *
 *   1. the env the Governor gives the supervisor process it spawns;
 *   2. the `launchEnv` the Governor puts on the wire for create/attach.
 *
 * Both are built from a POSITIVE allowlist. A denylist of secret-shaped names
 * (`TOKEN`, `SECRET`, `PASSWORD`, ...) is explicitly rejected as the mechanism:
 * a secret can be called anything, and the conformance suite proves that a
 * sentinel whose name contains none of those words is still withheld.
 */

/**
 * Variables the pinned runtime needs to function, and nothing that carries
 * identity or credentials. Each entry is here for a reason:
 *
 * - `PATH`, `HOME`, `TMPDIR`, `SHELL`: process basics; Prime derives its
 *   socket directory from `TMPDIR` and its agent dir from `HOME`.
 * - `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TERM`: locale and terminal.
 * - `USER`, `LOGNAME`: read by tooling for display; not credentials.
 * - `PRIME_AGENT_CODING_AGENT_DIR`, `PRIME_AGENT_SESSION_DIR`: state roots.
 * - `PRIME_AGENT_TELEMETRY`, `PRIME_AGENT_INSTALL_UV`,
 *   `PRIME_AGENT_KERNEL_VENV`, `PRIME_AGENT_KERNEL_PYTHON`,
 *   `PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS`, `UV_CACHE_DIR`: runtime knobs
 *   the Governor sets deliberately.
 *
 * Deliberately absent: `NODE_OPTIONS` and `NODE_EXTRA_CA_CERTS` (code and
 * trust injection), `SSH_AUTH_SOCK`, anything `*_API_KEY`-shaped (provider
 * credentials reach the daemon through its own auth store or an explicit
 * profile grant, never by ambient inheritance).
 */
export const DEFAULT_LAUNCH_ENV_ALLOWLIST: readonly string[] = [
	"PATH",
	"HOME",
	"TMPDIR",
	"SHELL",
	"LANG",
	"LC_ALL",
	"LC_CTYPE",
	"TZ",
	"TERM",
	"USER",
	"LOGNAME",
	"PRIME_AGENT_CODING_AGENT_DIR",
	"PRIME_AGENT_SESSION_DIR",
	"PRIME_AGENT_TELEMETRY",
	"PRIME_AGENT_INSTALL_UV",
	"PRIME_AGENT_KERNEL_VENV",
	"PRIME_AGENT_KERNEL_PYTHON",
	"PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS",
	"UV_CACHE_DIR",
];

export interface LaunchEnvBuild {
	/** Exactly the variables that will cross the boundary. */
	readonly env: Record<string, string>;
	/** Names (never values) present in the source but not forwarded. Evidence only. */
	readonly withheld: readonly string[];
	/** The allowlist that produced `env`, for the record. */
	readonly allowlist: readonly string[];
}

export interface LaunchEnvOptions {
	/** Replaces the default allowlist entirely when given. */
	readonly allowlist?: readonly string[];
	/**
	 * Additional variables to forward, by name, on top of the allowlist. This
	 * is the only way a non-listed variable crosses, and it is a per-call,
	 * explicit grant -- the profile that needs `FOO` says `FOO`.
	 */
	readonly grant?: readonly string[];
	/** Values to set outright (they win over the source). */
	readonly overrides?: Readonly<Record<string, string>>;
}

/**
 * Build the environment that may cross the Governor -> Prime boundary.
 *
 * Pure: reads only the `source` it is given. Keys not in the allowlist or the
 * explicit grant are never copied, whatever their name looks like.
 */
export function buildLaunchEnv(
	source: Readonly<Record<string, string | undefined>>,
	options: LaunchEnvOptions = {},
): LaunchEnvBuild {
	const allowlist = [...(options.allowlist ?? DEFAULT_LAUNCH_ENV_ALLOWLIST), ...(options.grant ?? [])];
	for (const name of allowlist) {
		if (name.startsWith("PRIME_AGENT_INTERNAL_")) {
			throw new Error(`launch env: ${name} is a Prime-internal role variable and may never be forwarded`);
		}
	}
	const allowed = new Set(allowlist);
	const env: Record<string, string> = {};
	const withheld: string[] = [];
	for (const [key, value] of Object.entries(source)) {
		if (value === undefined) continue;
		if (allowed.has(key)) env[key] = value;
		else withheld.push(key);
	}
	for (const [key, value] of Object.entries(options.overrides ?? {})) {
		if (key.startsWith("PRIME_AGENT_INTERNAL_")) {
			throw new Error(`launch env: ${key} is a Prime-internal role variable and may never be forwarded`);
		}
		env[key] = value;
	}
	withheld.sort();
	return { env, withheld, allowlist };
}

/** True when every key of `env` is permitted by `allowlist` (plus `grant`). */
export function launchEnvIsWithinAllowlist(
	env: Readonly<Record<string, unknown>>,
	allowlist: readonly string[] = DEFAULT_LAUNCH_ENV_ALLOWLIST,
	grant: readonly string[] = [],
): boolean {
	const allowed = new Set([...allowlist, ...grant]);
	return Object.keys(env).every((key) => allowed.has(key));
}
