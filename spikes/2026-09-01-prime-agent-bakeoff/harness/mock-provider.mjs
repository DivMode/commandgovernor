// Mock OpenAI-compatible chat-completions server for the substrate bake-off.
// Deterministic, credential-free, and every request is logged to a JSONL file so
// "did the runtime call the model again?" is answerable from evidence.
//
// Behaviour is selected by the LAST user message text:
//   ECHO:<text>          -> stream <text> back, end_turn
//   SLOW:<n>:<ms>        -> stream n chunks, one every ms milliseconds, then end
//   TOOL:<name>:<json>   -> emit one tool_call <name> with arguments <json>; when the
//                           tool result comes back (last message role=tool), reply "done"
//   TOOLSLOW:<ms>:<name>:<json> -> like TOOL but waits ms before emitting the call
// Anything else         -> "ok"
import http from "node:http";
import fs from "node:fs";

const port = Number(process.env.MOCK_PORT || 18765);
const logPath = process.env.MOCK_LOG || new URL("./requests.jsonl", import.meta.url).pathname;
let seq = 0;
const seenTools = new Set();

function log(entry) {
  fs.appendFileSync(logPath, JSON.stringify({ ts: new Date().toISOString(), ...entry }) + "\n");
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function lastUser(messages) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") {
      const c = messages[i].content;
      if (typeof c === "string") return c;
      if (Array.isArray(c)) return c.map((p) => p.text ?? "").join("");
    }
  }
  return "";
}

function sse(res, obj) { res.write(`data: ${JSON.stringify(obj)}\n\n`); }

async function handleChat(req, res, body) {
  const id = `chatcmpl-${++seq}`;
  const model = body.model ?? "mock";
  const messages = body.messages ?? [];
  const last = messages[messages.length - 1] ?? {};
  const text = lastUser(messages);
  const toolNames = (body.tools ?? []).map((t) => t.function?.name);
  const sysText = messages.filter((m) => m.role === "system" || m.role === "developer").map((m) => typeof m.content === "string" ? m.content : JSON.stringify(m.content)).join("\n");
  log({ kind: "request", id, model, stream: !!body.stream, lastRole: last.role, lastUser: text.slice(0, 200), nMessages: messages.length, toolNames, sysLen: sysText.length, sysHasSkillMarker: /CG-SKILL-MARKER-7f3a/.test(sysText), sysMentionsSkill: /cg-bakeoff-skill/.test(sysText) });
  for (const t of body.tools ?? []) { const n = t.function?.name; if (n && !seenTools.has(n)) { seenTools.add(n); log({ kind: "tool-schema", name: n, schema: t.function }); } }

  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
  const base = { id, object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model };
  const chunk = (delta, finish = null) => sse(res, { ...base, choices: [{ index: 0, delta, finish_reason: finish }] });
  const usage = () => sse(res, { ...base, choices: [], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } });

  chunk({ role: "assistant", content: "" });

  if (last.role === "tool") {
    chunk({ content: "done" }); chunk({}, "stop"); usage(); res.end("data: [DONE]\n\n");
    log({ kind: "response", id, mode: "after-tool", text: "done" }); return;
  }
  let m;
  if ((m = text.match(/^ECHO:(.*)$/s))) {
    chunk({ content: m[1] }); chunk({}, "stop"); usage(); res.end("data: [DONE]\n\n");
    log({ kind: "response", id, mode: "echo" }); return;
  }
  if ((m = text.match(/^SLOW:(\d+):(\d+)/))) {
    const n = Number(m[1]), ms = Number(m[2]);
    for (let i = 0; i < n; i++) { chunk({ content: `tick${i} ` }); log({ kind: "chunk", id, i }); await sleep(ms); if (res.destroyed) { log({ kind: "client-gone", id, at: i }); return; } }
    chunk({}, "stop"); usage(); res.end("data: [DONE]\n\n");
    log({ kind: "response", id, mode: "slow", n }); return;
  }
  if ((m = text.match(/^TOOL(SLOW:(\d+))?:([A-Za-z0-9_]+):(.*)$/s))) {
    const wait = m[2] ? Number(m[2]) : 0; const name = m[3]; const args = m[4];
    if (wait) { log({ kind: "tool-wait", id, ms: wait }); await sleep(wait); }
    if (res.destroyed) { log({ kind: "client-gone", id, before: "tool_call" }); return; }
    chunk({ tool_calls: [{ index: 0, id: `call_${id}`, type: "function", function: { name, arguments: "" } }] });
    chunk({ tool_calls: [{ index: 0, function: { arguments: args } }] });
    chunk({}, "tool_calls"); usage(); res.end("data: [DONE]\n\n");
    log({ kind: "response", id, mode: "tool_call", name, args: args.slice(0, 200) }); return;
  }
  chunk({ content: "ok" }); chunk({}, "stop"); usage(); res.end("data: [DONE]\n\n");
  log({ kind: "response", id, mode: "default" });
}

const server = http.createServer((req, res) => {
  let buf = "";
  req.on("data", (d) => (buf += d));
  req.on("end", async () => {
    try {
      if (req.method === "GET" && req.url?.startsWith("/v1/models")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ object: "list", data: [{ id: "mock-1", object: "model", owned_by: "cg" }] })); return;
      }
      if (req.method === "POST" && req.url?.includes("/chat/completions")) {
        await handleChat(req, res, JSON.parse(buf || "{}")); return;
      }
      log({ kind: "unhandled", method: req.method, url: req.url });
      res.writeHead(404); res.end();
    } catch (e) { log({ kind: "error", error: String(e) }); res.writeHead(500); res.end(); }
  });
});
server.listen(port, "127.0.0.1", () => { log({ kind: "listen", port }); console.log(`mock provider on http://127.0.0.1:${port}`); });
