// Child-process driver for conformance/runtime/claude-bridge-boundary.test.ts.
//
// Runs the vendored, patched pi-claude-agent-sdk's own `child-env.ts` — the
// module that decides what the Claude Code child may see — under Node's type
// transform, which the suite's default loader does not apply to a
// parameter-property class elsewhere in the package. `child-env.ts` has no
// runtime imports beyond node built-ins, so no resolution hook is needed.
//
// One command: `env <scenario-json>`. The scenario carries the base
// environment to hand the module and a registry stub shaped like Prime's
// (`getAll` + `getApiKeyAndHeaders`) or Pi's (`getProviderAuth`), or none.
// Output is one JSON object: the resulting child env, or the refusal.

const packageDir = process.env.CG_BRIDGE_DIR;
if (!packageDir) {
	process.stdout.write(JSON.stringify({ ok: false, error: "CG_BRIDGE_DIR is required" }));
	process.exit(2);
}

const { resolveClaudeChildEnv, STRIPPED_ENV_KEYS } = await import(`${packageDir}/src/child-env.ts`);
const scenario = JSON.parse(process.argv[3] ?? "{}");

let registry;
if (scenario.registry?.kind === "prime") {
	registry = {
		getAll: () => [{ provider: "anthropic", id: "claude-haiku-4-5" }],
		getApiKeyAndHeaders: async () => scenario.registry.result,
	};
} else if (scenario.registry?.kind === "pi") {
	registry = { getProviderAuth: async () => scenario.registry.result };
}

try {
	const env = await resolveClaudeChildEnv(registry, scenario.base ?? {});
	process.stdout.write(JSON.stringify({ ok: true, env, stripped: STRIPPED_ENV_KEYS }));
} catch (error) {
	process.stdout.write(JSON.stringify({ ok: false, refused: true, error: String(error?.message ?? error), stripped: STRIPPED_ENV_KEYS }));
}
