/**
 * Post-run sweep: prove that the conformance suite left nothing behind.
 *
 * A `ps` command-line sweep is not enough and the reason is measured: Prime
 * sets `process.title = "prime-agent"`, which on macOS REPLACES the argv `ps`
 * reports. A supervisor or worker started with `--daemon-socket
 * /tmp/cg-XXXXXX/tmp/d.sock` appears as the bare string `prime-agent`, so
 * matching the fixture root finds only the children that keep a real command
 * line — uv, `/bin/bash`, the kernel python. A suite that only greps `ps`
 * would report "clean" with a leaked supervisor still listening.
 *
 * So this sweeps three things, for exactly the roots this run created (the
 * fixtures append each one to the manifest named on the command line — another
 * agent's `/tmp/cg-*` fixture is none of our business and is never touched):
 *
 *   1. a live daemon: connect to `<root>/tmp/d.sock` and see whether anything
 *      answers. This is the check `ps` cannot make.
 *   2. processes whose command line still references the root.
 *   3. the root directory itself, which every fixture removes on teardown, so
 *      one that is still there means teardown never ran.
 *
 * Usage: node conformance/sweep.ts <manifest-file>
 * Exits non-zero, with the evidence printed, if anything survived.
 */

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { socketAnswers } from "./lib/prime.ts";

const manifest = process.argv[2];
if (!manifest) {
	console.error("sweep: a manifest file is required");
	process.exit(2);
}

const roots = existsSync(manifest)
	? readFileSync(manifest, "utf8")
			.split("\n")
			.map((line) => line.trim())
			.filter(Boolean)
	: [];

if (roots.length === 0) {
	console.log("conformance: no runtime fixture roots were created; nothing to sweep");
	process.exit(0);
}

for (const root of roots) {
	if (!root.startsWith("/tmp/cg-")) {
		console.error(`sweep: refusing to act on ${root}: fixture roots live directly under /tmp/cg-`);
		process.exit(2);
	}
}

function processesReferencing(root: string): string[] {
	const output = execFileSync("ps", ["-axww", "-o", "pid=,command="], { encoding: "utf8" });
	return output
		.split("\n")
		.filter((line) => line.includes(root))
		// The sweep's own `ps` pipeline is not a survivor.
		.filter((line) => !line.includes("conformance/sweep.ts"))
		.map((line) => line.trim());
}

const findings: string[] = [];

for (const root of roots) {
	const socket = join(root, "tmp", "d.sock");
	if (await socketAnswers(socket)) findings.push(`a daemon is still listening on ${socket}`);
	for (const row of processesReferencing(root)) findings.push(`process still references ${root}: ${row.slice(0, 160)}`);
	if (!process.env.CG_KEEP_ROOTS && existsSync(root)) findings.push(`fixture root ${root} was not removed, so its teardown never ran`);
}

if (findings.length > 0) {
	for (const finding of findings) console.error(`conformance: ${finding}`);
	console.error(`conformance: ${findings.length} leftover(s) from ${roots.length} fixture root(s)`);
	process.exit(1);
}

console.log(`conformance: ${roots.length} fixture root(s) swept clean (no live daemon socket, no referencing process, no leftover root)`);
