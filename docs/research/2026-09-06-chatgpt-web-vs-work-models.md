# ChatGPT "chat" surface vs the "work" surface — one account, two capability sets

**Date of research:** 2026-09-06. Live model/account facts were measured on the
user's real account the same day; web facts were fetched the same day. Model
names and quota numbers in 2026 are past my training cutoff (January 2026), so
every version-sensitive number below is dated and, where possible, grounded on
the account probe rather than memory.

**Repo root verified:** `pwd` and `git rev-parse --show-toplevel` both resolve
to `/Volumes/Data/Developer/commandgovernor`; `origin` is
`git@github.com:DivMode/commandgovernor.git`.

**Method / primary sources.**

- **Account probe (authoritative for slugs and quota):** a read-only Node
  script reproduced `pi-gpt`'s own `gpt_list_models` and `gpt_account_status`
  paths against `https://chatgpt.com/backend-api/...` using the user's
  `~/.codex/auth.json` bearer token — `GET /backend-api/models`,
  `GET /backend-api/me`, `GET /backend-api/accounts/check/v4-2023-04-27`, and
  `POST /backend-api/conversation/init` (the same call `pi-gpt` uses to read the
  Deep Research quota; it starts no chat turn). **No chat message was sent**
  by this probe. The endpoints and headers are those hard-coded in
  `pins/packages/pi-gpt-0.4.3/src/client.ts` and
  `pins/packages/pi-gpt-0.4.3/extensions/chatgpt.ts`.
- **Served-model probes (later the same day, authoritative for what answers):**
  real chat turns sent through `pi-gpt`'s own `ConversationClient` on the same
  token, reading back the leaf assistant message's `resolved_model_slug` /
  `model_slug` from `GET /backend-api/conversation/{id}`. These are the
  measurements in §3a and they **supersede** the list-based inferences in §3
  wherever the two disagree. The companion document
  `2026-09-06-chatgpt-served-model-and-wm-downgrade.md` (ADR 0012) records the
  same findings from the code's side.
- **Vendored code (authoritative for what the chat surface can and cannot do):**
  the committed `pi-gpt@0.4.3` tarball under `pins/packages/`.
- **Prior Command Governor research:**
  `docs/research/2026-09-01-chatgpt-transport-review.md`,
  `docs/research/2026-09-06-chatgpt-web-adapter-evaluation.md`,
  `docs/research/2026-09-04-zero-custom-code-proof.md`.
