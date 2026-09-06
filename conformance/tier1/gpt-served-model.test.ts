/**
 * GPT-SERVED-MODEL — the safety-critical guard behind /gpt's no-fallback rule.
 *
 * ChatGPT reports three model fields on a reply and they disagree.
 * `default_model_slug` merely ECHOES the requested slug — it is what
 * gpt-6-astra-wm uses to lie, reporting the requested model while gpt-5-mini
 * actually answered. The truth is `resolved_model_slug`, then `model_slug`.
 *
 * The firm rule (memory: gpt-command-no-fallback): if the served model is not
 * the requested one, /gpt errors loudly and never retries on a lesser model.
 * This test replays the astra→mini downgrade shape and proves the assertion
 * REJECTS it, and passes when the models match. If someone regresses
 * pickServedModel to trust default_model_slug, or softens assertServedModel,
 * this goes red.
 *
 * Black-box against the vendored package: the source under test is the
 * bootstrap-extracted pi-gpt tree named by pins.json, imported at runtime (not
 * statically), so this file stays decoupled from the vendored internals' types.
 */

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { readPins, REPO_ROOT } from "../lib/repo.ts";

const pinned = readPins().packages.find((entry) => String(entry.source).includes("pi-gpt"));
const source = pinned ? join(REPO_ROOT, String(pinned.source).replace(/^\.\//, "")) : "";
const servedPath = source ? join(source, "src", "served.ts") : "";
const reason =
  !pinned ? "pi-gpt is not pinned"
  : !existsSync(servedPath) ? `${servedPath} is not extracted; run scripts/bootstrap.sh first`
  : undefined;

// Loosely typed on purpose: the module lives in the bootstrap-extracted pins/
// tree (excluded from this repo's tsc program), so it is imported at runtime
// only. A local shape names exactly the surface this test drives.
interface AssertOpts { match?: "exact" | "family"; context: string }
interface Served {
  pickServedModel(metadata: unknown): string | null;
  servedModelFromConversationDetail(detail: unknown): string | null;
  assertServedModel(served: string | null, requested: string, opts: AssertOpts): void;
  ServedModelError: new (message: string, requested: string, served: string | null) => Error;
}
async function load(): Promise<Served> {
  return (await import(servedPath)) as Served;
}

/** A /backend-api/conversation/{id} detail whose leaf assistant carries these model fields. */
function detailWithModelFields(fields: Record<string, string>): unknown {
  return {
    current_node: "n2",
    mapping: {
      n1: { id: "n1", parent: null, children: ["n2"], message: { id: "u1", author: { role: "user" }, content: { content_type: "text", parts: ["hi"] } } },
      n2: {
        id: "n2",
        parent: "n1",
        children: [],
        message: {
          id: "a1",
          author: { role: "assistant" },
          create_time: 2,
          content: { content_type: "text", parts: ["answer"] },
          metadata: fields,
        },
      },
    },
  };
}

describe("GPT-SERVED-MODEL: the served model is read truthfully", { skip: reason }, () => {
  it("SM-001: reads resolved_model_slug, never the echoing default_model_slug", async () => {
    const { pickServedModel } = await load();
    // The astra→mini shape: default echoes the request, resolved is the truth.
    assert.equal(
      pickServedModel({ default_model_slug: "gpt-6-astra-wm", resolved_model_slug: "gpt-5-mini", model_slug: "gpt-5-mini" }),
      "gpt-5-mini",
    );
    // resolved wins over model_slug.
    assert.equal(pickServedModel({ resolved_model_slug: "gpt-5-6-thinking", model_slug: "other" }), "gpt-5-6-thinking");
    // falls back to model_slug when resolved is absent.
    assert.equal(pickServedModel({ model_slug: "gpt-5-6-thinking" }), "gpt-5-6-thinking");
    // default_model_slug ALONE is not trusted — it is the field that lies.
    assert.equal(pickServedModel({ default_model_slug: "gpt-6-astra-wm" }), null);
    assert.equal(pickServedModel({}), null);
    assert.equal(pickServedModel(null), null);
  });

  it("SM-002: the conversation-detail reader returns the served model of the leaf assistant", async () => {
    const { servedModelFromConversationDetail } = await load();
    assert.equal(
      servedModelFromConversationDetail(detailWithModelFields({ default_model_slug: "gpt-6-astra-wm", resolved_model_slug: "gpt-5-mini" })),
      "gpt-5-mini",
    );
    assert.equal(
      servedModelFromConversationDetail(detailWithModelFields({ resolved_model_slug: "gpt-5-6-thinking" })),
      "gpt-5-6-thinking",
    );
  });

  it("SM-003: assertServedModel REJECTS the astra→mini downgrade", async () => {
    const { assertServedModel, servedModelFromConversationDetail, ServedModelError } = await load();
    const served = servedModelFromConversationDetail(
      detailWithModelFields({ default_model_slug: "gpt-6-astra-wm", resolved_model_slug: "gpt-5-mini", model_slug: "gpt-5-mini" }),
    );
    assert.throws(
      () => assertServedModel(served, "gpt-6-astra-wm", { match: "exact", context: "/gpt review (test)" }),
      (e: unknown) => e instanceof ServedModelError && /downgrad|served/i.test(String((e as Error).message)),
      "a downgrade to gpt-5-mini must throw, not be accepted",
    );
    // Also rejects when the request was for the honest thinker but mini answered.
    assert.throws(() => assertServedModel("gpt-5-mini", "gpt-5-6-thinking", { match: "exact", context: "/gpt chat" }));
  });

  it("SM-004: assertServedModel PASSES only when the served model matches the request", async () => {
    const { assertServedModel } = await load();
    assert.doesNotThrow(() => assertServedModel("gpt-5-6-thinking", "gpt-5-6-thinking", { match: "exact", context: "/gpt chat" }));
    // family match: a real gpt-6-pro variant is accepted for a gpt-6-pro request.
    assert.doesNotThrow(() => assertServedModel("gpt-6-pro-2026-01-01", "gpt-6-pro", { match: "family", context: "/gpt review (pro)" }));
    // but a mini masquerading under a pro request is still rejected.
    assert.throws(() => assertServedModel("gpt-5-mini", "gpt-6-pro", { match: "family", context: "/gpt review (pro)" }));
  });

  it("SM-005: a null served read (unverifiable reply) is a hard error, not a pass", async () => {
    const { assertServedModel, ServedModelError } = await load();
    assert.throws(
      () => assertServedModel(null, "gpt-5-6-thinking", { match: "exact", context: "/gpt chat" }),
      (e: unknown) => e instanceof ServedModelError,
      "an unverifiable reply must not be trusted",
    );
  });

  // SM-006/SM-007 isolate the two independent rejection branches. For every
  // case above both branches fire, so deleting either one alone stayed green
  // (found by mutation in the PR #34 review). Each case below trips exactly one.
  it("SM-006: a *-wm/mini slug is rejected even when it EQUALS the requested slug (classifier branch alone)", async () => {
    const { assertServedModel, ServedModelError } = await load();
    // exact match would pass; only the classifier can reject these.
    assert.throws(
      () => assertServedModel("gpt-6-astra-wm", "gpt-6-astra-wm", { match: "exact", context: "/gpt chat" }),
      (e: unknown) => e instanceof ServedModelError && /downgraded\/work model/.test(String((e as Error).message)),
    );
    assert.throws(
      () => assertServedModel("gpt-5-mini", "gpt-5-mini", { match: "exact", context: "/gpt chat" }),
      (e: unknown) => e instanceof ServedModelError && /downgraded\/work model/.test(String((e as Error).message)),
    );
  });

  it("SM-007: a DIFFERENT non-mini, non-wm model is rejected (match branch alone)", async () => {
    const { assertServedModel, ServedModelError } = await load();
    // neither classifier fires; only the exact/family match can reject these.
    assert.throws(
      () => assertServedModel("gpt-5-6-instant", "gpt-5-6-thinking", { match: "exact", context: "/gpt chat" }),
      (e: unknown) => e instanceof ServedModelError && /requested gpt-5-6-thinking but the backend served gpt-5-6-instant/.test(String((e as Error).message)),
    );
    // family: a different Pro slug is not the requested family.
    assert.throws(
      () => assertServedModel("gpt-5-5-pro", "gpt-6-pro", { match: "family", context: "/gpt research" }),
      (e: unknown) => e instanceof ServedModelError && /requested gpt-6-pro but the backend served gpt-5-5-pro/.test(String((e as Error).message)),
    );
  });
});
