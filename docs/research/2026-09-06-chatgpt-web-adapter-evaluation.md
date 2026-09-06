# A direct `/gpt` chat path to ChatGPT web inside Prime — and why the browser adapter is not needed — 2026-09-06

Status: **research, with executed measurements.** Every claim marked *Verified* was
read from a primary source on 2026-09-06 — Prime 0.9.1's shipped `dist/` type
declarations and implementation under `pins/prime-0.9.1`, the vendored
`pins/packages/pi-gpt-0.4.3` source, the `@minzicat/pi-chatgpt-web-adapter@0.1.1` npm
tarball, or a live GitHub/npm registry response. Two things were *run*: a Prime
extension load test (§6) and a runtime export probe (§2.4). Claims marked
*Hypothesis* have not been measured.

**No code was written.** This is an evaluation and a recommendation. No tracked file
in the repository was modified by this work; this document is the only addition.

**Pin note (the pin is moving under this document).** The measurements in §6 were run
on `pins/prime-0.9.1`. While this was being written, concurrent work in the repository
added `pins/prime-0.9.2` and extended
`pins/patches/pi-gpt-0.4.3-foreman-guards.patch` by ~133 lines. **Every Prime API this
recommendation depends on was re-checked against 0.9.2 and is identical**, at the same
declaration sites: `registerCommand` (`:825`), the
`handler: (args: string, ctx) => Promise<void>` signature (`:780`),
`getArgumentCompletions` (`:779`), `sendMessage` (`:842`), `appendEntry` (`:854`),
`registerProvider` (`:929`), and the `custom_message` branch that puts custom messages
into the model's context (`dist/core/session-manager.js:235`). The §4 recommendation
targets that patch file, so it must be rebased onto whatever the concurrent work
lands.

Architecture assumed, per the re-scope: **Fable via the Claude bridge remains the
harness that makes all tool calls. ChatGPT web is a foreman the user consults.** What
is being designed here is a direct chat path to that foreman from inside Prime that
spends no harness-model tokens and uses no browser.

---

## The answer

**Build `/gpt` as a small command inside the already-vendored `pi-gpt`. Do not pin the
browser adapter, and do not build a ChatGPT-web model provider.**

Everything the feature needs already exists and is already on this machine:

- *Verified.* **Prime can register the command and stream into the TUI.**
  `registerCommand` takes a free-form argument string plus
  `getArgumentCompletions`, so `/gpt` can offer tab-completion for models and
  efforts. Streaming output goes through `ctx.ui` (§2).
- *Verified.* **Prime has an exact primitive for "not visible to the harness model
  unless asked."** `pi.appendEntry(customType, data)` is documented *"Append a custom
  entry to the session for state persistence (**not sent to LLM**)"*, and the session
  code confirms the distinction structurally: custom **messages** are pushed into the
  model's message array, custom **entries** are not (§2.3). This is the requirement's
  load-bearing API, and it is a documented public surface, not a trick.
- *Verified.* **`pi-gpt` already does the send, the stream, the async poll, the read,
  the model selection and the thinking effort** — browserlessly, over the Codex OAuth
  token, with `conversation_id` continuity (§3).
- *Verified.* **Model and effort can be enumerated from the account**, not hardcoded:
  `GET /backend-api/models` returns each model's `slug` plus its own
  `thinking_efforts` list (§3.2).

The estimate is **≈180–300 lines**, added to the vendored `pi-gpt` as a new file plus
one manifest line (§5).

**Where it must live: inside the vendored `pi-gpt`, not as a new extension under
`harness/`.** The reason is concrete, not stylistic. *Verified:* Prime loads a
local-path package **in place** — `parsed.type === "local"` resolves to
`existsSync(path) ? path : undefined` with no copy and no `node_modules` entry
(`dist/core/package-manager.js:637`) — and `pi-gpt` publishes **no `exports` map**,
with `main` pointing at a `.ts` file and internals imported by relative path. So a
separate `harness/` extension has **no resolvable specifier** for
`pi-gpt/src/conversation.ts`; it could only reach in by a repo-relative filesystem
path, which breaks the moment `harness/` is consumed by another project — a case
`harness/settings.project.json` explicitly supports. There is also a second reason
in §4: the existing foreman guards live in the *tool* path, and a command that calls
the client directly would silently bypass them.

