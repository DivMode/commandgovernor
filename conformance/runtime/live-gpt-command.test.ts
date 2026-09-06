/**
 * LIVE-GPT — the /gpt command's core against the REAL ChatGPT account: a
 * gpt-5-6-thinking@max reply must be SERVED by gpt-5-6-thinking, read back from
 * the persistent conversation exactly as the command does it. OPT-IN; never a
 * merge gate.
 *
 * This is the one measurement that can come back negative when the provider
 * changes what a slug serves — the astra→mini class of failure the no-fallback
 * rule exists to catch. The credential-free tier1 test replays a fixed shape
 * and cannot see a live provider change; this can.
 *
 *   CG_LIVE=1 scripts/conformance.sh          (needs ~/.codex/auth.json)
 *   CG_LIVE=1 node --test conformance/runtime/live-gpt-command.test.ts
 *
 * It drives the vendored pi-gpt modules directly (the same ConversationClient +
 * served.ts the /gpt command calls), because slash commands are a client-side
 * surface the mock-provider print path cannot reach. The prompt is NON-TRIVIAL
 * on purpose: a gpt-5-mini fast path intercepts trivial prompts on any model, so
 * a trivial probe would measure the fast path, not the requested model.
 *
 * What it leaves behind: ONE persistent chat in the account (persistence is
 * required — a temporary chat 404s the served-model readback). It is titled as a
 * Command Governor probe. pi-gpt wraps no archive/delete endpoint, so it is left
 * in place; this is an opt-in, user-run lane against the user's own account.
 */

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import { readPins, REPO_ROOT } from "../lib/repo.ts";

const codexHome = process.env.CODEX_HOME ?? join(homedir(), ".codex");
const enabled = process.env.CG_LIVE === "1";
const loggedIn = existsSync(join(codexHome, "auth.json"));
const pinned = readPins().packages.find((entry) => String(entry.source).includes("pi-gpt"));
const source = pinned ? join(REPO_ROOT, String(pinned.source).replace(/^\.\//, "")) : "";
const extracted = source ? existsSync(join(source, "src", "served.ts")) : false;
const reason =
  !enabled ? "opt-in: set CG_LIVE=1 to run against the real ChatGPT account"
  : !loggedIn ? `CG_LIVE=1 but no Codex login at ${codexHome}/auth.json`
  : !extracted ? `${source} is not extracted; run scripts/bootstrap.sh first`
  : undefined;

interface Conv { complete(model: string, messages: { role: string; content: string }[], opts: Record<string, unknown>): Promise<{ text: string; conversationId: string | null }> }
interface Backend { get(path: string): Promise<unknown> }

// pi-gpt is authored for jiti/bun (it uses TS parameter properties, which Node's
// strip-only loader rejects), so load its modules exactly as Prime does — through
// the jiti transformer shipped in the pinned install — rather than via a bare
// dynamic import. This is the same loader Prime uses to run the /gpt command.
async function load(): Promise<{ backend: Backend; conv: Conv; servedModelFromConversationDetail(d: unknown): string | null; assertServedModel(s: string | null, r: string, o: { match?: "exact" | "family"; context: string }): void }> {
  const jitiMod = await import(join(REPO_ROOT, "pins", "prime-0.9.2", "node_modules", "jiti", "lib", "jiti.mjs"));
  const jiti = (jitiMod.createJiti ?? jitiMod.default)(import.meta.url);
  const client = await jiti.import(join(source, "src", "client.ts"));
  const conversation = await jiti.import(join(source, "src", "conversation.ts"));
  const served = await jiti.import(join(source, "src", "served.ts"));
  const backend = new client.BackendClient() as Backend;
  const conv = new conversation.ConversationClient(backend) as unknown as Conv;
  return { backend, conv, servedModelFromConversationDetail: served.servedModelFromConversationDetail, assertServedModel: served.assertServedModel };
}

describe("LIVE-GPT: the requested model is the served model", { skip: reason }, () => {
  it("LGPT-001: gpt-5-6-thinking@max is served by gpt-5-6-thinking (non-trivial prompt)", async () => {
    const { backend, conv, servedModelFromConversationDetail, assertServedModel } = await load();
    const marker = `CG-LIVE-${Date.now().toString(36).toUpperCase()}`;
    // Non-trivial: a small review task with a decision to make, so the mini
    // fast path does not intercept it.
    const prompt =
      `Command Governor served-model probe ${marker}. Review this function and answer in 2-3 sentences: ` +
      "does it correctly compute an average, and what is one edge case it mishandles?\n\n" +
      "```js\nfunction avg(xs){ let s=0; for(const x of xs) s+=x; return s/xs.length; }\n```";

    const result = await conv.complete(
      "gpt-5-6-thinking",
      [{ role: "user", content: prompt }],
      { thinkingEffort: "max", temporary: false, signal: undefined },
    );
    assert.ok(result.text && result.text.trim().length > 0, "no answer text came back");
    assert.match(String(result.conversationId ?? ""), /^[0-9a-f-]{36}$/, "no persistent conversation id came back");

    const detail = await backend.get(`/backend-api/conversation/${result.conversationId}`);
    const served = servedModelFromConversationDetail(detail);
    // The whole point: the backend served the requested model, not a downgrade.
    assert.equal(served, "gpt-5-6-thinking", `requested gpt-5-6-thinking@max but the backend served ${served}`);
    assert.doesNotThrow(() => assertServedModel(served, "gpt-5-6-thinking", { match: "exact", context: "LIVE /gpt chat" }));
  });
});
