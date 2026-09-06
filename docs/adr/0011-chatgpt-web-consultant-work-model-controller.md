# ADR 0011: ChatGPT web is a consultant surface; a work model is always the controller

- **Status:** Proposed — awaits the user. Do not treat as Accepted until the
  user says so.
- **Date:** 2026-09-06
- **Refines:** ADR 0008 §6–§8 (ChatGPT Web foreman transport), ADR 0009 §16–§17
  (independent review, ChatGPT foreman gate)
- **Research:** [`../research/2026-09-06-chatgpt-web-vs-work-models.md`](../research/2026-09-06-chatgpt-web-vs-work-models.md)

## Context

Command Governor reaches the user's OpenAI account through two different
surfaces, and the product's foreman/consultant design depends on the difference
between them. The distinction was verified against the live account and the
vendored transport on 2026-09-06
(`../research/2026-09-06-chatgpt-web-vs-work-models.md`); this ADR records it so
later work does not re-derive it or place a capability where it cannot exist.

The two surfaces share **one credential** — the Codex OAuth token in
`~/.codex/auth.json` — but expose **different capabilities**:

- **Work surface** — Codex, or an Agent-SDK loop (including Fable via the
  vendored Claude bridge). A model here drives a coding agent: it reads and
  edits the working tree and runs shell/tool calls locally.
