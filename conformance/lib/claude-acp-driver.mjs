// Child-process driver for conformance/runtime/claude-acp-boundary.test.ts.
//
// Runs the SHIPPED modules of harness/extensions/claude-acp under Node's type
// transform, so what the tests assert about is the code Prime loads rather than
// a copy of its rules restated in the test.
//
// Two commands, both answering on stdout with one JSON object:
//
//   env <scenario-json>     build the adapter child's environment, or report the
//                           refusal. The scenario carries the base environment
//                           and a registry stub shaped like Prime's
//                           (`getAll` + `getApiKeyAndHeaders`) or upstream Pi's
//                           (`getProviderAuth`), or none.
//   models                  the model ids the provider would register, and what
//                           the long-context guard does to a bad one.
//
// Whether a variable REACHES a real child process is NOT asserted here — that
// would only re-measure this driver. It is asserted end to end in
// claude-acp-session.test.ts, where the stub agent records the environment it
// was actually started with.

import { join } from "node:path";

const extensionDir = process.env.CG_ACP_EXTENSION_DIR;
if (!extensionDir) {
	process.stdout.write(JSON.stringify({ ok: false, error: "CG_ACP_EXTENSION_DIR is required" }));
	process.exit(2);
}

const src = (name) => join(extensionDir, "src", name);
const command = process.argv[2];
const scenario = JSON.parse(process.argv[3] ?? "{}");

function registryFor(spec) {
	if (spec?.kind === "prime") {
		return { getAll: () => [{ provider: "anthropic", id: "claude-haiku-4-5" }], getApiKeyAndHeaders: async () => spec.result };
	}
	if (spec?.kind === "pi") return { getProviderAuth: async () => spec.result };
	return undefined;
}

if (command === "env") {
	const { resolveAcpChildEnv, STRIPPED_ENV_KEYS, FORCED_CHILD_ENV } = await import(src("child-env.ts"));
	try {
		const env = await resolveAcpChildEnv(registryFor(scenario.registry), scenario.base ?? {});
		process.stdout.write(JSON.stringify({ ok: true, env, stripped: STRIPPED_ENV_KEYS, forced: FORCED_CHILD_ENV }));
	} catch (error) {
		process.stdout.write(JSON.stringify({ ok: false, refused: true, error: String(error?.message ?? error), stripped: STRIPPED_ENV_KEYS }));
	}
	process.exit(0);
}

if (command === "models") {
	const { buildModels, assertNoLongContextSuffix, MODEL_IDS_IN_ORDER, REGISTERED_CONTEXT_WINDOW } = await import(src("models.ts"));
	const registered = buildModels([]);
	let guardRejected = null;
	try {
		assertNoLongContextSuffix(["claude-haiku-4-5", "claude-opus-5[1m]"]);
		guardRejected = false;
	} catch (error) {
		guardRejected = String(error?.message ?? error);
	}
	process.stdout.write(
		JSON.stringify({
			ok: true,
			ids: registered.map((model) => model.id),
			contextWindows: registered.map((model) => model.contextWindow),
			costs: registered.map((model) => model.cost),
			order: [...MODEL_IDS_IN_ORDER],
			registeredContextWindow: REGISTERED_CONTEXT_WINDOW,
			guardRejected,
		}),
	);
	process.exit(0);
}

process.stdout.write(JSON.stringify({ ok: false, error: `unknown command ${String(command)}` }));
process.exit(2);
