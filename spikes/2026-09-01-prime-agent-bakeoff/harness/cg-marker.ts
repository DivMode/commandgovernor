// Harmless bake-off extension: proves the runtime loaded a project-local extension
// by writing a marker file named in CG_EXT_MARKER when the session starts.
import { writeFileSync } from "node:fs";
export default function (pi: any) {
  const marker = process.env.CG_EXT_MARKER;
  const write = (event: string) => {
    if (marker) writeFileSync(marker, JSON.stringify({ event, pid: process.pid, at: new Date().toISOString() }) + "\n", { flag: "a" });
  };
  write("loaded");
  pi.on?.("session_start", () => write("session_start"));
  pi.registerCommand?.("cg-marker", { description: "Command Governor bake-off marker", handler: async () => {} });
}