- **Chat surface** — the `chatgpt.com` web backend that `pi-gpt` drives with the
  same token. A model here has **no local filesystem or shell access**; local
  files reach it only as inlined text or uploaded images/PDFs, and its only
  tools are ChatGPT's own server-side tools (`tools, tools2, search, canvas,
  image_gen`, per the account probe). It can consult; it cannot act on the repo.

The same model family appears on both surfaces (e.g. `gpt-6-astra-wm` is listed
by the chat backend), so the difference is the **surface**, not the weights. The
chat surface additionally exposes the Pro / deep-research tier (`gpt-6-pro`,
`deep_research_heavy`) that the work surface does not offer as a controller
model.

Two facts about the chat surface's models bound how it should be used
(measured 2026-09-06; version-sensitive, so dated):

- **Normal reasoning models are effectively unmetered** for this ChatGPT Pro
  subscriber — no quota counter, no block.
- **Deep Research is hard-capped** at 250 requests/month (shared by the light
  and heavy paths), returning `rate_limited` with a reset when exhausted.

The user's account is ChatGPT Pro and the user's standing policy is
subscription-only, no API keys, no metered usage (`MEMORY.md`). The Pro /
deep-research tier is reachable through the API only as separate pay-as-you-go
billing, and Codex does not expose it as a callable agent-loop model — so **at
plan pricing, under the subscription-only policy, the Pro/deep-research
consultant is reachable only through the chat surface.** (The stronger folk
claim that these models are "web-only" is false — they exist on the API — and
the product must not depend on it; see the research doc §5.)

The user has waived the transport's terms-of-service objections (ADR 0008 §8
amendment, 2026-09-04). This ADR is therefore a **capability** decision, not a
terms decision.

## Decision

### 1. A work model is always the controller

Every task is owned by a **work-surface** model — Fable via the Claude bridge,
or an OpenAI work model via Codex. The controller owns the working tree and
makes all tool calls. This is a hard constraint, not a preference: the chat
surface has no local file or shell access, so it **cannot** be the controller.

### 2. The chat surface is a consultant the controller calls

The controller calls the `chatgpt.com` chat surface (via the pinned `pi-gpt`
tools) to get an answer, never to perform repository work. A chat-surface model
is handed context explicitly (inlined files, a diff, a question); it returns
text; the controller decides what to do with it.

### 3. Two consultant jobs map to two models

- **Deep research → the web Pro / deep-research tier** (`gpt-6-pro` /
  `deep_research_heavy`). Reserve it for genuine web research with citations,
  because it is the only surface offering it at plan pricing and its budget is
  the finite 250/month. Do not spend it on routine questions; when it is
  exhausted, do not retry before the reported reset.
- **Independent review of finished work → the non-Pro web reasoning model**
  (`gpt-6-astra-wm` at max effort — `pi-gpt`'s `extra_high` default). It is
  effectively unmetered, and being a different model that cannot see the working
  tree, it delivers a genuinely independent read of a diff the controller hands
  it.

### 4. Never invert controller and consultant

No design may make a chat-surface model "drive" while a work model "assists".
That topology is impossible on this transport (no files, no shell, no local
tools), not merely disfavoured. Any proposal that assumes it is rejected at the
architecture stage.

### 4a. The consultant is invoked by a slash command, not a skill or subagent

The web models are reachable only through `pi-gpt`'s `gpt_chat` tool, not as a
`/model` provider (`pi-gpt` registers no provider; `enabledModels` is
`claude-bridge/*`). Of the four ways Prime 0.9.2 can drive that tool, only a
**slash command handler runs code deterministically and spends zero
work-model tokens** — verified against the pinned substrate and packages
(`../research/2026-09-06-chatgpt-web-vs-work-models.md` §8):

- **slash command** (`registerCommand`, `types.d.ts:775-825`) — code on the
  keystroke, zero model turn unless it explicitly starts one;
- **skill / prompt** — prose the model reads and acts on; always a model turn,
  cannot call a tool deterministically;
- **subagent** (`@gotgenes/pi-subagents`) — model is pinnable in agent-file
  frontmatter but must be a *configured provider* model (so Claude/Fable here,
  never a web-chat model); costs work-model tokens;
- **the harness model deciding to call `gpt_chat`** — non-deterministic, costs
  the work model a turn.

Therefore Command Governor ships **`/gpt` with subcommands as the primary
consult path** (the user's chosen shape): `/gpt research` → web Pro
(`gpt-6-pro` / `deep_research_heavy`); `/gpt review` → `gpt-6-astra-wm` on the
`git diff` the handler computes in code; `/gpt chat` → model/effort selectable.
It is built as a **new file inside the vendored `pi-gpt`**, so it can reach the
guarded tool path by relative import and inherit `pi-gpt`'s foreman guards
rather than bypass them (adapter evaluation §4). A **skill complements but does
not replace it**: the `chatgpt` / `cg-foreman` skills document the tool for the
*automatic in-agent* path, where the work model is already running and the
consultation is part of its turn. A skill or subagent is never promoted to the
primary user consult path, because both spend work-model tokens the slash
command avoids.

`pi-pr-review` (pinned) is unaffected: it runs its reviewers on configured
provider models (Claude/Fable here) and calls no ChatGPT-web model — verified
from its source.

### 5. This is how the review invariant is met on capability grounds

ADR 0008 §4.8 and ADR 0009 §16 require that an implementer cannot satisfy its
own independent review. Because the review consultant is a **different model on a
surface that cannot touch the repository**, it cannot be the implementer
reviewing itself. This ADR records that the separation is enforced by the
surface boundary, consistent with the acceptance record in ADR 0009 (the
foreman's correlated ChatGPT reply as the acceptance record).

### 6. Model slugs and quotas are pins, dated and probe-grounded

The default model policy lives in the vendored `pi-gpt` patch
(`extra_high → gpt-6-astra-wm@max`, `pro → gpt-6-pro`,
`instant → gpt-5-6-instant`; `pins/pins.json`). Slugs and quota numbers are
version-sensitive and must be re-grounded on the account (`gpt_list_models` /
`gpt_account_status`) rather than assumed. As measured 2026-09-06 the account is
ChatGPT Pro; `gpt-6-astra-wm` and `gpt-6-pro` exist; a bare `gpt-6` slug does
not; Deep Research is 250/month. The `pi-gpt@0.4.3` README's "40 per window"
figure is stale.

## Relationship to prior ADRs

- **ADR 0008 §6–§8** established the ChatGPT Web foreman closed loop and the
  capability-gated transport (terms waived in the §8 amendment). This ADR adds
  the capability boundary underneath it: which side may act, and which may only
  advise.
- **ADR 0009 §16–§17** kept independent review and the ChatGPT foreman as a
  separate gate. This ADR records that the review separation is enforced by the
  surface boundary, not merely by role assignment.
- No prior decision is superseded; this ADR sharpens them.

## Consequences

### Positive

- Removes an entire class of impossible designs (chat surface as controller)
  before they are built.
- Assigns the finite Deep Research budget deliberately, protecting it from
  routine use.
- Gives the "implementer cannot self-approve" invariant a concrete,
  capability-based enforcement point.
- Keeps the decision on capability, consistent with the terms waiver.

### Costs / risks

- The consultant surface can change without notice (undocumented backend); slugs
  and quotas must be re-probed, not trusted from memory or a stale README.
- Deep Research is capped; heavy reliance on it will hit the 250/month wall.
- Depends on the single Codex OAuth credential for both surfaces; if that login
  breaks, both the controller (Codex path) and the consultant break together.

## Alternatives considered

### Let the chat surface act on the repository

Rejected — impossible on this transport. It has no local file or shell access.

### Use the API / Responses API for the Pro / deep-research model

Rejected under current policy. It is separate metered billing, which the user
has ruled out (subscription-only, no API keys). Revisit only if that policy
changes.

### Treat all chat-surface models identically

Rejected. The unmetered reasoning model and the finite-budget Pro/deep-research
tier have different costs and different jobs; conflating them either wastes the
Deep Research budget or under-uses the free review capacity.

## Acceptance

Move this ADR from **Proposed** to **Accepted** only when the user confirms the
consultant/controller split and the two-job mapping. Until then it records the
verified capability boundary and the recommended use, and awaits the user.
