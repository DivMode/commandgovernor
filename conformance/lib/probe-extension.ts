/**
 * A minimal Prime extension that registers one tool with a name nothing else
 * uses. It is the POSITIVE half of the package-load probe's falsifiability
 * control: if `cg_probe_loaded` does not reach the wire, the probe is blind and
 * every "package X registered tool Y" result in the same file is worthless.
 *
 * Deliberately imports nothing. Prime resolves `@earendil-works/*` and
 * `typebox` through jiti aliases that exist only inside a running Prime, so an
 * extension written against them cannot be typechecked from this repository.
 * The parameter schema below is the JSON Schema a `Type.Object({...})` produces
 * at runtime, written out.
 */

interface ToolResult {
	content: { type: string; text: string }[];
	details: Record<string, unknown>;
}

interface ExtensionHost {
	registerTool(definition: {
		name: string;
		label: string;
		description: string;
		parameters: Record<string, unknown>;
		execute(id: string, params: Record<string, unknown>): Promise<ToolResult>;
	}): void;
}

export default function register(pi: ExtensionHost): void {
	pi.registerTool({
		name: "cg_probe_loaded",
		label: "CG load probe",
		description: "Registered by the Command Governor conformance suite to prove an extension reached the model's tool list.",
		parameters: { type: "object", properties: { note: { type: "string" } }, required: ["note"] },
		async execute(_id: string, params: Record<string, unknown>): Promise<ToolResult> {
			return { content: [{ type: "text", text: `CG_PROBE_OK:${String(params.note)}` }], details: {} };
		},
	});
}
