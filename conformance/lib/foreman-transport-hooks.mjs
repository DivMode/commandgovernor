// Module-resolution hook for conformance/lib/foreman-transport-driver.mjs.
//
// The vendored pi-gpt extension imports two of Prime's own packages as bare
// specifiers (`typebox`, `@earendil-works/pi-ai`). Under Prime they resolve
// from Prime's tree; the driver runs the extension outside Prime, so the same
// two names are resolved from the pinned Prime install root here and nothing
// else is redirected. Loaded with `--import`: on the main thread the file
// registers itself as the hooks module; on the hooks thread it exports
// `resolve`.

import { register } from "node:module";
import { pathToFileURL } from "node:url";
import { isMainThread } from "node:worker_threads";

if (isMainThread) register(import.meta.url);

const anchor = pathToFileURL(`${process.env.CG_PRIME_NODE_MODULES}/_anchor.mjs`).href;

export async function resolve(specifier, context, nextResolve) {
	if (specifier === "typebox" || specifier.startsWith("@earendil-works/")) {
		return nextResolve(specifier, { ...context, parentURL: anchor });
	}
	return nextResolve(specifier, context);
}