- **Web (secondary, for OpenAI's plan/limit/API posture):** OpenAI Help Center
  and reputable reporting, cited inline. Every 2026 model name from the open web
  is treated as unverifiable against training data and used only to corroborate
  the account probe.

---

## 1. The answer, in one paragraph

The distinction the product depends on is **real, but it is a distinction of
_surface_, not of model weights.** The same OpenAI account reaches the same
model family two ways. Through a **work surface** — Codex / an Agent-SDK loop
running against the local machine — a model drives a coding agent with real
filesystem and shell tool calls. Through the **chat surface** — the
`chatgpt.com` web backend that `pi-gpt` drives with the Codex OAuth token — the
same model has **no local file or shell access at all**; it can only use
ChatGPT's own server-side tools (web search, canvas, image gen). The correct
architecture therefore has a **work model as the controller** (Fable via the
Claude bridge, or an OpenAI work model via Codex) that **calls the chat surface
as a consultant** — for deep research or for an independent read of finished
work — never the reverse, because the chat surface cannot touch the repository
and so cannot be the thing that does the work.

---

## 2. Capability matrix (measured 2026-09-06)

| Property | **Work surface** (Codex / Agent-SDK loop) | **Chat surface** (`chatgpt.com` backend via `pi-gpt`) |
| --- | --- | --- |
| Credential | Same `~/.codex/auth.json` OAuth token | Same `~/.codex/auth.json` OAuth token |
| Reaches local filesystem | **Yes** — reads/edits the working tree | **No** — "the agent can't read your filesystem directly"; files must be inlined or uploaded as context (`pi-gpt` README) |
| Runs shell / executes code locally | **Yes** | **No** |
| Tool calls | Local harness tools (edit, shell, MCP, subagents) | **Only ChatGPT's own server-side tools** — `enabled_tools` per model is `tools, tools2, search, canvas, image_gen_tool_enabled` (probe); no local-tool channel exists on this path |
| Model family reachable | OpenAI "work" models (and Fable via the bridge) | Everything the account lists — see §3 |
| Deep-research / Pro reasoning model | Not the controller's job | **Yes** — the `deep_research_heavy` lane serves a real Pro model at plan pricing (§3a); chat-lane `gpt-6-pro` is listed but serves mini on this token |
| Metering | Subscription/plan | Normal models effectively unmetered; Deep Research hard-capped — see §4 |
| Role in the architecture | **Controller** — makes all tool calls, owns the working tree | **Consultant** — answers a question, cannot act on the repo |

The chat-surface row "No" answers are not inferred from behaviour; they are what
the vendored transport is built to be. `pi-gpt`'s README states plainly that
"the agent can't read your filesystem directly" and that local files are passed
by inlining text or uploading images/PDFs as context. The 2026-09-06 adapter
evaluation reached the same conclusion independently: "ChatGPT web exposes no
tool calling, so it cannot read the working tree or run anything"
(`docs/research/2026-09-06-chatgpt-web-adapter-evaluation.md` §7), and the
2026-09-01 transport review already recorded the intended topology — "the
harness … makes all tool calls. ChatGPT web is a foreman the user consults"
(§ "recommendation").

---

## 3. The account's real model slugs (probe, 2026-09-06)

Account: `has_active_subscription: true`, `subscription_plan: "chatgptpro"`
(the $200 **ChatGPT Pro** tier), `country: US`, entitlement expires
`2026-10-06`. `GET /backend-api/models` returned 22 models. The ones that matter
for this decision:

| Slug | Title | `reasoning_type` | thinking efforts | `enabled_tools` |
| --- | --- | --- | --- | --- |
| `gpt-6-astra-wm` | GPT-6 Astra | `reasoning` | min, standard, extended, max | tools, tools2, search, **canvas**, image_gen |
| `gpt-6-pro` | GPT-6 Pro | `pro` | standard | tools, tools2, search, image_gen (**no canvas**) |
| `gpt-5-6-instant` | GPT-5.6 Sol | `none` | — | tools, tools2, search, canvas, image_gen |
| `gpt-5-5-pro` | GPT-5.5 Pro | `pro` | standard, extended | tools, tools2, search, image_gen |
| `research` | Deep Research | `none` | — | tools, tools2, image_gen |
| `o3-pro` | o3-pro | `pro` | — | tools, tools2, dalle_3, search |

Also present: `gpt-5-5`, `gpt-5-5-instant`, `gpt-5-6`, `gpt-5-5-thinking`,
`gpt-5-6-thinking`, `gpt-5.5-wm`, `gpt-5.6-sol-wm`, `gpt-5.6-terra-wm`,
`gpt-5.6-luna-wm`, `gpt-5-6-pro`, mini variants.

**Reconciling the user's stated ids against the account:**

- `gpt-6-astra-wm` — **listed, but not served** (§3a). Title "GPT-6 Astra",
  full reasoning-effort range. PR #30 had mapped `medium/high/extra_high` to it
  in `src/models.ts`; the served-model probes showed it answers as `gpt-5-mini`
  at every effort, and ADR 0012 removed that mapping.
- `gpt-6-pro` — **listed, but not served on the chat lane** (§3a).
  `reasoning_type: "pro"`, and notably it is the only tier with `canvas` removed
  from its tool set — a signal that it is a distinct product, not a drop-in
  "normal" model.
- `gpt-5-6-instant` — **confirmed.**
- `gpt-6` (bare) — **not present.** There is no bare `gpt-6` slug; the gpt-6
  family appears only as `gpt-6-astra-wm` and `gpt-6-pro`. The bare-generation
  slugs the account exposes are `gpt-5-6` (title "GPT-5.6 Sol") and `gpt-5-5`.
  The user's "gpt-6" is a near-miss for `gpt-6-astra-wm`.

**On the `-wm` suffix.** Every `-wm` slug (`gpt-5.5-wm`, `gpt-5.6-*-wm`,
`gpt-6-astra-wm`) is `reasoning_type: "reasoning"` with the full min→max effort
range, whereas the bare `gpt-5-6` / `gpt-5-5` are `reasoning_type: "auto"`. The
first draft of this document read that shape as "the explicit work-model-grade
variant for programmatic selection". The served-model probes in §3a **refute
that reading for this credential**: a `-wm` request is accepted and then routed
to `gpt-5-mini`. What `-wm` means to the backend remains undocumented; what it
does on this token is measured.

### 3a. What each slug actually serves (served-model probes, 2026-09-06)

A reply carries three model fields, and one of them is only an echo:
`default_model_slug` repeats the requested slug; `resolved_model_slug` is the
model the backend routed to; `model_slug` is the model that produced the
message. The truthful served model is `resolved_model_slug`, then `model_slug`,
never `default_model_slug`.

| Request (chat lane, this token) | `default_model_slug` | served (`resolved_model_slug` / `model_slug`) |
| --- | --- | --- |
| `gpt-6-astra-wm` at min / standard / extended / max | `gpt-6-astra-wm` | **`gpt-5-mini`** |
| `gpt-6-pro` | `gpt-6-pro` | **`gpt-5-mini`** |
| `gpt-5-6-thinking` at max, non-trivial prompt | `gpt-5-6-thinking` | `gpt-5-6-thinking` |
| any model, trivial prompt | (echo) | `gpt-5-mini` (a fast path intercepts) |

Only the metered `deep_research_heavy` lane (`chat_type: deep_research_heavy`,
`system_hints` research, 5–30 min per request, 250/month) has been observed to
serve a real Pro model (`gpt-5-5-pro`). The same probe of account capabilities
reported `plugins: false`, every `context_connector_*` flag `false`, and only
`browsing: true`.

**The contradiction that is still open.** The user's ChatGPT app, on this same
account, shows Astra Pro and a private-repository GitHub connector and uses
them. So the app's `POST /backend-api/conversation` carries something this
transport does not send (workspace or work-mode context, connector selection,
a gizmo/mode, or different headers). The list probe cannot see that; only a
capture of the app's real request can. ADR 0011 §7 records the method.

Consequences for this document: the "review → `gpt-6-astra-wm`" mapping in the
first draft of §7 and §8 is withdrawn; the review slug is an open pin; and any
consumer of a chat-lane reply must read and assert the served model (ADR 0012).

---

## 4. Metering: what is capped and what is not (probe, 2026-09-06)

`POST /backend-api/conversation/init` returned:

- `limits_progress`: **`deep_research` → remaining 250**, reset `2026-10-06`
  (≈ one month out, i.e. a monthly window); `image_gen` → remaining 1000.
- `blocked_features`: **empty.**
- No other model or feature carries a counter — in particular the normal
  reasoning models (`gpt-6-astra-wm` etc.) have **no** `limits_progress` entry
  and **no** `blocked_features` entry.

Read against the user's three claims:

- **"Non-pro web models are effectively unmetered for a subscriber"** —
  **supported.** The account exposes no counter and no block for the normal
  reasoning families; the only metered features are Deep Research and image gen.
- **"The Pro / deep-research tier has a hard per-window cap"** —
  **supported.** Deep Research is a finite 250-per-month counter shared by both
  `deep_research` and `deep_research_heavy` (the heavy path runs the Pro model);
  when it is exhausted the transport returns `rate_limited` with a reset time
  and does not retry (`pi-gpt` README; `extensions/chatgpt.ts`).
- **"A recent change tightened it because it was being abused (~$150 of value
  on a $200 plan)"** — **not supported by any primary source I could find, and
  partly contradicted.** On this account there is no block and 250 remaining.
  Reporting describes the Pro Deep Research allowance as **250/month and
  increasing over time**, with a lightweight fallback introduced to *manage*
  load rather than a crackdown that *cut* the Pro cap (see Sources). The "$150"
  figure is anecdotal; treat it as the user's own estimate, not a measured fact.
  The architectural conclusion (reserve the finite Deep Research budget for
  genuine deep research) stands on the 250-cap alone and does not need the
  crackdown story.