**The browser adapter is not needed and should be rejected** (§6). It cannot hold a
ChatGPT-side conversation, cannot make tool calls, needs a third credential and a
permanent headful Chrome — and, decisively, **its declared source repository does not
exist**.

---

## 1. What is being built, in one paragraph

A slash command, `/gpt`, that sends the user's text straight to ChatGPT web over
`pi-gpt`'s existing HTTP client on the Codex OAuth token; lets the user choose the
model (`gpt-5-5-pro`, `gpt-5-5-thinking`, `gpt-5-5-instant`, or whatever the account
exposes) and the thinking effort (`min`/`standard`/`extended`/`max`, per the
account's own per-model list) either as arguments or as a sticky per-session setting;
keeps one ChatGPT `conversation_id` per Prime session so replies continue the same
thread; renders the reply in the TUI as it streams; and keeps the exchange **out of
the harness model's context** unless the user explicitly asks to share it. It spends
no Fable tokens, because Fable is never invoked.

---

## 2. (a) Can Prime register such a command and print streamed output?

**Yes, on documented public APIs.** All *Verified* from Prime 0.9.1's shipped types
and implementation.

### 2.1 Command registration and arguments

`ExtensionAPI` (`dist/core/extensions/types.d.ts:825`):

```ts
registerCommand(name: string, options: Omit<RegisteredCommand, "name" | "sourceInfo">): void;
```

and (`:775`):

```ts
interface RegisteredCommand {
  name: string;
  description?: string;
  getArgumentCompletions?: (argumentPrefix: string)
      => AutocompleteItem[] | null | Promise<AutocompleteItem[] | null>;
  handler: (args: string, ctx: ExtensionCommandContext) => Promise<void>;
}
```

Two things matter here:

- `args` is a **single free-form string**, so `/gpt --model pro --effort max <text>`
  is parsed by us. Fine, and it keeps the sticky-setting design open.
- `getArgumentCompletions` gives the user **tab-completion over the account's real
  model and effort names**, with `AutocompleteItem = {value, label, description?,
  argumentHint?, sourceTag?, takesArgument?}` (`@earendil-works/pi-tui/dist/
  autocomplete.d.ts`). This is what makes model/effort selection feel like `/model`
  rather than like remembering flags — and it is free.

### 2.2 Printing streamed output

The handler returns `Promise<void>`, so **all output goes through the context**, not
a return value. `ExtensionContext` (`:~250`) provides `ui: ExtensionUIContext` with,
among others:

| API | Use for `/gpt` |
| --- | --- |
| `ui.setWidget(key, string[] \| factory, {placement})` | the live reply pane; call repeatedly as chunks arrive |
| `ui.setStatus(key, text)` | footer state, e.g. the sticky model/effort |
| `ui.setWorkingMessage(msg)` / `setWorkingVisible` / `setWorkingIndicator` | "GPT-5.5 Pro thinking…" during long Pro turns |
| `ui.notify(message, type)` | errors, rate limits |
| `ui.custom<T>(factory)` | a focusable pane if the reply deserves one |
| `pi.registerMessageRenderer(customType, renderer)` | custom rendering when an exchange *is* promoted into the transcript |

`ctx.signal` and `ctx.abort()` are exposed, so Esc-to-cancel maps onto the
`AbortSignal` that `pi-gpt`'s client already accepts end-to-end (§3.1).

**Caveat, verified:** `ctx.hasUI` is `false` in print/RPC mode. The command needs a
plain-text fallback path, or it silently produces nothing in `--print`.

### 2.3 Keeping the exchange away from the harness model — the decisive API

This is the requirement most likely to be got wrong, and Prime draws the line
structurally rather than by convention.

*Verified.* `ExtensionAPI` offers both of these (`:842`, `:855`):

```ts
/** Send a custom message to the session. */
sendMessage<T>(message: Pick<CustomMessage<T>, "customType"|"content"|"display"|"details">,
               options?: { triggerTurn?: boolean; deliverAs?: "steer"|"followUp"|"nextTurn" }): void;

/** Append a custom entry to the session for state persistence (not sent to LLM). */
appendEntry<T>(customType: string, data?: T): void;
```

And the difference is real in the session layer, not just in the doc comment. When
Prime builds the message list handed to the model, it walks session entries and
pushes `type: "message"` and `type: "custom_message"` entries into `messages`
(`dist/core/session-manager.js:225-240`) — **custom entries written by `appendEntry`
are a different entry type and are not in that path.**

So the two-tier design the user asked for maps exactly onto two documented calls:

- **Default — private to the user.** Render with `ui.setWidget` / `ui.custom`, and
  persist the exchange with `appendEntry("gpt-exchange", {...})`. It survives in the
  session file, it is durable across a restart, and the harness model never sees it.
- **On request — "show that to Claude."** Promote the stored exchange with
  `sendMessage({customType, content, display: true}, {triggerTurn: false})`, which
  *does* enter the model's context, or `sendUserMessage(...)` to also trigger a turn.

Because the exchange is already persisted by `appendEntry`, the "share it" step can
happen **later** — the user can decide after reading the reply, which is precisely
the requested behaviour.

### 2.4 Prime-vs-Pi seams

One real seam, one that looked real and is not.

**Real — command handlers return nothing in Prime.** The browser adapter's own Pi
extension registers a command whose handler returns `{ type: "text", text }` and
casts it `as never`, and it guards with `typeof pi.registerCommand !== "function"`.
Prime's signature is `=> Promise<void>`. *Hypothesis:* upstream Pi's command handler
returns renderable content and Prime's does not. Either way the rule for us is
concrete and verified on the Prime side: **a `/gpt` handler must write through
`ctx.ui` / `pi.sendMessage` / `pi.appendEntry`; a returned value is ignored.**

**Not a seam — `StringEnum`.** `pi-gpt` imports `StringEnum` from
`@earendil-works/pi-ai`, and it is absent from that package's `dist/index.d.ts`,
which looks like the familiar `pi-ai` export seam. It is not. Probed directly on the
pinned Prime:

```
$ node --input-type=module -e 'import * as ai from "@earendil-works/pi-ai";
                               console.log(typeof ai.StringEnum, typeof ai.getModels)'
function function
```

Both resolve at runtime. Recording this because the type-declaration grep gives a
false positive, and a future reader will otherwise "fix" a seam that does not exist.

None of the four seams catalogued in
`pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch` (`pi-ai/compat` import,
`CONFIG_DIR_NAME`, registry method name, `cacheRetention`) applies to a command added
inside `pi-gpt`: the package already loads on Prime, and the new code would sit in a
file that uses the same imports the existing extension already uses successfully.

---

## 3. (b) What `pi-gpt` already does

*Verified* from `pins/packages/pi-gpt-0.4.3/src/`.

### 3.1 Send, stream, poll, read

`src/conversation.ts:603` — a plain `fetch` + SSE generator, **no browser anywhere**:

```ts
async *stream(
  model: string,
  messages: ChatMessage[],
  opts: { gizmoId?; temporary?; thinkingEffort?; conversationId?;
          parentMessageId?; attachments?; signal? } = {},
): AsyncGenerator<string | ConvIdSentinel>
```

and `:716` `complete(model, messages, opts)` wrapping it.

| Requirement | Function | Notes |
| --- | --- | --- |
| Send to a conversation | `stream()` / `complete()` | `buildPayload` sets `conversation_id` (`:109`) and `parent_message_id` (`:99`) |
| Stream chunks for the TUI | `stream()` | yields text as it arrives — maps straight onto repeated `ui.setWidget` |
| Long/Pro turns | `pollAsyncResponse()` (`:752`) | extended/max/Pro return **asynchronously**: the SSE closes with zero text frames and the answer lands later. Already handled, with an **ancestry guard** (`requiredAncestorIds`) so a stale answer cannot be picked up |
| Continue the same thread | `leafMessageId(conversationId)` (`:594`) | resolves `current_node` for the next turn's `parent_message_id` |
| Read a thread | `BackendClient.get('/backend-api/conversation/<id>')` | used by `gpt_get_conversation` |
| Cancel | `signal?: AbortSignal` | threaded through fetch **and** the poll loop |
| Model | `model: string` | free-form slug, per turn |
| Effort | `opts.thinkingEffort` | → `payload.thinking_effort`, per turn |

`ChatMessage` is `{id?, role, content}` — already the shape a command would build.

The credential is the **Codex OAuth token** from `~/.codex/auth.json`
(`src/auth.ts:21`). Subscription-only; no API key anywhere in this path.

### 3.2 Model and effort selection, from the account

*Verified.* `gpt_list_models` (`extensions/chatgpt.ts`) calls

```
GET /backend-api/models?history_and_training_disabled=false
```

and maps each entry to `{slug, title, reasoning_type, thinking_efforts[], tags,
enabled_tools}`, where `thinking_efforts` is that model's **own** effort list. The
helper already exists in `src/models.ts`:

```ts
export function effortsForModel(slug: string, models: any[]): string[]
```

`src/models.ts` also already names the family — `gpt-5-5-instant`,
`gpt-5-5-thinking` (`standard` / `extended` / `max`), `gpt-5-5-pro`.

So `/gpt`'s `getArgumentCompletions` can offer **the account's real models, and for a
chosen model only the efforts that model actually supports**, discovered at runtime.
Nothing is hardcoded, and a model OpenAI adds later appears with no code change.
This is strictly better than the browser adapter's three frozen ids.

**Switching model or effort mid-thread.** *Verified structurally; not live-measured.*
`buildPayload` puts `model` and `thinking_effort` in the **per-turn body**, while
`conversation_id` and `parent_message_id` are independent per-turn options
(`src/conversation.ts:99,108,109`). Nothing binds a model to a conversation, which
matches chatgpt.com's own behaviour, where the model picker changes the next turn
without starting a new chat. *Hypothesis until measured:* that `gpt-5-5-pro`
specifically accepts a `conversation_id` created by a different model. **Test this
first** — if Pro requires a fresh thread, the picker still works but continuity
across a switch into Pro does not, and `/gpt` should say so rather than silently
starting a new thread.

### 3.3 Two things to get right

- **The `temporary` default.** `buildPayload` defaults `temporary = true` →
  `history_and_training_disabled: true`. For a real, resumable, user-visible ChatGPT
  thread `/gpt` must pass **`temporary: false`** explicitly. Easy to miss; it
  silently decides whether the conversation exists in ChatGPT history at all.
- **The system prompt / preamble.** Whether the web backend honours an
  `author.role: "system"` message is unverified. The browser adapter's author
  evidently concluded it does not — it **prepends** the system text to the user
  message. Prefixing is the safe choice, and on a persistent thread it need only be
  sent on the first turn. `/gpt` should send little or no preamble anyway: the user
  is talking to their foreman, not configuring an agent.

---

## 4. (c) Where the command should live

Classified under ADR 0010 §2, this is **PLUGIN** — "genuinely Command-Governor-
specific and belongs in the smallest practical Prime/Pi extension/package". It is not
USE EXISTING (nothing provides it), not TEMP WORKAROUND (no upstream defect is being
worked around), and not DELETE.

The question is *which* surface is smallest. Two candidates, and the evidence points
one way.

### Option A — a separate tiny extension under `harness/`

**Blocked by module resolution, verified.** Three facts compose badly:

1. Prime loads a local-path package **in place**: `parsed.type === "local"` →
   `resolvePathFromBase(...)` → `existsSync(path) ? path : undefined`
   (`dist/core/package-manager.js:637`). There is **no copy** and **no
   `node_modules/pi-gpt` entry** created. `getNpmInstallPath` handles only npm and
   git sources.
2. `pi-gpt` has **no `exports` map**; `main` is `extensions/chatgpt.ts`, and its own
   code imports internals by relative path (`../src/client.ts`,
   `../src/conversation.ts`).
3. Therefore a `harness/` extension has no bare specifier to import
   `ConversationClient` from. It would have to hardcode a **repo-relative filesystem
   path** into `pins/packages/pi-gpt-0.4.3/src/` — which exists only in this
   checkout, and breaks for a consuming project, a case
   `harness/settings.project.json` explicitly supports ("another project installs it
   with `prime-agent package install --local <repo>/pins/packages/pi-gpt-0.4.3`").

There is a second, softer objection: `harness/package.json` declares
`"pi": { "extensions": [] }` and describes itself as *"skills, prompts, roles and
project configuration … Contains no extensions and no runtime code"*, echoed in
`harness/README.md`. Adding runtime code there is a deliberate architectural
threshold — defensible if it were the right home, but it is not, per (1)–(3).

### Option B — extend the vendored `pi-gpt` patch

`pins/patches/pi-gpt-0.4.3-foreman-guards.patch` is 160 lines already spanning **five
files** (`README.md`, `extensions/chatgpt.ts`, `skills/chatgpt/SKILL.md`,
`src/models.ts`, `tests/models.test.ts`). Adding `/gpt` here means the code sits next
to the client it uses, with the same relative imports the package already uses, and
inside the package that already owns the credential and already loads on Prime.

**And there is a correctness argument that settles it.** *Verified:* the existing
foreman guards live in `gpt_chat`'s `execute`, not in `ConversationClient`:

```
+        parentId = (await conv.leafMessageId(p.conversation_id)) || undefined;
+        if (!parentId) throw new Error(`gpt_chat: could not read the current leaf of conversation ${p.conversation_id}; not sending`);
...
+      if (p.conversation_id && result.conversationId && result.conversationId !== p.conversation_id) {
+        throw new Error(`gpt_chat: requested conversation ${p.conversation_id} but the backend answered from ${result.conversationId}; ...`);
+      }
```

These are the R1/R2 protections — **no fabricated `parent_message_id`**, and
**assert the answering thread is the requested thread**. A `/gpt` command that calls
`conv.complete()` directly would **bypass both**. That is a real hazard (§7), and it
is far easier to share one guard implementation inside the package than to reimplement
it across a package boundary.

### Recommendation for (c)

**Option B, structured to minimise upgrade pain: add a *new file* rather than
editing existing ones.** Concretely, the patch grows by one new
`extensions/gpt-command.ts` plus a one-line addition to `pi.extensions` in
`package.json` (and a README/SKILL note). New files do not conflict on a vendor
refresh the way edited hunks do, so the standing cost of carrying this across a
`pi-gpt` upgrade stays close to zero.

The guards should be **lifted into a shared helper** used by both `gpt_chat` and
`/gpt`, rather than duplicated — otherwise the two lanes will drift, and the lane
that drifts is the one that posts into the foreman thread.

Per ADR 0010 §18, if any part of this turns out to be generic to Pi rather than
specific to Command Governor, it should be offered upstream to `pi-gpt`; the local
patch is the bridge, not the destination.

---

## 5. (d) Size estimate

New code in the vendored `pi-gpt`, reusing `BackendClient`, `ConversationClient`,
`models.ts` and the token modules unchanged:

| Component | Lines (est.) |
| --- | --- |
| `registerCommand` + argument parsing (`--model`, `--effort`, sticky defaults) | 40–60 |
| `getArgumentCompletions` over the account model/effort list (uses `effortsForModel`) | 25–40 |
| Streaming render loop into `ui.setWidget` + working indicator + abort | 45–70 |
| Per-session conversation id: `appendEntry` persist, `leafMessageId` continuation | 35–55 |
| Share-on-request path (`sendMessage` promotion of a stored exchange) | 20–35 |
| Errors, 401/403 re-auth hint, rate-limit surfacing, `hasUI:false` fallback | 25–40 |
| **Total** | **≈ 190–300** |

Plus a modest refactor to share the two foreman guards (§4), which mostly moves
existing lines rather than adding them.

For scale: `pi-gpt`'s `src/` is 3,022 lines, of which the 1,417-line
`conversation.ts` and 445 lines of token handling are reused for free. This is a
command on top of a working client, not a reimplementation.

---

## 6. (e) Interaction with the foreman-thread lane — the safety rule

`/gpt` and the foreman transport share one credential, one HTTP client and one
account. They must not share a conversation.

*Verified context.* `pi-gpt` keeps a per-project chat registry
(`src/registry.ts`: `addChat`, `getChat`, `listChats`, `updateChat` over
`ChatRecord`), and ADR 0008 §8 records `pi-gpt` as the `foreman-transport` owner,
with the live send-and-correlate evidence in
`docs/research/2026-09-04-zero-custom-code-proof.md` §6. The foreman thread is a
specific `conversation_id`, and the guards in §4 exist to keep sends inside it.

**The rule, stated so it can be tested:**

1. **`/gpt` owns its own conversation id, per Prime session**, persisted with
   `appendEntry` (§2.3). It is created on first use and is **never** seeded from the
   registry's foreman record.
2. **`/gpt` never defaults to the foreman conversation.** No "most recent chat"
   fallback, no `listChats()[0]`. Absent a stored id for this session, it starts a
   new thread.
3. **It may target the foreman thread only when the user names that conversation id
   explicitly** in the command. Anything less specific — "the foreman", "the usual
   thread" — is refused.
4. **When it does target a named id, it must go through the same guards** as
   `gpt_chat`: fail rather than fabricate a `parent_message_id`, and assert the
   answering `conversationId` equals the requested one. This is the concrete reason
   §4 recommends sharing the guard code instead of duplicating it.
5. **The foreman lane's correlation protocol is not `/gpt`'s to imitate.** The
   transport review's delivery-id-in-body protocol
   (`docs/research/2026-09-01-chatgpt-transport-review.md` §3, R2, including the
   constraint that ids must contain letters because `redact()` destroys long digit
   runs) belongs to the foreman lane. A casual `/gpt` message into the foreman thread
   without a delivery id would appear in that thread as an uncorrelated human turn,
   and could be mistaken for a foreman reply on readback. **This is the strongest
   practical reason to keep the lanes apart.**

*Hypothesis worth a live check:* whether a `/gpt` turn posted into the foreman thread
would move `current_node` in a way that disturbs an in-flight foreman correlation.
Given rule 2 this should never arise, but if rule 3 is ever exercised it is the
failure mode to watch.

---

## 7. The browser adapter, briefly: evaluated, and not needed

I evaluated `@minzicat/pi-chatgpt-web-adapter@0.1.1` in full, including a load test
on the pinned Prime. Summary, since it is no longer the recommended path.

**It does load — that is not the problem.** *Verified by execution* in an isolated
agent dir on `pins/prime-0.9.1`:

```
$ prime-agent package install npm:@minzicat/pi-chatgpt-web-adapter
added 85 packages in 4s
$ prime-agent model list
provider     model             context  max-out  thinking  images
chatgpt-web  gpt-5-5           272K     128K     no        no
chatgpt-web  gpt-5-5-pro       272K     128K     yes       no
chatgpt-web  gpt-5-5-thinking  272K     128K     yes       no
```

**Zero patches needed** — none of the four known Prime seams fires, because its only
Pi import is a type and it carries its own credential. On pure Prime-compatibility
grounds it is a better composition candidate than the bridge. It is rejected anyway:

| Finding | Verdict |
| --- | --- |
| **Declared source repository does not exist** — `gh api repos/minzique/dotfiles-agents` → **404**, same for its `bugs` URL; sourcemaps ship `sourcesContent: false`; none of the author's 71 public repos mirrors it | **Decisive.** An unauditable binary artifact holding a ChatGPT credential |
| **No tool calling** — `dist/translate.js` has zero `tools`/`tool_call`/`function_call` | Chat only, in any case |
| **No ChatGPT-side conversation** — every turn sends `parent_message_id: "client-created-root"` and never `conversation_id` (`dist/chat/conversation.js:39,112`); a **new thread per turn** | Fails the user's continuity requirement outright |
| **Needs its own browser login** — zero references to `~/.codex/auth.json`; its `doctor` reported `auth: ✗ not logged in`; `auth login` opens a visible Chrome | A **third** credential, and a permanent headful Chrome per `docs/ARCHITECTURE.md` |
| One release (2026-06-18), sole maintainer, no reachable issue tracker, 85 transitive packages | Unmaintainable as a pin |
| No usage/quota reporting — `usage` hardcoded to zeros; `MAX_TURNS_PER_HOUR` marked `TODO(v0.2)` and referenced nowhere | — |

**The live exchange in the original brief was not run**, because the brief said to
stop rather than log in on the user's behalf, and the login requirement was confirmed
both in code and at runtime. So its reply quality, latency and quota effect remain
unmeasured — and are now moot.

**One useful thing it taught us.** Its browser does three jobs — login, security-token
minting, and the conversation call itself (`page.evaluate` at
`dist/chat/conversation.js:129,140,193`). But the third is incidental:
`docs/ARCHITECTURE.md` gives the reason as cookie fidelity, not any DOM dependency,
and its protocol code (`sse-reassembler.js`, `translate.js`) is pure Node. **The
browser is load-bearing only for minting** — which `pi-gpt` already does in Node, in
`src/sentinel.ts`, `src/pow.ts` and `src/turnstile.ts`. Both packages drive the same
undocumented backend endpoint, `chatgpt.com/backend-api/f/conversation`, and both
carry modules addressing the provider's anti-automation controls; **their existence is
noted here, and how they work is out of scope.** Per ADR 0008 §8 (amended 2026-09-04)
the terms objection is waived, so this is judged on capability, safety and maintenance
only.

That split is exactly why `/gpt` on `pi-gpt` needs no browser.

**Also not recommended: a ChatGPT-web *model provider*.** Prime's `registerProvider`
would accept one (`streamSimple` + `api`, or a local OpenAI-compatible endpoint), and
`pi-gpt`'s client could drive it in ~250–400 lines. But under the assumed
architecture Fable is the harness and makes all tool calls, so a ChatGPT model
*inside* the agent loop would be a model that cannot use tools — a trap disguised as
a normal `/model` switch. A command is the honest shape for a consultation.

---

## 8. Recommendation

**Build `/gpt` as a new file in the vendored `pi-gpt`, extending
`pins/patches/pi-gpt-0.4.3-foreman-guards.patch`. Reject the browser adapter. Do not
build a ChatGPT-web model provider.**

**Single strongest reason:** every capability the feature needs already exists on
this machine and is already pinned — `pi-gpt`'s browserless client on the Codex
subscription token for the send/stream/poll/continue, and Prime's documented
`registerCommand` + `ui.*` + `appendEntry` for the command, the streamed rendering,
and the "invisible to the harness model unless asked" guarantee. The remaining work
is glue measured in the low hundreds of lines, and it adds **no browser, no daemon,
no port, and no third credential**.

**Trade-offs, stated honestly:**

- *We take on the token-rotation liability.* `pi-gpt` computes the security tokens in
  Node, so a provider-side rotation breaks `/gpt` and we own the fix; the adapter's
  live-page minting would have healed itself. Mitigating evidence: this path was
  **measured working live on the user's real account on 2026-09-04**
  (`docs/research/2026-09-04-zero-custom-code-proof.md` §6), and the risk is already
  accepted for the foreman transport, so `/gpt` adds no *new* exposure. `pi-oracle`
  remains the browser-backed fallback per ADR 0008 §8.
- *The vendored patch grows.* From 160 lines across five files to that plus a new
  file. Adding a file rather than editing hunks keeps upgrade conflicts near zero,
  but the patch is no longer purely defensive — it now carries a feature. If `pi-gpt`
  upstream would take it, ADR 0010 §18 says offer it.
- *Two lanes now share one account.* `/gpt` and the foreman transport use the same
  credential and client. §6 gives the rule that keeps them apart; the sharp edge is
  that the foreman guards currently live in the tool path, and the refactor to share
  them is part of the work, not optional polish.
- *One behaviour is unproven.* Whether `gpt-5-5-pro` accepts a `conversation_id`
  created by another model (§3.2). Test it before promising seamless mid-thread
  switching into Pro.
- *`/gpt` is a consultation, not an agent.* ChatGPT web exposes no tool calling, so
  it cannot read the working tree or run anything. The command should make that
  obvious — and `pi-gpt`'s existing skill already warns that repository paths and
  summaries are not review context.

---

## 9. Appendix: what was run, and cleanup

Executed 2026-09-06 in an isolated scratch dir against `pins/prime-0.9.1`.

1. `npm view` / `npm pack` of the adapter; full read of the unpacked tarball.
2. GitHub API checks of the declared repo, its issue tracker, and the author's 71
   public repos.
3. `prime-agent package install` → `prime-agent model list` in an isolated agent dir
   (`PRIME_AGENT_CODING_AGENT_DIR`), with a negative control ("No models available")
   confirming the user's real `~/.prime/agent` was never read.
4. `pi-chatgpt-web auth status` and `doctor` — read-only; both reported not logged in.
5. Runtime export probe of `@earendil-works/pi-ai` on the pinned Prime (§2.4).
6. Source reads of `pins/packages/pi-gpt-0.4.3/src/`, the vendor patches, and Prime
   0.9.1's `dist/core/extensions/`, `dist/core/session-manager.js`,
   `dist/core/model-registry.js`, `dist/core/package-manager.js`, `dist/config.js`.

**Cleanup — verified complete.** No adapter daemon and no Chromium was ever started
(the extension spawns its daemon lazily and no model call was made; confirmed by `ps`,
and ports 1456 and 14561 were both free). The one change outside the scratch dir was
the package install, which landed in the **global npm root** rather than the isolated
agent dir — worth recording, since `PRIME_AGENT_CODING_AGENT_DIR` does **not** sandbox
package installation (`dist/core/package-manager.js:1536-1543`). It was removed:

```
$ prime-agent package remove npm:@minzicat/pi-chatgpt-web-adapter
removed 85 packages in 398ms
```

`npm root -g` then still held an empty `@minzicat/` scope directory created by that
install; it was removed with `rmdir`. The global npm root is back to its prior
contents (`@gotgenes`, `corepack`, `npm`, `pi-pr-review`, `pi-tasks`), and
`which pi-chatgpt-web` reports not found.

**The user's live Prime session was never touched.** No global `prime-agent shutdown`
was run; the same four PIDs (58735, 58899, 87916, 87962) were present before and
after this work.
