# ChatGPT web models: which slug actually answers, and the `*-wm` downgrade — 2026-09-06

Status: **findings of record, relied on by code.** The measurements below were
established by direct probing of the user's ChatGPT account (Codex OAuth token,
`chatgpt.com/backend-api`) in the work that preceded the `/gpt` command. This
session did **not** re-probe the account — the account owner's instruction was
explicit: build on these facts, do not re-run the probes. Where a claim rests on
that earlier probing it is marked *Established*; where it is a property of the
vendored source it is marked *Verified in source* and was read here.

This document exists because `pins/packages/pi-gpt-0.4.3/src/models.ts`,
`src/served.ts`, `extensions/gpt-command.ts`, the two conformance tests
(`conformance/tier1/gpt-served-model.test.ts`, `gpt-models.test.ts`) and
ADR 0012 all reference it as the authority for two decisions: **which model each
intelligence tier maps to**, and **how the served model is read and asserted**.

---

## 1. The three model fields disagree, and one of them lies

*Established.* A ChatGPT reply's message metadata carries three model-identifying
fields:

- `default_model_slug` — **echoes the slug the request asked for.** It is a copy
  of the request, not a statement of what ran. It is the field that lies.
- `resolved_model_slug` — the model the backend actually routed to.
- `model_slug` — the model the message was produced by.

The truthful served model is therefore **the first defined of
`resolved_model_slug`, then `model_slug`** — never `default_model_slug`.

*Verified in source.* `pickServedModel` in
`pins/packages/pi-gpt-0.4.3/src/served.ts` implements exactly that precedence and
ignores `default_model_slug`; `conformance/tier1/gpt-served-model.test.ts`
(SM-001) pins it.

## 2. `gpt-6-astra-wm` is a silent downgrade

*Established.* Requesting `gpt-6-astra-wm` (a `*-wm` "work model") at **every**
thinking effort returns a reply whose `default_model_slug` echoes
`gpt-6-astra-wm` while `resolved_model_slug` / `model_slug` is **`gpt-5-mini`**.
The backend serves gpt-5-mini and reports the requested slug in the one field
that is just an echo. A caller that trusts `default_model_slug` — or that never
reads the served model at all — believes it got a frontier model and got mini.

Consequence: **no intelligence tier, and no default, may map to any `*-wm`
slug.** PR #30 had mapped `extra_high → gpt-6-astra-wm`; that is the regression
this work removes.

## 3. `gpt-5-6-thinking` at `max` is honest

*Established.* Requesting `gpt-5-6-thinking` (GPT-5.6 "Sol") at `max` returns a
reply whose `resolved_model_slug` is `gpt-5-6-thinking` — it serves its own
model. It is the newest **non-pro** thinker confirmed to do so, so it is the
honest choice for the default (`extra_high`) and for the routine `/gpt review`
lane.

Caveat *Established*: a `gpt-5-mini` **fast path** intercepts *trivial* prompts on
any model and answers as mini. A real review prompt (a diff plus instructions)
is not trivial and bypasses it — but the fast path is exactly why reading the
served model on every reply is mandatory, and why any live check must use a
non-trivial prompt.

## 4. Effort labels

*Established.* The account exposes reasoning effort as `min` / `standard` /
`extended` / `max`. `/gpt chat --effort` and the intelligence map use these.

---

## What the code does with this

1. **The map** (`src/models.ts`): `instant → gpt-5-6-instant`,
   `medium/high/extra_high → gpt-5-6-thinking` at `standard/extended/max`,
   `pro → gpt-6-pro`. No `*-wm` slug anywhere; `extra_high` (the default) is
   `gpt-5-6-thinking@max`. Guarded by `gpt-models.test.ts` (MDL-001/002) and the
   package's own `tests/models.test.ts`.
2. **The assertion** (`src/served.ts` + `extensions/gpt-command.ts`): every
   `/gpt` reply's served model is read back from the persistent conversation and
   asserted against the requested model. A mismatch, a `*-wm`/mini downgrade, or
   an unreadable served model is a **loud error with no fallback** (memory:
   gpt-command-no-fallback). Guarded by `gpt-served-model.test.ts`
   (SM-003/004/005).