**Version-sensitivity note.** The vendored `pi-gpt@0.4.3` README still states
"Pro | 40 [deep research] per window". The live account shows **250/month** on
2026-09-06. The README figure is stale; the probe is authoritative. Record the
date with any quota number, because OpenAI moves these.

---

## 5. The claim about API / Responses-API availability

The mission attributes to the `pi-gpt` README the claim that a Pro /
deep-research tier model is "genuinely not available at plan pricing through the
Responses API or Codex." **The README makes no such claim** — it only says the
heavy deep-research path uses the Pro model and that the chat surface can't read
your filesystem. So the attribution is inaccurate; the claim has to be judged on
its own.

Judged on its own, the accurate finding is a **pricing/packaging** distinction,
not an absolute availability one:

- **The capability exists via the API.** OpenAI exposes Deep Research models
  through the Responses API (e.g. `o3-deep-research`, `o4-mini-deep-research`),
  and reporting indicates Pro-tier reasoning models have reached the API as
  well. So "web-only" in the strict sense is **refuted** — the models are not
  locked to the browser. (2026 API dates are past my cutoff; see Sources, and
  treat the specific dates as reported, not verified.)
- **But not at _plan_ pricing.** Responses-API usage is separate, metered,
  pay-as-you-go billing — exactly the "API keys / extra usage" the user has
  ruled out (`MEMORY.md`: "Subscription only, no API keys"). And **Codex**, the
  coding-agent CLI on the subscription, drives *work* models; it does not expose
  `gpt-6-pro` / Deep Research as a callable model for its own agent loop. So
  under the user's own subscription-only constraint, the **only** way to reach
  the Pro / deep-research tier without a new metered bill is the **ChatGPT web
  chat surface** — which is precisely the consultant surface this architecture
  reserves it for.

