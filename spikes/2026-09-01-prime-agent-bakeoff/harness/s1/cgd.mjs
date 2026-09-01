// Minimal raw client for the Prime Agent public daemon protocol (JSONL over a Unix socket).
// Deliberately independent of Prime's own DaemonClient so the bake-off observes the wire,
// not the vendor's client conveniences. Records every line in/out to an evidence log.
import net from "node:net";
import fs from "node:fs";
import { randomUUID } from "node:crypto";

export const PROTO = { name: "prime-agent.daemon", version: 7 };

export function launchEnv(source = process.env) {
  const env = {};
  // Prime forwards the whole client env to the supervisor; the harness withholds secret-shaped keys so
  // the evidence logs never carry credentials. The daemon behaviour under test does not depend on them.
  for (const [k, v] of Object.entries(source)) if (v !== undefined && !k.startsWith("PRIME_AGENT_INTERNAL_") && !/TOKEN|SECRET|PASSWORD|PASSWD|API_KEY|PRIVATE|CREDENTIAL|SESSION_KEY|SECURITYSESSIONID|SSH_AUTH_SOCK|MESSAGING_SOCKET/i.test(k)) env[k] = v;
  return env;
}

export class RawDaemonClient {
  constructor(socketPath, { clientId = `cg-raw:${randomUUID()}`, log } = {}) {
    this.socketPath = socketPath; this.clientId = clientId; this.logPath = log;
    this.pending = new Map(); this.events = []; this.listeners = new Set(); this.hello = undefined; this.buf = "";
    this.lastCursor = undefined; this.closed = false; this.n = 0;
  }
  log(dir, obj) { if (this.logPath) fs.appendFileSync(this.logPath, JSON.stringify({ ts: new Date().toISOString(), dir, client: this.clientId, ...obj }) + "\n"); }
  connect(timeoutMs = 5000) {
    return new Promise((resolve, reject) => {
      const sock = net.createConnection(this.socketPath);
      this.sock = sock;
      const t = setTimeout(() => { sock.destroy(); reject(new Error("connect timeout")); }, timeoutMs);
      sock.once("connect", () => { clearTimeout(t); });
      sock.once("error", (e) => { clearTimeout(t); this.failAll(e); reject(e); });
      sock.on("close", () => { this.closed = true; this.failAll(new Error("socket closed")); for (const l of this.listeners) l({ type: "__closed" }); });
      sock.on("data", (d) => { this.buf += d.toString("utf8"); let i; while ((i = this.buf.indexOf("\n")) >= 0) { const line = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); if (line.trim()) this.handle(line); } });
      this.helloPromise = new Promise((res, rej) => { this.helloResolve = res; setTimeout(() => rej(new Error("hello timeout")), timeoutMs); });
      this.helloPromise.then(resolve, reject);
    });
  }
  handle(line) {
    let msg; try { msg = JSON.parse(line); } catch { this.log("in-bad", { line: line.slice(0, 200) }); return; }
    this.log("in", { msg });
    if (msg.type === "daemon_hello") { this.hello = msg; this.helloResolve?.(msg); return; }
    if (msg.type === "response" && msg.id && this.pending.has(msg.id)) { const p = this.pending.get(msg.id); this.pending.delete(msg.id); p.resolve(msg); return; }
    const cursor = msg.cursor ?? msg.meta?.cursor;
    if (msg.type === "event" || msg.type === "session_event" || cursor) { if (cursor) { this.lastCursor = cursor; msg.cursor = cursor; } this.events.push(msg); }
    for (const l of this.listeners) l(msg);
  }
  failAll(e) { for (const [, p] of this.pending) p.reject(e); this.pending.clear(); }
  on(fn) { this.listeners.add(fn); return () => this.listeners.delete(fn); }
  /** Send a versioned command envelope with a stable command id; returns the response envelope. */
  request(command, { id = `${this.clientId.slice(-8)}-${++this.n}-${randomUUID().slice(0, 6)}`, timeoutMs = 60000 } = {}) {
    const env = { type: "command", id, protocol: PROTO, clientId: this.clientId, command };
    this.log("out", { msg: env });
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => { this.pending.delete(id); reject(new Error(`timeout waiting for ${command.type} ${id}`)); }, timeoutMs);
      this.pending.set(id, { resolve: (m) => { clearTimeout(t); resolve(m); }, reject: (e) => { clearTimeout(t); reject(e); } });
      this.sock.write(JSON.stringify(env) + "\n");
    });
  }
  /** Fire-and-forget: write the envelope and return the id without waiting (for crash-window tests). */
  send(command, id) {
    id ??= `${this.clientId.slice(-8)}-${++this.n}-${randomUUID().slice(0, 6)}`;
    const env = { type: "command", id, protocol: PROTO, clientId: this.clientId, command };
    this.log("out", { msg: env }); this.sock.write(JSON.stringify(env) + "\n"); return id;
  }
  ack(commandId) { return this.request({ type: "ack_result", commandId }); }
  waitFor(pred, timeoutMs = 30000) {
    return new Promise((resolve, reject) => {
      const hit = this.events.find(pred); if (hit) return resolve(hit);
      const t = setTimeout(() => { off(); reject(new Error("waitFor timeout")); }, timeoutMs);
      const off = this.on((m) => { if ((m.type === "event" || m.type === "session_event") && pred(m)) { clearTimeout(t); off(); resolve(m); } });
    });
  }
  close() { this.sock?.destroy(); }
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export function pidAlive(pid) { try { process.kill(pid, 0); return true; } catch { return false; } }
export async function waitUntil(fn, timeoutMs = 20000, every = 100) { const end = Date.now() + timeoutMs; for (;;) { const v = await fn(); if (v) return v; if (Date.now() > end) throw new Error("waitUntil timeout"); await sleep(every); } }
export async function connectEventually(socketPath, opts, timeoutMs = 20000) {
  const end = Date.now() + timeoutMs; let last;
  while (Date.now() < end) { const c = new RawDaemonClient(socketPath, opts); try { await c.connect(1500); return c; } catch (e) { last = e; c.close(); await sleep(150); } }
  throw new Error(`no supervisor on ${socketPath}: ${last}`);
}
