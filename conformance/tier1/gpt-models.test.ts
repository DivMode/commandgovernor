/**
 * GPT-MODELS — no intelligence tier may point at a silent-downgrade model.
 *
 * PR #30 mapped extra_high → gpt-6-astra-wm, which echoes the requested slug in
 * default_model_slug while the backend actually serves gpt-5-mini at every
 * effort (docs/research/2026-09-06-chatgpt-web-vs-work-models.md). This test
 * guards against that regression returning: every tier of INTELLIGENCE_MAP must
 * resolve to a real, non-*-wm slug, and extra_high (the default) must be the
 * honest newest thinker, gpt-5-6-thinking at max.
 *
 * Black-box: the vendored map is imported at runtime from the bootstrap-
 * extracted pi-gpt tree named by pins.json, not statically.
 */

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { readPins, REPO_ROOT } from "../lib/repo.ts";

const pinned = readPins().packages.find((entry) => String(entry.source).includes("pi-gpt"));
const source = pinned ? join(REPO_ROOT, String(pinned.source).replace(/^\.\//, "")) : "";
const modelsPath = source ? join(source, "src", "models.ts") : "";
const reason =
  !pinned ? "pi-gpt is not pinned"
  : !existsSync(modelsPath) ? `${modelsPath} is not extracted; run scripts/bootstrap.sh first`
  : undefined;

interface ModelChoice { model: string; thinkingEffort?: string; reasoningType: string }
interface Models {
  INTELLIGENCE_MAP: Record<string, ModelChoice>;
  DEFAULT_INTELLIGENCE: string;
}
async function load(): Promise<Models> {
  return (await import(modelsPath)) as Models;
}

const WM_RE = /(^|[-_])wm$|-wm-/i;

describe("GPT-MODELS: no silent-downgrade slug in the intelligence map", { skip: reason }, () => {
  it("MDL-001: no tier maps to a *-wm work model", async () => {
    const { INTELLIGENCE_MAP } = await load();
    for (const [level, choice] of Object.entries(INTELLIGENCE_MAP)) {
      assert.ok(!WM_RE.test(choice.model), `intelligence "${level}" → ${choice.model} is a *-wm work model (silent downgrade)`);
    }
  });

  it("MDL-002: extra_high is the default and resolves to gpt-5-6-thinking at max", async () => {
    const { INTELLIGENCE_MAP, DEFAULT_INTELLIGENCE } = await load();
    assert.equal(DEFAULT_INTELLIGENCE, "extra_high");
    assert.deepEqual(INTELLIGENCE_MAP.extra_high, {
      model: "gpt-5-6-thinking",
      thinkingEffort: "max",
      reasoningType: "reasoning",
    });
  });
});