So the load-bearing statement is: *at plan pricing, and under a subscription-only
policy, the Pro/deep-research consultant is reachable only through the chat
surface.* That is verified. The stronger "not available via the API at all" is
false and the product should not lean on it.

---

## 6. Verified vs. hypothesis

**Verified (probe or vendored code, 2026-09-06):**

1. One credential (`~/.codex/auth.json`) reaches both surfaces.
2. The chat surface has no local filesystem/shell access; local files reach it
   only as inlined text or uploaded images/PDFs.
3. The chat surface's only tools are ChatGPT's server-side tools
   (`tools, tools2, search, canvas, image_gen`).
4. Account is ChatGPT Pro; `gpt-6-astra-wm`, `gpt-6-pro`, `gpt-5-6-instant`
   exist; bare `gpt-6` does not.
5. Deep Research is capped at 250/month (2026-09-06) and shared by light and
   heavy paths; normal reasoning models carry no counter or block.
6. `gpt-6-pro` is a distinct `reasoning_type: "pro"` product with `canvas`
   removed — not a drop-in normal model.
7. `default_model_slug` echoes the request; `resolved_model_slug` / `model_slug`
   report what ran (§3a).
8. On this token the chat lane serves `gpt-5-mini` for `gpt-6-astra-wm` (every
   effort) and for `gpt-6-pro`; `gpt-5-6-thinking@max` serves itself on a
   non-trivial prompt; the `deep_research_heavy` lane serves a real Pro model
   (§3a).
