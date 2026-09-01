import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { RawDaemonClient, connectEventually, launchEnv, sleep, pidAlive, waitUntil } from "./cgd.mjs";
export { RawDaemonClient, connectEventually, launchEnv, sleep, pidAlive, waitUntil };

export const S = process.env.CG_S;
export const P = `${S}/bakeoff/prime`;
export const HOME = `${P}/home`;
export const AGENT_DIR = `${HOME}/.prime/agent`;
export const SESSION_DIR = `${AGENT_DIR}/sessions`;
export const WORK = `${P}/work`;
export const SOCK = `${process.env.TMPDIR.replace(/\/+$/, "")}/prime-agent-${process.getuid()}/daemon.sock`;
export const PA = `${P}/install/node_modules/.bin/prime-agent`;
export const MOCK_LOG = `${S}/bakeoff/mock/requests.jsonl`;
export const EVID = `${S}/bakeoff/s1/evidence`;

export function evidence(name) {
  const file = `${EVID}/${name}.log`; fs.writeFileSync(file, "");
  const out = (...a) => { const line = a.map((x) => (typeof x === "string" ? x : JSON.stringify(x))).join(" "); console.log(line); fs.appendFileSync(file, `[${new Date().toISOString()}] ${line}\n`); };
  out.file = file; out.wire = `${EVID}/${name}.wire.jsonl`; fs.writeFileSync(out.wire, "");
  out.check = (label, ok, detail) => { out(`${ok ? "PASS" : "FAIL"} ${label}${detail !== undefined ? " :: " + (typeof detail === "string" ? detail : JSON.stringify(detail)) : ""}`); if (!ok) out.failed = true; return ok; };
  return out;
}
export function sessionConfig(extra = {}) {
  return { cwd: WORK, agentDir: AGENT_DIR, sessionDir: SESSION_DIR, provider: "mock", model: "mock-1", noExtensions: true, noSkills: true, noContextFiles: true, noPromptTemplates: true, noThemes: true, telemetryDisabled: true, ...extra };
}
export async function createRoot(client, { name, sessionPath, config = {} } = {}) {
  const cmd = { type: "create", ...(name ? { name } : {}), ...(sessionPath ? { sessionPath } : {}), config: sessionConfig(config), launchEnv: launchEnv() };
  const r = await client.request(cmd, { timeoutMs: 120000 });
  if (!r.success) throw Object.assign(new Error(`create failed: ${r.error}`), { response: r });
  return r.data; // SessionSummary
}
export const sid = (s) => s.activeSessionId ?? s.id;
export async function attach(client, activeSessionId, { resumeCursor, capabilities } = {}) {
  const cmd = { type: "attach", activeSessionId, clientId: client.clientId, telemetryDisabled: true, launchEnv: launchEnv(), ...(resumeCursor ? { resumeCursor } : {}), ...(capabilities ? { capabilities } : {}) };
  return client.request(cmd, { timeoutMs: 60000 });
}
export async function list(client, extra = {}) { const r = await client.request({ type: "list", ...extra }); if (!r.success) throw new Error(r.error); return r.data.sessions ?? r.data; }
export async function getState(client, activeSessionId) { return client.request({ type: "get_state", activeSessionId }); }
export async function getMessages(client, activeSessionId) { const r = await client.request({ type: "get_messages", activeSessionId }); return r; }
export async function prompt(client, activeSessionId, message, extra = {}) { return client.request({ type: "prompt", activeSessionId, message, ...extra }); }
export async function waitForIdle(client, activeSessionId, timeoutMs = 120000) { return client.request({ type: "wait_for_idle", activeSessionId }, { timeoutMs }); }
export function readJsonl(file) { if (!fs.existsSync(file)) return []; return fs.readFileSync(file, "utf8").split("\n").filter(Boolean).map((l) => { try { return JSON.parse(l); } catch { return { __bad: l }; } }); }
export function mockRequests(filter) { return readJsonl(MOCK_LOG).filter((e) => e.kind === "request" && (!filter || filter(e))); }
export function mockEntries(filter) { return readJsonl(MOCK_LOG).filter(filter); }
export function supervisorDirs() { const d = `${AGENT_DIR}/daemon-workers`; return fs.existsSync(d) ? fs.readdirSync(d).map((x) => path.join(d, x)) : []; }
export function commandJournals() { return supervisorDirs().map((d) => path.join(d, "command-journal.jsonl")).filter((f) => fs.existsSync(f)); }
export function workerDescriptors() { const out = []; for (const d of supervisorDirs()) for (const f of fs.readdirSync(d)) if (f.endsWith(".json") && !f.includes("journal")) out.push({ file: path.join(d, f), ...JSON.parse(fs.readFileSync(path.join(d, f), "utf8")) }); return out; }
export function primePids() { try { return execFileSync("pgrep", ["-f", "prime-agent"], { encoding: "utf8" }).trim().split("\n").filter(Boolean).map(Number); } catch { return []; } }
export function psTree() { const out = {}; for (const pid of primePids()) { try { out[pid] = { ppid: Number(execFileSync("ps", ["-o", "ppid=", "-p", String(pid)], { encoding: "utf8" }).trim()), etime: execFileSync("ps", ["-o", "etime=", "-p", String(pid)], { encoding: "utf8" }).trim() }; } catch {} } return out; }
export function kill(pid, sig = "SIGKILL") { try { process.kill(pid, sig); return true; } catch (e) { return false; } }
export async function waitSocketGone(sock, timeoutMs = 15000) { return waitUntil(() => !fs.existsSync(sock), timeoutMs); }
export async function newClient(name, out) { const c = await connectEventually(SOCK, { clientId: name, log: out.wire }); return c; }
export async function request(client, command, id, timeoutMs = 60000) { return client.request(command, { id, timeoutMs }); }

