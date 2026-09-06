# Substrate defect: the model picker lists every built-in model, not the ones a credential can run

Status: **patched in this repository**, not filed upstream. Prime's
contribution gate admits changes only from approved contributors (owner's
finding, 2026-09-05), so this record is the whole trail: what is wrong, the
one-line delta carried under `pins/patches/`, and how the conformance suite
proves it is on the installed tree. Re-check at every re-pin (ritual step 4 in
`docs/prime-distribution.md`); close this record when a release ships the
behaviour and the patch no longer applies because it is already there.

**Present on Prime 0.9.2** (`9c54a35d`), measured 2026-09-05 on the pinned
install with an empty `auth.json` and no provider API key in the environment.

## What happens

`/model`, Ctrl+P and the configuration menu's Models tab list the entire
built-in catalog (1289 rows on 0.9.2: 297 openrouter, 237 vercel-ai-gateway,
122 amazon-bedrock, ...), with the usable models sorted first and every other
row tagged `sign in`. The same model id appears once per reseller
(`claude-fable-5` under claude-bridge, anthropic and cloudflare-ai-gateway),
and a fuzzy search for `fable` returns 307 rows. The header hint states the
intent: "Signed-in providers first. Other models prompt sign-in."

## Where

`ModelRegistry.refreshModelCatalog()` (`dist/core/model-registry.js`) returns
`{ models: <every model>, configuredProviders: <providers with auth> }`. The
interactive client stores both (`applyConnectionModelCatalog`) and has the
right accessor, `getAvailableConnectionModels()`, which filters the catalog by
`connectionConfiguredProviders`. It is used for exact-name matching and the
provider count. The picker, however, is built from
`getCachedModelCandidates()` (`dist/modes/interactive/interactive-mode.js`,
and the same code in the built `dist/bundle/chunk-*.js` the binary executes),
which iterates the raw `connectionModelCatalog`. `enabledModels` /
`--models` only add a "scoped" view over that list; Alt+S and search in the
"all" scope expose the full catalog again.

## The delta

`pins/patches/prime-0.9.2-model-picker-configured-only.patch`: in
`getCachedModelCandidates()`, iterate `this.getAvailableConnectionModels()`
instead of `this.connectionModelCatalog`. Session-scoped models (from
`enabledModels`) are still merged first, unchanged. Applied in both the
unbundled module and the bundle chunk. Nothing else in the client reads the
raw catalog for display.

## Proof

`conformance/tier1/pin.test.ts` reverse-dry-runs every `substrate.patches`
entry against the install root and asserts the shipped bundle's
`getCachedModelCandidates` reads the configured-provider accessor and not the
raw catalog.

## Why not a setting

Prime has no setting that hides unconfigured providers; `enabledModels` is a
positive allow-list that must be maintained by hand and still leaves the full
catalog one keystroke away. The product decision (owner, 2026-09-05) is that a
model without a credential must never be offered.
