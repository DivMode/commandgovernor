/**
 * An extension that fails at load, on purpose.
 *
 * It is the NEGATIVE half of the package-load probe's control. Prime discards
 * extension load failures in headless modes — measured: a broken extension
 * produces no diagnostic on stdout or stderr under `-p`, and none under
 * `--mode json` either — so a run that loaded nothing looks exactly like a run
 * whose packages registered nothing. Pairing this with
 * `probe-extension.ts` in the same run is what turns the wire's `tools` array
 * into a measurement: the working probe's tool must be present, this one's must
 * not, and the process must still exit 0.
 *
 * The tool name it would have registered, had it loaded, is
 * `cg_probe_broken` — searched for by the test, and it must never appear.
 */

interface ExtensionHost {
	registerTool(definition: Record<string, unknown>): void;
}

throw new Error("cg-conformance: this extension fails to load on purpose (negative control for the package-load probe)");

// Unreachable. Present so the file states the tool it would have registered.
export default function register(pi: ExtensionHost): void {
	pi.registerTool({ name: "cg_probe_broken", label: "never registered", description: "unreachable" });
}
