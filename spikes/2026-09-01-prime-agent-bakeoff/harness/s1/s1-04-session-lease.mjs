import fs from "node:fs";
import { spawnSync } from "node:child_process";
import * as C from "./common.mjs";
const RUN = Date.now().toString(36);
const out = C.evidence("s1-04-session-lease");
const c = await C.newClient("cg-lease-1", out);
const A = await C.createRoot(c, { name: `cg-s1-04-${RUN}` }); const idA = C.sid(A);
await C.attach(c, idA); await C.prompt(c, idA, "ECHO:lease-owner"); await C.waitForIdle(c, idA);
out("owner", { id: idA, file: A.sessionFile, pid: A.workerPid });
const bytes0 = fs.readFileSync(A.sessionFile, "utf8"); const workers0 = C.workerDescriptors().length;
// (1) a second daemon client issues create on the same path
const c2 = await C.newClient("cg-lease-contender", out);
let r; try { r = await C.createRoot(c2, { sessionPath: A.sessionFile }); } catch (e) { r = e.response; }
out("(1) contender create on the owned path", r?.activeSessionId ? { converged: true, activeSessionId: C.sid(r), workerPid: r.workerPid } : r);
out.check("(1) contender receives the owning active-session identity (converged, not a second writer)", (r?.activeSessionId ? C.sid(r) === idA && r.workerPid === A.workerPid : r?.errorInfo?.code === "session_already_active" && r?.errorInfo?.activeSessionId === idA), r?.errorInfo ?? { convergedTo: C.sid(r) });
out.check("(1) no additional worker was launched", C.workerDescriptors().length === workers0 && (await C.list(c2)).filter((s) => s.sessionFile === A.sessionFile).length === 1);
// (2) another resident root tries to switch onto the owned path
const B = await C.createRoot(c2, { name: `cg-s1-04-B-${RUN}` }); const idB = C.sid(B);
const sw = await c2.request({ type: "switch_session", activeSessionId: idB, sessionPath: A.sessionFile }, { timeoutMs: 60000 });
out("(2) switch_session from another root onto the owned path", { success: sw.success, code: sw.errorInfo?.code, owner: sw.errorInfo?.activeSessionId, error: sw.error?.slice(0, 200) });
out.check("(2) rejected with typed session_already_active naming the owner", sw.success === false && sw.errorInfo?.code === "session_already_active" && sw.errorInfo?.activeSessionId === idA);
// (3) a one-shot CLI client (client-owned worker) resumes the owned path
const cli = spawnSync(C.PA, ["-p", "--resume", A.sessionFile, "--provider", "mock", "--model", "mock-1", "--no-extensions", "--no-skills", "ECHO:lease-cli"], { env: process.env, encoding: "utf8", input: "", timeout: 90000, cwd: C.WORK });
out("(3) one-shot CLI --resume on the owned path", { status: cli.status, stdout: cli.stdout.slice(0, 300), stderr: cli.stderr.slice(0, 500) });
out.check("(3) one-shot client refused rather than writing the owned transcript", cli.status !== 0 && /already active|session_already_active/i.test(cli.stdout + cli.stderr));
const bytes1 = fs.readFileSync(A.sessionFile, "utf8");
out.check("transcript bytes unchanged by all three contenders", bytes0 === bytes1, { before: bytes0.length, after: bytes1.length });
out.check("transcript still parses line by line", C.readJsonl(A.sessionFile).every((e) => !e.__bad));
out("lease dir entries", fs.existsSync(`${C.AGENT_DIR}/session-leases`) ? fs.readdirSync(`${C.AGENT_DIR}/session-leases`).length : "absent");
const p = await C.prompt(c, idA, "ECHO:lease-owner-still-works"); await C.waitForIdle(c, idA); out.check("owner keeps working after contention", p.success);
c.close(); c2.close(); process.exit(out.failed ? 1 : 0);
