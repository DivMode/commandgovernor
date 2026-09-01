// Turn evidence/*.log into a markdown summary: per scenario, PASS/FAIL lines plus key observations.
import fs from "node:fs";
const dir = new URL("./evidence/", import.meta.url).pathname;
const order = ["s1-01-client-detach","s1-02-supervisor-replacement","s1-03-worker-crash-isolation","s1-04-session-lease","s1-05-generation-reconnect","s1-06-idempotence","s1-07-uncertain","s1-08-schedule","s1-09-child-recovery"];
let md = "";
for (const name of order) {
  const f = `${dir}${name}.log`; if (!fs.existsSync(f)) { md += `\n### ${name}\n\n_not run_\n`; continue; }
  const lines = fs.readFileSync(f, "utf8").split("\n").filter(Boolean).map((l) => l.replace(/^\[[^\]]+\] /, ""));
  const checks = lines.filter((l) => /^(PASS|FAIL) /.test(l));
  const obs = lines.filter((l) => /^(observation|.*observation:)/i.test(l));
  const verdict = checks.some((l) => l.startsWith("FAIL")) ? "FAIL" : checks.length ? "PASS" : "INCOMPLETE";
  md += `\n### ${name} — **${verdict}** (${checks.filter((l) => l.startsWith("PASS")).length} pass / ${checks.filter((l) => l.startsWith("FAIL")).length} fail)\n\n`;
  for (const c of checks) { const [tag, ...rest] = c.split(" "); const [label, detail] = rest.join(" ").split(" :: "); md += `- ${tag === "PASS" ? "✅" : "❌"} ${label}${detail ? `  \n  \`${detail.slice(0, 220)}\`` : ""}\n`; }
  for (const o of obs) md += `- ℹ️ ${o.slice(0, 300)}\n`;
}
process.stdout.write(md);