9. The token's capability probe reports `plugins: false` and every
   `context_connector_*` flag `false`, while the user's app on the same account
   shows Astra Pro and a private GitHub connector (§3a).

**Hypothesis / unverified:**

- Which request fields the ChatGPT app sends that unlock real Astra Pro and the
  GitHub connector (workspace/work-mode context, connector selection metadata,
  gizmo/mode, headers). Resolvable only by capturing the app's request.
- Any 2026 API availability date for Pro/deep-research models (reported on the
  open web; past my cutoff).
- A recent abuse-driven tightening of the Pro Deep Research cap, and the "$150
  of value" figure — **no primary source found; partly contradicted.**

**Refuted / corrected:**

- "The `pi-gpt` README claims Pro is unavailable via the Responses API" — the
  README does not.
- "The Pro/deep-research model is strictly web-only" — the capability is
  reachable via the API; what is web-only is *plan-priced* access under a
  subscription-only policy.
- User id "gpt-6" — the account exposes `gpt-6-astra-wm` / `gpt-6-pro`, not a
  bare `gpt-6`.
- "`-wm` marks the work-model-grade variant for programmatic selection" (this
  document's first draft) — on this token a `-wm` request is served by
  `gpt-5-mini` (§3a).
- "Review → `gpt-6-astra-wm` at max effort" (first draft of §7/§8) — withdrawn
  for the same reason; the review slug is an open pin (ADR 0011 §3, §7).

---

## 7. Conclusion — the consultant architecture

The product's dependence is sound once stated as a surface distinction:

- **A work model is always the controller.** Fable via the Claude bridge, or an
  OpenAI work model via Codex, owns the working tree and makes every tool call.
  The chat surface cannot do this and must never be placed where the repository
  work happens.
- **The chat surface is a consultant the controller calls.** Two jobs map to
  two models:
  - **Deep research → the `deep_research_heavy` lane.** It has web browsing
    and citations, it is the only lane observed to serve a real Pro model, it
    is only reachable at plan pricing through this surface, and its budget is
    the finite 250/month — so reserve it for genuine research, not routine
    questions.
  - **Independent review of finished work → a non-Pro chat-lane model that is
    truthfully served** (the slug is an open pin — §3a, ADR 0011 §3/§7; the
    first draft's `gpt-6-astra-wm` is withdrawn because it serves mini).
    Because it is a different model that cannot see the working tree, it gives
    a genuinely independent read of a diff the controller hands it — which
    satisfies the ADR 0008 §4.8 / ADR 0009 §16 "implementer cannot
    self-approve" invariant on capability grounds. Every reply's served model
    is asserted with no fallback (ADR 0012).
- **Never the reverse.** A chat-surface model cannot be the controller: no
  files, no shell, no local tools. Any design that has ChatGPT "drive" and the
  work model "assist" is impossible on this transport, not merely undesirable.

This is a capability judgement, consistent with the user's waiver of the
transport's terms objections (ADR 0008 §8, amended 2026-09-04): the terms are
not what gates the decision; the surface's capabilities are.

---

## 8. Invoking the consultant from Prime (verified against Prime 0.9.2 + pinned packages)

The web models are reachable **only** through `pi-gpt`'s `gpt_chat` tool — they
are not a `/model` provider. `pi-gpt` registers no model provider (no
`registerProvider`/`registerModel` in `extensions/*.ts`; `pins/pins.json`
records it as registering *tools*, and the only provider in the product is
`claude-bridge`, `harness/settings.project.json` `enabledModels:
["claude-bridge/*"]`). So "how do we invoke the consultant" is a real design
question with four mechanisms, and they differ sharply in determinism and token
cost. Prime is pinned at **0.9.2** (`pins/current -> prime-0.9.2`,
`pins.json` `prime-agent.version 0.9.2`).

**1. A slash command runs code directly — no model turn.** `registerCommand(name,
{ handler })` takes `handler: (args, ctx) => Promise<void>`
(`pins/prime-0.9.2/node_modules/prime-agent/dist/core/extensions/types.d.ts:775-825`).
The handler is plain code executed when the user types `/name`; the documented
example just calls `ctx.ui.notify(...)` with no model involved
(`docs/extensions.md:93`), and `pi-gpt`'s own `/gpt-observer` command is a code
handler (`extensions/observer.ts:268`). It consumes a harness-model turn **only
if it explicitly starts one** (`sendUserMessage`/`sendMessage` with
`triggerTurn`, available on `ReplacedSessionContext`); a handler that calls the
`pi-gpt` HTTP client and prints the reply spends **zero** harness-model tokens.
A `/gpt review` handler can even compute the `git diff` itself in code and hand
it to `gpt_chat` deterministically. **Verified.**

**2. A skill or prompt only injects text; the model then acts — always a model
turn.** Skills and prompt templates are prose addressed to the model, not code.
`pi-gpt`'s `chatgpt` skill ("lets you (the agent) interact with a ChatGPT
account"), the `cg-foreman` skill ("the rules below are the product"), and
`harness/prompts/cg-review.md` are all instructions the model reads and decides
to act on. There is no mechanism by which a skill or prompt deterministically
executes a tool; invoking `gpt_chat` from a skill/prompt means the **model**
issues the tool call, which costs a model turn. **Verified.**

**3. A subagent's model can be pinned, but it runs ON a provider model —
never on a web-chat model.** `@gotgenes/pi-subagents@21.4.0` (pinned) defines
agent types in `.pi/agents/<name>.md` with YAML frontmatter that includes a
`model` field (`provider/modelId` or a fuzzy name) and a `thinking` level
(package README §"Agent file format", line 122), and it "automatically
filter[s] to only available/configured models" (README §Features) — an
unresolvable `model` string is rejected (README:307). So a reviewer subagent
*can* be pinned to a work model and call `gpt_chat` as a tool. But it **cannot
run on `gpt-6-astra-wm`**: that model is not in the session's model registry
(no provider registers it), and on this product `enabledModels` is
`claude-bridge/*` only. Command Governor's own roles deliberately inherit the
parent's model (`harness/agents/implementer.md`: "provider choice is the
user's, not the role file's"). A subagent that consults the web model therefore
spends **work-model (Claude/Fable) tokens** on its own reasoning turns, plus the
`gpt_chat` tool call. **Verified.**

**4. `pi-pr-review@1.17.10` uses the session's work models, not any GPT-web
model.** It is "parallel, model-agnostic AI code review" that runs reviewer
passes as subagents on **configured provider models**
(`/pr-review-config light=provider/model heavy=provider/model:high`; package
README §"Configure models"), and "if the extension is unavailable, the prompt
falls back to the current Pi session model" (README). Its shipped source
(`x-prreview/package/{lib,extensions}`) has **zero** references to
`gpt`/`chatgpt`/`codex`/`openai`/`pi-gpt`. So on this product its reviewers run
on `claude-bridge` (Fable/Claude), never on the ChatGPT web surface.
**Verified** against the npm tarball fetched 2026-09-06.

### Ranked by determinism and token cost

| Rank | Mechanism | Deterministic? | Work-model tokens | Notes |
| --- | --- | --- | --- | --- |
| **(a)** | **`/gpt` slash command** whose handler calls `pi-gpt` directly | **Yes** — code runs on the keystroke | **Zero** | The only path that spends no harness tokens. For `/gpt review` the handler builds the diff in code. Caveat: calling the client directly bypasses `pi-gpt`'s in-tool foreman guards, so route through the guarded tool path or replicate the guards (adapter eval §4). |
| (b) | Harness model calls `gpt_chat` when asked in natural language | **No** — the model decides whether/how to call | Costs the harness/work model a turn to orchestrate | The default if nothing is built; fine as a fallback, wrong as the primary. |
| (c) | A reviewer subagent/role that calls `gpt_chat` | Automatic within a flow, not deterministic | Costs a work model (Claude/Fable) the subagent runs on | Right for the *automatic in-loop* review, not for a user's one-shot consult. |

### Recommendation

Ship **(a) `/gpt` with subcommands as the primary** invocation — it is the only
deterministic, zero-work-model-token path, and it matches the user's chosen
shape: `/gpt research` → the `deep_research_heavy` lane; `/gpt review` → the
pinned review model (open pin, §3a) on the `git diff` the handler computes;
`/gpt chat` → model/effort selectable; every reply's served model asserted
(ADR 0012). Build it as a **new file inside the vendored
`pi-gpt`** (so it can import `ConversationClient`/the guarded path by relative
specifier and inherit the foreman guards — a separate `harness/` extension has
no resolvable specifier into `pi-gpt/src`; adapter eval §4). A **skill should
complement, not replace it**: the `chatgpt`/`cg-foreman` skills document the
tool for the *automatic* in-agent path (option b/c), where the work model is
already running and consulting the web model is part of its turn. Do **not**
promote a skill or subagent to the primary user-facing consult path — both cost
work-model tokens that the slash command avoids entirely.

## Sources

- Account probe, read-only, 2026-09-06: `GET /backend-api/models`,
  `/backend-api/me`, `/backend-api/accounts/check/v4-2023-04-27`,
  `POST /backend-api/conversation/init` on `https://chatgpt.com` with the user's
  Codex OAuth token (script preserved with this session; endpoints and headers
  per `pins/packages/pi-gpt-0.4.3/src/client.ts`).
- `pins/packages/pi-gpt-0.4.3/README.md` — "the agent can't read your
  filesystem directly"; deep-research quota table; intelligence levels.
- `pins/packages/pi-gpt-0.4.3/src/models.ts`, `extensions/chatgpt.ts` —
  default model policy and quota/`init` handling.
- `docs/research/2026-09-06-chatgpt-web-adapter-evaluation.md` §1, §7 — "no tool
  calling … cannot read the working tree or run anything."
- `docs/research/2026-09-01-chatgpt-transport-review.md` — "the harness … makes
  all tool calls. ChatGPT web is a foreman the user consults."
- OpenAI Help Center, "Model Release Notes":
  https://help.openai.com/en/articles/9624314-model-release-notes
- OpenAI, "Introducing deep research":
  https://openai.com/index/introducing-deep-research/
- Deep Research via the Responses API (o3-deep-research / o4-mini-deep-research):
  https://pub.towardsai.net/deep-research-with-openais-api-key-ed77ed842774
- ChatGPT Pro Deep Research allowance (250/month; lightweight fallback to manage
  load, not a Pro cut):
  https://www.byteplus.com/en/topic/451490 ;
  https://www.linkedin.com/posts/nicoleleffer_the-number-of-deep-research-credits-you-get-activity-7322750322785857536-uWhD
- ChatGPT web vs app share the same models/tools:
  https://www.cometapi.com/is-the-web-chatgpt-any-different-from-the-app/
- Prime 0.9.2 extension API:
  `pins/prime-0.9.2/node_modules/prime-agent/dist/core/extensions/types.d.ts`
  (`registerCommand`/`RegisteredCommand`, `ExtensionCommandContext`) and
  `pins/prime-0.9.2/node_modules/prime-agent/docs/extensions.md`.
- `@gotgenes/pi-subagents@21.4.0` README (npm tarball, fetched 2026-09-06) —
  agent-file `model`/`thinking` frontmatter; model filtering to configured
  providers.
- `pi-pr-review@1.17.10` README + `lib/`,`extensions/` (npm tarball, fetched
  2026-09-06) — model-agnostic reviewer passes over configured provider models;
  no ChatGPT-web reference.
- `harness/settings.project.json` (`enabledModels: ["claude-bridge/*"]`),
  `harness/agents/*.md`, `harness/skills/*/SKILL.md`,
  `pins/packages/pi-gpt-0.4.3/{skills/chatgpt/SKILL.md,extensions/observer.ts}`,
  `pins/pins.json`.