export function socketListeners(sock) { try { const o = execFileSync("lsof", ["-U", "-a", "-p", primePids().join(","), "-F", "pn"], { encoding: "utf8" }); let pid; const out = new Set(); for (const line of o.split("\n")) { if (line.startsWith("p")) pid = Number(line.slice(1)); else if (line.startsWith("n") && line.slice(1) === sock) out.add(pid); } return [...out]; } catch { return []; } }
/** After a worker crash: wait for the supervisor's verdict. v0.8.1 marks a resident root `failed`
 *  ("Waiting for a client with fresh runtime context") and does not relaunch it; the documented client
 *  path (and the vendor's own process test) is a fresh `create` on the same session path. */
export async function recoverRoot(client, id, oldPid, out, timeoutMs = 40000) {
  const t0 = Date.now(); const seen = []; let last;
  const settled = await waitUntil(async () => { const s = (await list(client)).find((x) => sid(x) === id); last = s; const k = `${s?.workerState}/${s?.workerPid}`; if (seen.at(-1) !== k) seen.push(k); return s && (s.workerState === "failed" || (s.workerState === "ready" && s.workerPid !== oldPid)) ? s : null; }, timeoutMs, 100);
  out?.("recovery transitions (state/pid)", seen, `${Date.now() - t0}ms`);
  if (settled.workerState === "ready") return { summary: settled, path: "automatic", sameActiveId: true };
  const desc = workerDescriptors().find((d) => d.rootActiveSessionId === id); out?.("failed descriptor", { lifecycle: desc?.lifecycle, lastError: desc?.lastError, consecutiveFailures: desc?.consecutiveFailures });
  const retry = await client.request({ type: "retry_worker", activeSessionId: id }); out?.("retry_worker", retry.success ? "ok" : retry.error);
  const reopened = await createRoot(client, { sessionPath: settled.sessionFile, config: {} }).catch((e) => ({ error: e.message, response: e.response }));
  if (reopened.error) { out?.("reopen via create failed", reopened); throw new Error(`reopen failed: ${reopened.error}`); }
  out?.("reopened via create(sessionPath)", { activeSessionId: sid(reopened), sessionId: reopened.sessionId, pid: reopened.workerPid, state: reopened.workerState, sameActiveId: sid(reopened) === id });
  const ready = await waitUntil(async () => { const s = (await list(client)).find((x) => sid(x) === sid(reopened)); return s && s.workerState === "ready" ? s : null; }, timeoutMs, 100);
  return { summary: ready, path: "client-reopen", sameActiveId: sid(ready) === id, failedSummary: settled };
}
