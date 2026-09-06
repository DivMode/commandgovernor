# ADR 0012: `/gpt` command, review targets, and the served-model no-fallback rule

- **Status:** Proposed
- **Date:** 2026-09-06
- **Refines:** ADR 0008 (§8, pi-gpt transport), ADR 0010 (composition-first),
  and ADR 0011 (consultant/controller boundary; this ADR is its mechanism)
- **Research:**
  - [`../research/2026-09-06-chatgpt-web-adapter-evaluation.md`](../research/2026-09-06-chatgpt-web-adapter-evaluation.md)
  - [`../research/2026-09-06-chatgpt-served-model-and-wm-downgrade.md`](../research/2026-09-06-chatgpt-served-model-and-wm-downgrade.md)
    (which slug answers; the `*-wm` downgrade)
  - [`../research/2026-09-06-chatgpt-web-vs-work-models.md`](../research/2026-09-06-chatgpt-web-vs-work-models.md)
    §3a (ADR 0011's research; the same measurements from the account side)
- **Scope note (2026-09-06):** the *mechanism* here — deterministic command,
  fresh context, served-model assertion, no `*-wm` tier — is decided. The
  *slugs* in §3 are pins, not decisions: ADR 0011 §3 records the review pin as
  open until the Astra Pro / private GitHub connector investigation (ADR 0011
  §7) lands. The user has not accepted `gpt-5-6-thinking` as the headline
  reviewer; it is the only non-Pro thinker verified to serve itself, so it is
  the interim value of the pin, not its accepted value.

## Context

The user wants code review moved OFF the expensive Fable/Claude harness model
onto the (unmetered) ChatGPT subscription for the common case, escalating to the
capped Astra Pro tier only for hard reviews, and to Claude only rarely. The prior
evaluation (`2026-09-06-chatgpt-web-adapter-evaluation.md`) established that
everything needed already exists in the already-vendored `pi-gpt`: Prime's
`registerCommand` runs a handler as code on the keystroke (no model turn, zero
harness-model tokens), and `pi-gpt`'s `ConversationClient` already drives
`chatgpt.com/backend-api` on the user's Codex login. No browser adapter and no
ChatGPT-web model provider are needed.

Two facts force the design (`2026-09-06-chatgpt-served-model-and-wm-downgrade.md`):

1. A reply's `default_model_slug` merely **echoes** the requested slug; the
   truthful served model is `resolved_model_slug` then `model_slug`.
2. `gpt-6-astra-wm` (mapped by PR #30) silently serves `gpt-5-mini` at every
   effort while echoing the requested slug. Trusting the request, or never
   reading the served model, ships mini in place of a frontier model.

## Decision

### 1. `/gpt` is a deterministic slash command inside `pi-gpt`

It lives in `pins/packages/pi-gpt-0.4.3/extensions/gpt-command.ts`, added by
`pins/patches/pi-gpt-0.4.3-foreman-guards.patch`, because Prime loads the package
in place and a separate `harness/` extension cannot import `pi-gpt`'s client. It
registers one command, `/gpt`, with three subcommands:

- `review [gpt|pro|max] [context]` — gathers the working-tree diff (vs the
  merge-base with the default branch; `git diff HEAD` if no default branch is
  found) and sends it, inlined as text, to a **brand-new** ChatGPT conversation.
- `research <topic>` — cited Pro-tier deep research via `deepResearchHeavy`.
- `chat <prompt> [--model <slug>] [--effort ...]` — ad-hoc one-shot, model and
  effort tab-completed from the account's real model list.

### 2. Fresh context is mandatory for review

Every `/gpt review` starts a completely new persistent conversation seeded only
with (diff + task + review instructions). No `conversation_id` is reused and no
turns are threaded. A session reviewing its own work shares the context that made
the mistake; the value of review is an independent context. The same rule governs
the `max` (Claude) target: it must be a fresh reviewer with clean context, never
the working session's own history.

### 3. Review targets, and the cost/cap distinction (slugs are interim pins)

- `gpt` (default): `gpt-5-6-thinking@max` — **unmetered**, for reviewing
  everything routinely. Interim pin (Scope note above).
- `pro`: `gpt-6-pro` — the **capped** lane; reserve for hard/high-stakes reviews.
  A rate-limited pro tier surfaces the limit and errors — no fallback. Note
  that on the plain chat lane this slug currently serves mini (research §3a),
  so today the assertion in §5 makes `/gpt review pro` error rather than
  silently review on mini. That is the intended behaviour until the real Pro
  request is captured and replicated (ADR 0011 §7).
- `max`: a Claude review **via the harness** — metered, rare.

### 4. `max` is not run in-process (limitation, stated honestly)

A `pi-gpt` command cannot cleanly spawn a fresh, independent Claude reviewer with
guaranteed clean context and verified plan-billed `claude-bridge` routing from
inside its own process: the extension API exposes no "run a fresh reviewer worker
and return its text", and an in-process one-shot on `ctx.model` cannot be verified
here to route through `claude-bridge` without leaking session context or a
credential. Rather than fake it, `/gpt review max` prints the exact fresh-context
harness path (a `prime-agent -p --no-session --provider claude-bridge` reviewer,
or asking the current session) and points here. This is the escape hatch the task
allowed; revisit if Prime exposes a first-class subagent-with-return API.

### 5. Served-model discipline (no silent fallback)

`src/served.ts` reads the served model from the persistent conversation
(`resolved_model_slug` → `model_slug`, never `default_model_slug`) on **every**
`/gpt` reply — `review`, `chat` **and `research`** — and asserts it against what
was requested. A mismatch, a `*-wm`/mini downgrade, or an unreadable served
model is a loud error; the command never retries on a lesser model (memory:
gpt-command-no-fallback). The intelligence map in `src/models.ts` maps no tier
to any `*-wm` slug. Match rules: `review gpt` and an explicitly pinned non-Pro
`chat --model` match exactly; `review pro`, `research`, and a pinned Pro-family
`chat --model` (`*-pro`) match by family prefix, so a dated variant of the
requested Pro slug passes and any other slug fails.

**Known gap, stated rather than papered over.** `/gpt research` asks the
`deep_research_heavy` lane for `gpt-6-pro` and asserts that family on the
reply. The lane has been observed to serve a real Pro model (`gpt-5-5-pro`) for
its vendored default request; whether it serves `gpt-6-pro` when asked for it
has **not** been verified in this lineage, because each verification spends one
of the 250 monthly deep-research requests and 5–30 minutes. If the lane routes a
`gpt-6-pro` request to `gpt-5-5-pro`, the command errors by design and the
research pin must be re-grounded — it must not be widened to "any Pro".

## Consequences

- Reviews and research cost **zero harness-model tokens**; only `max` spends
  Claude tokens, by design.
- The silent astra→mini downgrade cannot reach a user unnoticed: it errors.
- `/gpt` structurally cannot post into the user's foreman thread — it never takes
  or reuses a conversation id, so it always starts fresh.

## Guardrails (tests)

- `conformance/tier1/gpt-served-model.test.ts` — the served-model assertion
  rejects the astra→mini downgrade and a null read, passes on a match, and
  (SM-006/SM-007) rejects through each of its two branches independently: a
  `*-wm`/mini slug that equals the request, and a different non-mini model.
- `conformance/tier1/gpt-models.test.ts` — no intelligence tier maps to a `*-wm`
  slug; `extra_high` resolves to `gpt-5-6-thinking@max`.
- `conformance/runtime/package-load.test.ts` — `/gpt` registers on real Prime.
- `conformance/runtime/live-gpt-command.test.ts` — opt-in (`CG_LIVE=1`): a real
  `gpt-5-6-thinking@max` reply is served by `gpt-5-6-thinking` (non-trivial
  prompt, so the mini fast path does not intercept).

## Status note

This ADR is **Proposed**, not Accepted. It records the design that shipped behind
the `/gpt` command and the model-map fix; accepting it is a separate decision.
