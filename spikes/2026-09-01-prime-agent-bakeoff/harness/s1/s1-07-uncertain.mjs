import fs from "node:fs";
import * as C from "./common.mjs";
const RUN = Date.now().toString(36);
const out = C.evidence("s1-07-uncertain");
const lines = (F) => (fs.existsSync(F) ? fs.readFileSync(F, "utf8").split("\n").filter(Boolean).length : 0);
const jentries = (cmdId) => C.commandJournals().flatMap((f) => C.readJsonl(f)).filter((e) => JSON.stringify(e).includes(cmdId));
{ // (a) supervisor dies after admission/journaling, before the client has a durable result
  const CID = `cg-unc-client-a-${RUN}`; const c = await C.connectEventually(C.SOCK, { clientId: CID, log: out.wire });
  const A = await C.createRoot(c, { name: `cg-s1-07a-${RUN}` }); const id = C.sid(A); await C.attach(c, id);
  const F = `${C.WORK}/unc-a-${RUN}.txt`; const cmdId = `cg-unc-a-${RUN}`;
  const command = { type: "execute_bash_and_wait", activeSessionId: id, command: `sleep 4; echo unc >> ${JSON.stringify(F)}` };
  const P0 = c.hello.supervisorPid; c.send(command, cmdId); await C.sleep(400);
  out("(a) journal right after send", jentries(cmdId).map((e) => e.type));
  out("(a) SIGKILL supervisor mid-command", P0, C.kill(P0, "SIGKILL")); c.close();
  const c2 = await C.connectEventually(C.SOCK, { clientId: CID, log: out.wire }, 60000); out("(a) replacement supervisor", c2.hello.supervisorPid);
  await C.sleep(6000);
  const r = await C.request(c2, command, cmdId); out("(a) retry with the same clientId+commandId", { success: r.success, code: r.errorInfo?.code, error: r.error?.slice(0, 160) });
  out.check("(a) retry is reported uncertain, not re-executed", r.success === false && r.errorInfo?.code === "command_result_uncertain");
  const l1 = lines(F); const r2 = await C.request(c2, command, cmdId); await C.sleep(5500); const l2 = lines(F);
  out("(a) target lines after first retry / after second retry", l1, l2, { secondRetry: r2.errorInfo?.code });
  out.check("(a) target mutated at most once and never again on retries", l1 <= 1 && l2 === l1);
  out.check("(a) journal holds the receipt without a durable result", jentries(cmdId).some((e) => e.type === "received") && !jentries(cmdId).some((e) => e.type === "result"), jentries(cmdId).map((e) => e.type));
  const fresh = await C.request(c2, command, `${cmdId}-fresh`); out.check("(a) uncertainty is scoped to the command identity; a new id on the same root executes", fresh.success === true, { lines: lines(F) });
  c2.close();
}
{ // (b) worker dies after admission, before any effect (sleep first)
  const CID = `cg-unc-client-b-${RUN}`; const c = await C.connectEventually(C.SOCK, { clientId: CID, log: out.wire });
  const B = await C.createRoot(c, { name: `cg-s1-07b-${RUN}` }); const id = C.sid(B); await C.attach(c, id);
  const F = `${C.WORK}/unc-b-${RUN}.txt`; const cmdId = `cg-unc-b-${RUN}`;
  const command = { type: "execute_bash_and_wait", activeSessionId: id, command: `sleep 4; echo unc >> ${JSON.stringify(F)}` };
  const pending = c.request(command, { id: cmdId, timeoutMs: 30000 }).then((x) => x, (e) => ({ error: String(e) }));
  await C.sleep(400); out("(b) SIGKILL worker mid-command", B.workerPid, C.kill(B.workerPid, "SIGKILL"));
  const first = await pending; out("(b) response to the in-flight command", { success: first.success, code: first.errorInfo?.code, error: (first.error ?? "").slice(0, 200) });
  const r = await C.recoverRoot(c, id, B.workerPid, out); const id2 = C.sid(r.summary);
  await C.sleep(5000);
  const ra = await C.request(c, command, cmdId); const rb = await C.request(c, { ...command, activeSessionId: id2 }, cmdId);
  out("(b) retries with the same clientId+commandId (old id / reopened id)", { a: { success: ra.success, code: ra.errorInfo?.code, error: ra.error?.slice(0, 80) }, b: { success: rb.success, code: rb.errorInfo?.code, error: rb.error?.slice(0, 80) } });
  out("(b) journal entries", jentries(cmdId).map((e) => ({ type: e.type, success: e.response?.success, error: e.response?.error?.slice(0, 60) })));
  out.check("(b) the mutation was not re-executed on retry", ra.success === false && rb.success === false && lines(F) === 0, { lines: lines(F) });
  out.check("(b) retry surfaces the loss explicitly (uncertain code or the stored socket-loss failure), never a fabricated success", (ra.errorInfo?.code === "command_result_uncertain" || /socket closed/i.test(ra.error ?? "")), ra.error);
  out("(b) observation: worker loss mid-command is journaled as a definite failure result, not as command_result_uncertain", jentries(cmdId).map((e) => e.type));
  c.close();
}
{ // (c) worker dies AFTER the external effect but before the result is durable -> does the stored verdict lie?
  const CID = `cg-unc-client-c-${RUN}`; const c = await C.connectEventually(C.SOCK, { clientId: CID, log: out.wire });
  const Cc = await C.createRoot(c, { name: `cg-s1-07c-${RUN}` }); const id = C.sid(Cc); await C.attach(c, id);
  const F = `${C.WORK}/unc-c-${RUN}.txt`; const cmdId = `cg-unc-c-${RUN}`;
  const command = { type: "execute_bash_and_wait", activeSessionId: id, command: `echo unc >> ${JSON.stringify(F)}; sleep 4` };
  const pending = c.request(command, { id: cmdId, timeoutMs: 30000 }).then((x) => x, (e) => ({ error: String(e) }));
  await C.waitUntil(() => lines(F) === 1, 5000, 50); out("(c) effect landed on disk; SIGKILL worker before the command can complete", Cc.workerPid, C.kill(Cc.workerPid, "SIGKILL"));
  const first = await pending; out("(c) response to the in-flight command", { success: first.success, code: first.errorInfo?.code, error: (first.error ?? "").slice(0, 200) });
  const r = await C.recoverRoot(c, id, Cc.workerPid, out); const id2 = C.sid(r.summary);
  const rc = await C.request(c, { ...command, activeSessionId: id2 }, cmdId); out("(c) retry same clientId+commandId", { success: rc.success, code: rc.errorInfo?.code, error: rc.error?.slice(0, 120) });
  out("(c) journal entries", jentries(cmdId).map((e) => ({ type: e.type, success: e.response?.success, error: e.response?.error?.slice(0, 60) })));
  out.check("(c) not re-executed (file still has exactly one line)", lines(F) === 1, { lines: lines(F) });
  const honest = rc.errorInfo?.code === "command_result_uncertain";
  out.check("(c) CRITICAL: an effect that happened is reported as uncertain, not as a definite failure", honest, { reported: rc.errorInfo?.code ?? rc.error?.slice(0, 80), effectOnDisk: lines(F) });
  // Consequence: a client that believes the stored "failed" verdict retries as new work, and the runtime executes it.
  const again = await C.request(c, { ...command, activeSessionId: id2, command: `echo unc >> ${JSON.stringify(F)}` }, `${cmdId}-client-retry`);
  out("(c) client retries under a new command id because it was told the mutation failed", { success: again.success, lines: lines(F) });
  out.check("(c) consequence: the misreported verdict leads to a duplicate external effect", lines(F) === 1, { linesAfterClientRetry: lines(F) });
  c.close();
}
process.exit(out.failed ? 1 : 0);
