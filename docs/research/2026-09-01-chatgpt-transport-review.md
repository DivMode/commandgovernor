# ChatGPT Web foreman transport research — `pi-gpt` vs `pi-oracle`

**Date of research:** 2026-09-01 (all live fetches this date; these projects post-date my
training data, so nothing below is answered from memory).
**Scope:** ADR 0008 Gate P4 — closed loop into an *exact pre-existing* consumer ChatGPT Web
conversation (`https://chatgpt.com/c/<id>`) created by the user in their browser.
**Repo root verified:** `pwd` and `git rev-parse --show-toplevel` both `/Volumes/Data/Developer/commandgovernor`.
Work was read-only; nothing in the repository was modified.

**Scope limit honoured:** `pi-gpt` ships modules whose names indicate provider
security-control handling (`src/sentinel.ts`, `src/pow.ts`, `src/turnstile.ts`). This report
records **that** they exist, that the send path depends on them, and that this is a fragility
and terms-of-service risk factor. It does not describe, evaluate, or document how they work.

---

## 0. Method and primary sources

Because `pi-gpt`'s declared source repository is unavailable (see §1.1), the authoritative
primary source for both packages is the published npm tarball, which I downloaded and read in
full. Line references below are into those unpacked trees:

- `pi-gpt@0.4.3` — `https://registry.npmjs.org/pi-gpt/-/pi-gpt-0.4.3.tgz`
  (unpacked at `<scratchpad>`)
- `pi-oracle@0.7.20` — `https://registry.npmjs.org/pi-oracle/-/pi-oracle-0.7.20.tgz`
  (unpacked at `<scratchpad>`)

Registry metadata read live from `https://registry.npmjs.org/pi-gpt` and
`https://registry.npmjs.org/pi-oracle`. GitHub facts read live from `api.github.com`.

**Fetch failures, reported plainly:**

| URL | Result |
| --- | --- |
| `https://github.com/davidroman0O/pi-gpt` | **HTTP 404** (WebFetch) |
| `https://api.github.com/repos/davidroman0O/pi-gpt` | **404 Not Found** (GitHub API) |
| `https://www.npmjs.com/package/pi-gpt` | **HTTP 403 Forbidden** (WebFetch); worked around via `registry.npmjs.org` JSON, which succeeded |
| `https://raw.githubusercontent.com/earendil-works/pi/main/docs/extensions.md` | **404** — wrong path; the real path is `packages/coding-agent/docs/extensions.md`, which succeeded |

Everything else fetched cleanly.

---

## 1. `pi-gpt` — direct ChatGPT web-backend transport

### 1.1 Source repository: declared but **not public**

`package.json` declares `git+https://github.com/davidroman0O/pi-gpt.git` and a bugs URL at
that repo. Both 404. The author account exists and is real
(`https://api.github.com/users/davidroman0O` → login `davidroman0O`, display name `0xAkraw`,
75 public repos, created 2012-11-20) but `pi-gpt` is **not among its 75 public repositories**
(the only `pi*` repos are `pi-deep-research`, `pi-loop`, `ping-fs`).

`https://pi.dev/packages/pi-gpt` lists version 0.4.3, author `davidroman`, the same dead
repository link, 645 monthly / 89 weekly downloads, and carries a generic warning that "Pi
packages can execute code and influence agent behavior" with a recommendation to review source
before installing third-party packages.

**Verified fact:** the only obtainable source for `pi-gpt` is the npm tarball. There is no
public issue tracker, no commit history, no release diffs, no upstream to contribute to.
**Inference:** this fails ADR 0008's "Every dependency must be pinned, reviewed, licensed, and
exercised" bar for a *shipped* dependency — you can review a snapshot, but you cannot track it.
It does not block using it for a spike.

Registry facts: latest `0.4.3`, published 2026-08-30T15:47:43Z; 16 versions since
2026-07-06; license MIT; `main: extensions/chatgpt.ts`; **zero runtime dependencies**;
peerDeps `@earendil-works/pi-coding-agent: *`, `typebox: *`. The package registers **two**
extensions, `extensions/chatgpt.ts` and `extensions/observer.ts` — the latter is an unrelated
"silent supervisor" that watches the coding agent's messages and can call ChatGPT on its own
(off by default, enabled by `/gpt-observer`). Installing `pi-gpt` therefore installs a second,
independently-acting surface.

### 1.2 Authentication — Codex OAuth token, not browser cookies

`src/auth.ts:18-64` loads a bearer token from, in order:

1. `${CODEX_HOME:-~/.codex}/auth.json` → `tokens.access_token` (and `tokens.account_id`)
2. `~/.gpt2agent/token.json`

and throws ``No ChatGPT token found — run `codex login` `` if neither yields a >20-char string.
`src/client.ts:14-46` builds a `BackendClient` against `https://chatgpt.com` with
`Authorization: Bearer <token>`, a hardcoded desktop-Chrome `User-Agent`, hardcoded
`OAI-Client-Version`/`OAI-Client-Build-Number` (`prod-be885abb…`, `5955942`), random
per-process `OAI-Device-Id`/`OAI-Session-Id`, and `Origin`/`Referer` of `https://chatgpt.com`.
`reloadTokenIfStale()` re-reads the file when its mtime changes, because the Codex CLI
background-refreshes the token.

Requests use plain `fetch` — **no TLS impersonation, no native dependencies** (verified: the
package has zero runtime deps and `client.ts:62-66` calls global `fetch`). Some GET routes pass
`X-OpenAI-Target-Path` / `X-OpenAI-Target-Route` headers (`client.ts:58-59`), used for
`/backend-api/me`, `/backend-api/accounts/check/v4-2023-04-27`, `/backend-api/models`
(`extensions/chatgpt.ts:121-170`) but **not** for `/backend-api/conversation/<id>`.

**This is the single most important fact for Gate P4:** authentication is the *Codex* login,
not the user's browser session. The user's browser-created thread lives under the same ChatGPT
account, so it *should* be reachable — but whether a Codex-issued access token is authorised
for `GET`/`POST` on a conversation created by the browser client is **not proven by any source
I could reach**, and is the pivotal unknown (§3, R1).

### 1.3 Continuing an arbitrary existing conversation

**Structurally supported, with no gate on provenance.**

`gpt_chat` takes `conversation_id: Type.Optional(Type.String({ description: "Continue an
existing conversation by its id." }))` (`extensions/chatgpt.ts:226-228`). On the normal/agent
path (`extensions/chatgpt.ts:496-514`):

```ts
let parentId: string | undefined;
if (p.conversation_id) {
  try { parentId = (await conv.leafMessageId(p.conversation_id)) || undefined; }
  catch { /* fall through with a fresh parent */ }
}
const result = await conv.complete(model, messages, {
  ..., conversationId: p.conversation_id, parentMessageId: parentId, ...
});
```

`leafMessageId` (`src/conversation.ts:594-601`) does `GET /backend-api/conversation/<id>`,
reads `current_node`, and returns `mapping[current_node].message.id`. `buildPayload`
(`src/conversation.ts:98-109`) then sets `parent_message_id` and `conversation_id` on the
`POST https://chatgpt.com/backend-api/conversation` body.

The **local registry is not consulted for continuation.** `src/registry.ts` is explicitly a
convenience index — "Project-scoped chat registry… so an agent can answer 'list all chats I
started for THIS project'" (`registry.ts:1-3`), stored at `${PI_GPT_HOME:-~/.pi-gpt}/registry.json`,
written best-effort and never fatal (`registry.ts:37-44`). `gpt_list_chats` reads it;
`gpt_chat` does not. So a browser-created id is accepted on exactly the same footing as one
`pi-gpt` created — **there is no provenance restriction in the code.**

Two correctness gaps in the shipped code:

- **No equality assertion on the returned conversation.** `extensions/chatgpt.ts:516` returns
  `done(result.text, result.conversationId || p.conversation_id || null, …)` — if the server
  returned a *different* conversation, that id is reported; if it returned none, the code
  falls back to asserting the requested id. Thread drift would not be flagged.
- **`leafMessageId` failure is swallowed.** The `catch {}` at `extensions/chatgpt.ts:501-503`
  falls through with `parentId === undefined`, and `buildPayload` then substitutes
  `parent_message_id: randomUUID()` (`src/conversation.ts:98`). A transient read failure
  therefore sends into the thread with a fabricated parent rather than failing.

  (The deep-research path is stricter: `extensions/chatgpt.ts:382-385` throws
  ``Conversation not found or has no active leaf: ${convId}`` when the leaf cannot be read.)

Continuation is exercised by unit tests with mocked `fetch`:
`tests/conversation-continuation.test.ts:305` ("sends Heavy Research into the requested
conversation and leaf" — asserts `payload.conversation_id === "existing-conversation"` and
`payload.parent_message_id === "existing-leaf"`) and `:331` for legacy deep research. These
prove the payload wiring, **not** that the backend accepts it for a browser-created thread.

### 1.4 Conversation and message identities exposed

`gpt_get_conversation` (`extensions/chatgpt.ts:548-640`) does
`GET /backend-api/conversation/<id>?include_visually_hidden_messages=true&include_widget_state=true`,
then walks `current_node` up through `node.parent` and reverses — i.e. it returns the **active
branch in chronological order**. Each returned message carries:

```
{ id, role, content_type, status, create_time, text? | code? }
```

`gpt_get_message` (`:643-672`) fetches one message by id from the same mapping and returns
`{ id, role, content_type, status, create_time }` plus text.

So: **message ids yes; parent ids no.** `node.parent` is used internally to order the active
branch but is not surfaced in `details.messages`. Branch topology is therefore invisible to the
caller — you can see *the* active branch, not *which* branch or that a branch switch occurred.

**Three fidelity hazards, all verified in source:**

1. **Redaction mangles the payload.** Every returned message text passes through `redact()`
   (`extensions/chatgpt.ts:606, 620, 660`). `src/redact.ts:8` defines
   `PHONE_RE = /\+?\d[\d ()\-]{8,}\d/g` → any run of ≥10 characters drawn from digits, spaces,
   parentheses and hyphens is replaced with `<PHONE>`. **A numeric or digit-and-hyphen delivery
   id will be destroyed on readback.** `redact()` also rewrites JWTs, `Bearer …`, `sk-/pk-/rk-`
   keys, GitHub tokens, `refresh_token=…`, and emails.
2. **Truncation.** Message text is `.slice(0, 4000)` (`:606`), code `.slice(0, 500)` (`:608`),
   and the list is `.slice(-max)` with `max` defaulting to 50 (`:559, :624`).
3. **`gpt_get_message` returns only `parts[0]`** of the first string part (`:658-660`).

### 1.5 Reading a conversation back

Supported and cheap: `gpt_get_conversation` and `gpt_get_message` are plain authenticated GETs
with no send-side machinery (no sentinel path — compare `sentinelHeaders()` at
`src/conversation.ts:578-590`, used only by `stream()`/deep-research POSTs). The read leg is
strictly lower-risk than the write leg.

### 1.6 Interrupted send — what is there to reconcile against?

**Nothing durable, as shipped.**

- `complete()` returns only `{ text, conversationId }` (`src/conversation.ts:729-752`). The
  client-generated user-message id is **not returned**.
- The registry row is written only on the success path, inside `done()`
  (`extensions/chatgpt.ts:296-308`), and records only
  `{conversation_id, title, chat_type, intelligence, model, created_at, cwd}` — no delivery
  identity, no per-message record.
- If the SSE stream aborts, `stream()` throws (`src/conversation.ts:624-626`) or the generator
  ends; `gpt_chat` propagates the error. Nothing is persisted.
- Pi itself documents **"No independent background processes"** for extensions
  (`packages/coding-agent/docs/extensions.md`), so a `gpt_chat` call lives and dies inside one
  Pi turn. A Pi restart mid-send loses the call entirely.

**However, replay need not be blind — but only if Command Governor builds the reconciliation.**
The mechanism is available: embed a unique delivery id in the prompt body, record it durably
*before* calling `gpt_chat`, and on restart call `gpt_get_conversation(<id>)` and search the
active branch for that marker. This works *only* because the marker is in the message body, so
the redaction constraint in §1.4 is load-bearing, not cosmetic.

There is a genuinely good internal correlation primitive that the tool surface hides:
`complete()` builds `sentMessageIds` from the client-side message ids and passes them to
`pollAsyncResponse(..., requiredAncestorIds)` (`src/conversation.ts:732-733, 749`), which walks
each candidate assistant node up its `parent` chain and rejects any answer that does not
descend from the message just sent (`src/conversation.ts:781-795`). This is directly tested:
`tests/conversation-continuation.test.ts:392` — "async polling ignores finished answers outside
the new user-message branch" — constructs a mapping with an older `OLD ANSWER` and asserts the
poller returns `NEW ANSWER`. **That is exactly the stale-reply guard Gate P4 needs, and it is
implemented and tested — but it is not reachable from the tool surface**, because `ChatMessage.id`
(`src/conversation.ts:24-28`) is settable on the library but `gpt_chat` constructs
`[{ role: "user", content: prompt }]` with no id (`extensions/chatgpt.ts:494`) and never returns
the generated one.

### 1.7 Undocumented backend, license, maintenance, stability

- **License:** MIT (registry and `package.json`).
- **Backend:** entirely undocumented private endpoints — `/backend-api/conversation`,
  `/backend-api/f/conversation`, `/backend-api/conversation/init`, `/backend-api/files`,
  `/backend-api/me`, `/backend-api/models`, `/backend-api/accounts/check/v4-2023-04-27`,
  `/backend-api/sentinel/chat-requirements`. Client version/build strings are pinned constants
  (`src/client.ts:7-8`) that will drift.
- **Security-control dependency (opaque, per scope limit):** every send goes through
  `sentinelHeaders()` (`src/conversation.ts:578-590, 616`), which attaches sentinel /
  proof / turnstile headers produced by `src/sentinel.ts`, `src/pow.ts`, `src/turnstile.ts`.
  I did not analyse those modules. The relevant facts are: (a) the send path **depends** on
  them, so a provider-side change there breaks sends with no warning, and (b) their presence
  means adopting `pi-gpt` puts a provider-security-control-solving mechanism inside the
  Command Governor product — which sits badly against ADR 0008 §8's explicit position that
  "Command Governor does not define bypassing provider security controls as a product
  requirement." The **read** path does not use them.
- **Lineage and its own warning.** `src/auth.ts:1` — "mirrors gpt2agent's search order";
  `src/redact.ts:1` — "Ported from gpt2agent tools/_redact.py". The ancestor project
  (`https://github.com/robotlearning123/gpt2agent`) states in its own README that it is
  "**very likely against the OpenAI Terms of Service**, and automated/abnormal traffic can get
  your account **rate-limited, challenged, suspended, or banned**", uses "TLS fingerprint
  impersonation and vendored Proof-of-Work + Cloudflare Turnstile solvers", and advises "Use an
  account you can afford to lose, keep volume human-scale, and don't rely on it for anything
  critical." `pi-gpt` is a lighter variant (no TLS impersonation) but shares the endpoint
  surface and the account-risk profile. gpt2agent additionally exposes `list_conversations`
  ("Recent ChatGPT conversations") and `get_conversation` ("Full message history for a specific
  conversation") — **corroborating, but not proving, that the Codex token can read
  account-wide conversation history.**
- **Maintenance:** 16 versions in ~8 weeks (active), but no public repo, no issue tracker, no
  changelog. Bus factor 1, visibility 0.

---

## 2. `pi-oracle` — isolated-browser web-app transport

Repo `https://github.com/fitchmultz/pi-oracle`: MIT, 39 stars, 9 forks, **0 open issues**,
created 2026-04-02, last push **2026-07-29T00:44:36Z** (~5 weeks before today). Latest npm
`0.7.20`, published 2026-07-29; 62 versions. Author Mitch Fultz. Recent commits are release and
UI-drift fixes ("fix: accept undifferentiated ChatGPT Pro compact menu", "chore: validate Pi
0.80.9"). README status: **experimental public beta**; "Provider UI, auth, model controls, and
artifact download behavior can drift."

### 2.1 Exact-thread targeting — an explicit, designed feature

`CHANGELOG.md:119` records it as a deliberate addition: "added explicit existing ChatGPT
browser-thread targeting for `/oracle`, `oracle_preflight`, and `oracle_submit` through optional
`chatGptConversationId`, accepting raw ChatGPT conversation ids or full `https://chatgpt.com/c/...`
/ `https://chat.openai.com/c/...` URLs while preserving fresh-thread defaults when omitted."

`docs/ORACLE_DESIGN.md:162` names the target as "a user/browser-created ChatGPT
`https://chatgpt.com/c/<id>` URL", and `:508` — "for an explicit existing ChatGPT thread,
normalize `chatGptConversationId` to `https://chatgpt.com/c/<id>` **without requiring prior
oracle job state**". That last clause is exactly Gate P4's requirement.

Normalization: `resolveChatGptConversationReference` (`extensions/oracle/lib/tools.ts:231-260`)
accepts a full URL (must be `https:`, host in `{chatgpt.com, chat.openai.com}`, path
`/(c|chat)/<id>`) or a bare id matching `/^[A-Za-z0-9][A-Za-z0-9-]{7,}$/`, and returns
`{ chatUrl: "<origin>/c/<id>", conversationId }`. `resolveConversationTarget` (`:287-306`)
rejects passing both `followUpJobId` and `chatGptConversationId`, and stamps
`provider: "chatgpt"`.

The worker opens it directly: `const targetUrl = currentJob.chatUrl || currentJob.config.browser.chatUrl;`
then `launchBrowser(currentJob, targetUrl)` (`extensions/oracle/worker/run-job.mjs:2349-2350`).

**Gap found:** after send, the worker *overwrites* rather than *asserts*:

```js
const observedChatUrl = await waitForStableChatUrl(currentJob, currentJob.chatUrl);
const observedConversationId = conversationIdFromUrl(observedChatUrl) || currentJob.conversationId;
const awaitingResponsePatch = {
  heartbeatAt: ...,
  ...(observedConversationId ? { chatUrl: observedChatUrl, conversationId: observedConversationId } : {}),
};
```
(`run-job.mjs:2379-2386`). `resolveStableConversationUrlCandidate`
(`worker/chatgpt-flow-helpers.mjs:94-100`) accepts *any* conversation-path URL. So if
navigation silently landed on a fresh thread, the job record would be rewritten to that thread
and the job would still complete "successfully". **Command Governor must assert
`job.conversationId === requested_id` itself.**

### 2.2 Isolated browser auth runtime

`/oracle-auth` imports cookies from the configured local browser profile into an **isolated seed
profile**; each job clones that seed into its own temporary runtime profile (README "How it
works"; macOS APFS clone, recursive copy elsewhere). The real Chrome profile is never automated
(`docs/ORACLE_RECOVERY_DRILL.md:11-17` makes this an explicit safety guarantee). Cookie reading
is delegated to `@steipete/sweet-cookie` (the only runtime dependency). Worker launches Chromium
directly and attaches `agent-browser` over DevTools (`run-job.mjs:673-695`).

**This is a materially different risk posture from `pi-gpt`:** it drives a real browser with the
user's real web session. No sentinel/PoW/turnstile solving appears anywhere in the tree. It is
still automation of a web UI, but it does not put a security-control-solving mechanism into the
product.

### 2.3 Durable job state

`OracleJob` (`extensions/oracle/lib/jobs.ts:131-192`) is a rich durable record persisted as
`job.json` under `${PI_ORACLE_JOBS_DIR:-/tmp}/oracle-<job-id>/`
(`extensions/oracle/shared/state-path-helpers.mjs:4-9` — `DEFAULT_ORACLE_JOBS_DIR = "/tmp"`,
overridable via `PI_ORACLE_JOBS_DIR`). Fields directly relevant to Gate P4:

`id, status, phase, phaseAt, createdAt, queuedAt, submittedAt, completedAt, heartbeatAt,
cancelRequestedAt, projectId, sessionId, originSessionFile, followUpToJobId, chatUrl,
conversationId, responsePath, responseFormat, artifactPaths, workerPid, workerNonce,
workerStartedAt, runtimeId, seedGeneration, notifiedAt, notificationEntryId,
wakeupAttemptCount, wakeupSettledAt, wakeupObservedAt, notifyClaimedAt, notifyClaimedBy,
error, lifecycleEvents`.

Phase transitions are written durably at every step: `cloning_runtime → launching_browser →
verifying_auth → configuring_model → uploading_archive → [send] → awaiting_response →
extracting_response → downloading_artifacts → complete | complete_with_artifact_errors | failed`
(`run-job.mjs:2333-2450`), each via `mutateJob(job => transitionOracleJobPhase(...))` with a
heartbeat.

**Conversation leasing** exists: `createLease(ORACLE_STATE_DIR, "conversation", conversationId, metadata)`
with release on completion/failure (`run-job.mjs:374, 416-425, 524`). Two oracle jobs cannot
concurrently drive the same thread. (It does not and cannot lease against the *human* typing in
their own browser.)

Queue admission caps concurrent/queued jobs and queued archive bytes
(`lib/tools.ts:128-208`).

### 2.4 Wake-back into Pi — explicitly best-effort

`extensions/oracle/lib/poller.ts:1-5` states the contract: "Poll oracle jobs in the background,
reconcile stale state, and deliver best-effort wake-up reminders to eligible sessions… **wake-up
delivery is best-effort, and terminal-job notifications always re-read durable job state before
send.**" Delivery is guarded by `tryClaimNotification(jobId, claimant)` /
`releaseNotificationClaim` (`poller.ts:284-303`) plus a `wakeup-target` lease keyed by poller
session + pid + process start time — i.e. **idempotent, single-delivery, and safe under
concurrent Pi sessions**. README: "**Wake-up is best effort, storage is durable.** A missed
wake-up does not lose the result." `/oracle-read [job-id]` retrieves it regardless.

### 2.5 Reply reading and correlation — positional, not identity-based

This is `pi-oracle`'s weakest point for Gate P4.

- Before sending, the worker counts existing assistant turns:
  `const baselineAssistantCount = (await assistantMessages(currentJob)).length;`
  (`run-job.mjs:2373`).
- The reply is then taken **by index**: `const targetMessage = messages[baselineAssistantCount];`
  (`run-job.mjs:1830`), with completion detected by snapshot heuristics ("Stop streaming"
  absent, `Copy response` count exceeding baseline, text stable) (`run-job.mjs:1815-1876`).
- Extraction is a slice of the **accessibility snapshot** between `heading "ChatGPT said:"`
  markers (`worker/chatgpt-flow-helpers.mjs:16-33`), then `stripChatGptResponseChrome`
  (`run-job.mjs:2414`), written to `response.md` via `secureWriteText` (`:2415`).
- Send acceptance is also heuristic: `providerSendAccepted(before, after)`
  (`chatgpt-flow-helpers.mjs:78-87`) returns true if the conversation id in the URL changed, or
  the assistant count rose, or a "stop streaming" control appeared. It is a signal, not a receipt.

There are **no message ids anywhere** — the browser DOM does not surface them. Correlation must
be by content (an echoed delivery id). A concurrent human turn in the same thread shifts the
index and can cause the wrong message to be captured.

### 2.6 Other operational constraints

- **Requires a persisted Pi session** — `oracle_submit` fails with code
  `persisted_session_required` otherwise (`lib/tools.ts:409-414`); README confirms `pi --no-session`
  is unsupported.
- **`files` is mandatory** on `oracle_submit`: `Type.Array(..., { minItems: 1 })`
  (`lib/tools.ts:81-88`) and is not `Optional`. **Every foreman event therefore costs a
  `.tar.zst` repo-archive build and upload** (cap 250 MiB ChatGPT / 200 MiB Grok). For a small
  correlated envelope this is a heavy per-event cost and a latency floor of a full browser
  launch, auth verification, model configuration, and archive upload.
- Node ≥22.19.0; needs a Chromium-family browser plus `agent-browser`, `tar`, `zstd`.
- Pinned dev/peer deps target `@earendil-works/pi-*` `^0.80.9`; Pi core is currently **v0.84.4**
  (released 2026-08-28). Four minors of drift — a Gate P1 compatibility item.
- **Test quality is genuinely good** for a beta: `npm test` runs `verify:oracle` =
  syntax checks on every worker/shared module, an esbuild bundle check, two `tsc --noEmit`
  projects, a sanity runner, and `npm pack --dry-run`; `release:check` adds a ChatGPT preset
  proof and a three-platform smoke matrix (macOS, Ubuntu-in-container, Windows native) with an
  invariants script. There are documented operator drills:
  `docs/ORACLE_RECOVERY_DRILL.md` (expired/missing auth must fail *classified as auth*, not as
  "generic timeout" or "vague UI drift", then `/oracle-auth` must repair it) and
  `docs/ORACLE_ISOLATED_PI_VALIDATION.md`. These are **process/UI-drift** tests, not
  correlation-semantics tests — nothing tests the concurrent-human-turn case.
- Failure path: on any worker exception, `captureDiagnostics` then a durable `failed` phase
  transition (`run-job.mjs:2442-2450`). Worker is detached and outlives the Pi turn.

---

## 3. Assessment against the required foreman semantics

Legend: **SUP** = supported as shipped · **GLUE** = supportable with Command-Governor-owned glue
· **LIVE** = unknown, needs a live test · **NO** = not supported.

### R1 — Exact-thread binding to a pre-existing browser-created conversation

| | Verdict | Evidence |
| --- | --- | --- |
| `pi-gpt` | **LIVE** (structurally GLUE) | `conversation_id` flows straight to `payload.conversation_id` + a leaf-derived `parent_message_id`; the local registry imposes no provenance gate (`extensions/chatgpt.ts:496-514`, `src/conversation.ts:98-109`, `src/registry.ts:1-3`). Unproven link: whether a **Codex** OAuth token (`src/auth.ts:18-32`) is authorised on a **browser-created** conversation. gpt2agent's `list_conversations`/`get_conversation` corroborate account-wide history access but do not prove it. Also needs glue: assert the returned id equals the requested id (`extensions/chatgpt.ts:516` silently falls back). |
| `pi-oracle` | **SUP** | Designed feature: `chatGptConversationId` on `oracle_preflight`/`oracle_submit`, normalized to `https://chatgpt.com/c/<id>` "without requiring prior oracle job state" (`lib/tools.ts:99-113, 231-260`; `docs/ORACLE_DESIGN.md:162, 508`; `CHANGELOG.md:119`). Glue still required: assert `job.conversationId === requested`, because the worker overwrites it unasserted (`run-job.mjs:2379-2386`). |

### R2 — Unique delivery correlation

| | Verdict | Evidence |
| --- | --- | --- |
| `pi-gpt` | **GLUE** | The right primitive exists and is tested but is not exposed: `sentMessageIds` → `requiredAncestorIds` ancestry filter (`src/conversation.ts:732-733, 749, 781-795`), yet `gpt_chat` neither accepts nor returns a message id (`extensions/chatgpt.ts:494`, `src/conversation.ts:751`). CG must carry the delivery id **in the message body** and match on readback. **Constraint discovered:** `redact()` replaces any ≥10-char run of digits/spaces/parens/hyphens with `<PHONE>` (`src/redact.ts:8, 29`), so a numeric delivery id is destroyed on readback — ids must contain letters. |
| `pi-oracle` | **GLUE** | No message identity at all; correlation is positional (`run-job.mjs:2373, 1830`). CG must embed a delivery id in the prompt and require the foreman to echo it; the durable `job.json` (`id`, `conversationId`, `submittedAt`, `responsePath`) is the anchor. |

### R3 — Interrupted-send reconciliation, never blind replay

| | Verdict | Evidence |
| --- | --- | --- |
| `pi-gpt` | **NO as shipped → GLUE** | Nothing durable is written before or during a send; the registry row is written only on success (`extensions/chatgpt.ts:296-308`); the sent message id is discarded (`src/conversation.ts:751`); Pi has "No independent background processes" (`packages/coding-agent/docs/extensions.md`), so the whole call dies with the turn. Reconciliation is *constructible* — pre-write the delivery id, then `gpt_get_conversation` and search — and this is exactly why the redaction constraint above is load-bearing. |
| `pi-oracle` | **GLUE**, materially closer | Durable phase machine written across the send boundary (`uploading_archive` → send → `awaiting_response`), plus `submittedAt`/`heartbeatAt`/`workerPid`/`workerNonce`/`lifecycleEvents` (`lib/jobs.ts:131-192`, `run-job.mjs:2365-2392`) and a per-conversation lease. A crash in the window between `clickSend` and the `awaiting_response` write is still ambiguous, and `providerSendAccepted` is heuristic (`chatgpt-flow-helpers.mjs:78-87`) — so the resolver of last resort is still a readback for the delivery-id marker. |

### R4 — Reading the exact reply

| | Verdict | Evidence |
| --- | --- | --- |
| `pi-gpt` | **SUP** (with fidelity caveats) | `gpt_get_conversation` walks `current_node` up `parent` and returns the active branch with per-message `{id, role, content_type, status, create_time, text}` (`extensions/chatgpt.ts:565-624`); `gpt_get_message` by id (`:643-672`). Caveats: `redact()`, 4000-char truncation, last-50 default, and **no `parent` in the returned rows** — branch topology invisible. |
| `pi-oracle` | **SUP** (lossier) | `response.md` under the durable job dir, readable via `/oracle-read` regardless of wake-up. But the text is an accessibility-snapshot slice (`chatgpt-flow-helpers.mjs:16-33`) plus chrome-stripping — a scrape, not the source message. Not byte-exact. |

### R5 — Stale-reply detection

| | Verdict | Evidence |
| --- | --- | --- |
| `pi-gpt` | **GLUE** (strong basis) | The ancestry guard is implemented and directly tested — `tests/conversation-continuation.test.ts:392` "async polling ignores finished answers outside the new user-message branch" — but unreachable from the tool surface. CG can reproduce it over `gpt_get_conversation` output: locate the delivery-id user message on the active branch, take the assistant message immediately after it, reject actions whose `task_revision` is superseded. **Branch hazard:** if the user edits a message in the browser, `current_node` moves to a new branch and `leafMessageId` follows it — your event can land on a branch the user then abandons. Detectable only by re-reading and confirming the delivery id is still on the active branch. |
| `pi-oracle` | **LIVE** (weak) | Index-based capture (`messages[baselineAssistantCount]`) breaks if a human posts in the same thread while the job runs. Nothing in the test suite covers it. Mitigate by requiring the delivery id to be echoed and rejecting any response lacking it — but the *capture* can still pick the wrong message, so this is a live-test item. |

### R6 — Restart survival

| | Verdict | Evidence |
| --- | --- | --- |
| `pi-gpt` | **NO** | No durable job state; single-turn, in-process; registry is a best-effort convenience index (`src/registry.ts:37-44`). Pi documents no background processes. |
| `pi-oracle` | **SUP** | Detached worker outlives the Pi turn; durable `job.json` + `lifecycleEvents`; poller reconciles stale state; wake-up is idempotent-claimed and best-effort by contract; result survives a missed wake-up (`lib/poller.ts:1-5, 284-303`; README). Constraints: requires a persisted Pi session; **point `PI_ORACLE_JOBS_DIR` at a durable directory — `/tmp` is the default** (`shared/state-path-helpers.mjs:4`). |

### Summary matrix

| Requirement | `pi-gpt` | `pi-oracle` |
| --- | --- | --- |
| R1 exact-thread binding | LIVE (structurally fine) | **SUP** (+ assert id) |
| R2 delivery correlation | GLUE (id in body; letters mandatory) | GLUE (echoed id only) |
| R3 interrupted-send reconciliation | NO → GLUE | GLUE (best substrate) |
| R4 read the exact reply | **SUP** (ids, active branch) | SUP (lossy scrape) |
| R5 stale-reply detection | GLUE (strong basis, tested) | LIVE (weak, index-based) |
| R6 restart survival | **NO** | **SUP** |

Neither candidate delivers Gate P4 alone. `pi-gpt` has the better *correlation* material
(message ids, parent chains, a tested ancestry guard) and no durability. `pi-oracle` has the
better *durability* material (detached worker, durable job ledger, leases, idempotent wake-up)
and the weaker correlation. The gap in both cases is the same, and it is Command Governor's to
fill: a delivery-id-in-body protocol plus an owned event ledger.

---

## 4. Recommendation

### 4.1 Spike order

**Spike `pi-gpt` first — but as a ~30-minute *capability probe*, not as a candidate
integration.** Rationale:

- It answers the one unknown that changes the architecture, cheaply: *can a Codex-issued token
  read and write a conversation the user created in their browser?* Everything else about
  `pi-gpt` is already determined from source.
- Its **read** leg (`gpt_get_conversation`) is valuable *independently of which transport sends*:
  it is a plain authenticated GET with no sentinel dependency, it returns real message ids and
  the active branch, and it is the natural **verification oracle** for reconciliation and
  stale-reply checks even if `pi-oracle` does the sending. If the probe passes, you have a
  strictly better readback than scraping a DOM.
- If the probe fails, you have eliminated a whole branch of the design in half an hour.

**Build the Gate P4 loop on `pi-oracle` as the presumed product transport.** Rationale:

- It is the only candidate that already satisfies R1 and R6, which are the two requirements
  that cannot be glued on from outside.
- Its risk posture matches ADR 0008 §8. `pi-gpt`'s send path depends on provider
  security-control modules; making it the default transport would put that mechanism inside the
  product, against ADR 0008 §8's explicit stance, and its acknowledged ancestor states plainly
  that the technique risks account suspension. That is not a defect of the spike — it is a
  reason not to *ship* it as the default.
- Its source is public, MIT, reviewable, and has an issue tracker; `pi-gpt`'s is not obtainable
  at all (§1.1), which fails ADR 0008's dependency-curation bar for a shipped component.

**Do not adopt `pi-gpt` as a dependency on the strength of a passing probe.** If the probe
passes and you want the direct transport, the honest options are (a) use only its *read* leg,
or (b) implement the ~250 lines of conversation continuation you actually need in a
Command-Governor-owned extension — noting that (b) does not avoid the §8 problem for the send
path, only for the read path.

### 4.2 Minimal live experiment — Experiment A: `pi-gpt` exact-thread probe

Preconditions: `codex login` completed; `CODEX_HOME` noted; the user creates or picks a thread
in their browser, posts one human message in it, and supplies `https://chatgpt.com/c/<ID>`.

1. `gpt_account_status` → record `account_id`, `auth_profile`, plan. Confirms the token works
   at all and identifies which account you are on.
2. **Read before write (the falsifier).** `gpt_get_conversation(conversation_id: <ID>)`.
   - *Pass:* the browser-created thread's messages return → the Codex token can see browser
     threads; continue.
   - *Fail (403/404/empty):* → `pi-gpt` is **NOT-SUPPORTED** for exact pre-existing thread
     adoption. Stop; go to Experiment B.
   This step is read-only and cannot pollute the user's thread — do it first, always.
3. `gpt_chat(conversation_id: <ID>, prompt: <FOREMAN_EVENT carrying delivery_id
   "CG-D-<base32, letters guaranteed>">)`. Assert `details.conversation_id === <ID>` (do not
   trust the tool's fallback).
4. `gpt_get_conversation(<ID>)` again. Assert three things separately:
   (a) the delivery id survived readback **verbatim** — this tests the `redact()` hazard;
   (b) it sits on the active branch;
   (c) the assistant reply is the message immediately following it.
5. **Reconciliation probe.** Repeat step 3 with a fresh delivery id and abort mid-stream (Esc).
   Then `gpt_get_conversation(<ID>)` and check whether the aborted delivery id is present.
   Both outcomes are informative: *present* → an interrupted send is observable and blind
   replay is avoidable; *absent* → you have an ambiguous window and must decide how to
   classify it. Record which.
6. Cross-check in the user's real browser: is the message in the thread, in the right place,
   and did the thread's model/title change? (`buildPayload` sends its own `model` and
   `conversation_mode`, `src/conversation.ts:99-100`.)

### 4.3 Minimal live experiment — Experiment B: `pi-oracle` exact-thread adoption

1. Set `PI_ORACLE_JOBS_DIR` to a **durable** directory (not `/tmp`). Start a **persisted** Pi
   session. Run `/oracle-auth` to seed the isolated profile.
2. `oracle_preflight(provider: "chatgpt", chatGptConversationId: "https://chatgpt.com/c/<ID>")`
   → expect `ready: true`.
3. `oracle_submit(prompt: <FOREMAN_EVENT with delivery_id>, files: ["<one tiny file>"],
   chatGptConversationId: <ID>)`. Note the mandatory `files` — record the archive build+upload
   latency, since it is the per-event floor.
4. From the durable `job.json`, assert `conversationId === <ID>` **after** completion. This is
   the assertion `pi-oracle` does not make for you (`run-job.mjs:2379-2386`).
5. Assert `response.md` contains the echoed delivery id.
6. **Crash injection.** `kill -9` the worker between `uploading_archive` and `awaiting_response`.
   Restart Pi. Verify that the persisted phase plus a readback of the thread distinguishes
   "sent" from "not sent" *without* replaying. Repeat with the kill placed after
   `awaiting_response`.
7. **Concurrency falsifier.** While a job is in `awaiting_response`, have the user post a
   message in the same thread from their own browser. Check whether the captured response is
   still the correct one. This is the direct test of the index-based correlation in
   `run-job.mjs:1830` and the one thing the package's own suite does not cover.

### 4.4 Fallback plan

1. **Send via `pi-oracle`, read via `pi-gpt`** — only if Experiment A step 2 passed. Gives
   durable send plus identity-bearing readback, and keeps the sentinel-dependent send path out
   of the product. Cost: two transports, two auth systems, and the read leg still uses the
   undocumented backend.
2. **ADR 0004's MCP topology**, if neither candidate can bind the exact thread: ChatGPT calls
   back into a Command Governor MCP server to return the disposition. ADR 0008 §7 already
   preserves MCP as optional interoperability precisely for this case; this is where it earns
   its place rather than being legacy topology.
3. **User-mediated loop**: Pi renders the `FOREMAN_EVENT` envelope, the user pastes it into
   their thread and pastes the reply back into a `governor_foreman_reply` tool. Command
   Governor still owns the ledger, the correlation, the stale-revision rejection, and the
   disposition — so the *semantics* of Gate P4 are met and only the transport is manual. This
   should be built anyway, because it is the degraded mode when the browser transport drifts.

---

## 5. Durable state the Command Governor foreman extension must own regardless of transport

Neither candidate owns any of this, and Pi cannot own it either. Pi's only extension-persistence
primitive is `pi.appendEntry(customType, data?)` — "Persist extension data. Custom entries do
NOT participate in LLM context" — restored via `ctx.sessionManager.getEntries()` on
`session_start` (`packages/coding-agent/docs/extensions.md`). That is **session-scoped**: it
does not survive as a single authority across `session_before_fork` / `session_before_switch`,
and Pi documents **"No independent background processes"** for extensions. ADR 0008 §5 already
permits an extension-owned durable sidecar; Gate P4 makes one mandatory.

1. **Foreman event ledger** — append-only, outside the Pi session file, keyed by
   `delivery_id`. Records `task_id`, `task_revision`, `delivery_id`, `event_kind`, payload
   digest, bound `conversation_id`, `created_at`, and a state machine:
   `PREPARED → SEND_ATTEMPTED → SEND_OBSERVED → REPLY_READ → DISPOSITIONED`.
   The `SEND_ATTEMPTED` row **must be durable before the transport call returns or throws** —
   otherwise reconciliation is impossible and every restart is a blind replay.
2. **Thread binding record + lease** — which `conversation_id` a project/task is bound to, who
   bound it, when, and a lease so two Pi sessions never write into the same foreman thread.
   `pi-oracle` has a per-job conversation lease (`run-job.mjs:416-425`) that CG can build on;
   `pi-gpt` has nothing.
3. **Delivery-id → observed-reply mapping** — the assistant message id (`pi-gpt` path) or the
   job id + `responsePath` (`pi-oracle` path) that answered each delivery. This is the
   idempotence key that makes a duplicate reply a no-op rather than a second disposition.
4. **Disposition record** — `ACK | REVISE | DELEGATE | ASK_USER` with `task_revision` validated
   at record time, written durably **before** any worker side effect (ADR 0008 foreman-protocol
   semantics).
5. **Stale-revision rejection log** — replies received for a superseded revision must be
   *recorded as rejected*, not silently dropped, so "the foreman answered and we ignored it" is
   auditable.
6. **Reconciliation cursor per bound conversation** — last-read message id / `create_time`, so a
   fresh process can re-read incrementally and detect a reply that arrived while Pi was down.

### Phase-A foundation requirements that Gate P4 imposes

- **The ledger and binding store must be an extension-owned durable sidecar**, not the Pi
  session file. Use `pi.appendEntry` only as an in-session mirror for visibility.
- **`delivery_id` encoding must be fixed in Phase A with the redaction hazard designed in:**
  letters mandatory, never a long digit/hyphen run (`src/redact.ts:8` will turn one into
  `<PHONE>` on readback). This is a foundation decision because every stored ledger row and
  every sent envelope carries it.
- **The `FOREMAN_EVENT`/`FOREMAN_ACTION` envelope must be scrape-tolerant**, because at least
  one candidate reads replies from an accessibility snapshot and the other truncates at 4000
  characters. The delivery id and the action keyword must appear early, on their own lines, and
  survive whitespace normalization — not be buried in JSON a scraper may mangle.
- **The transport must be an interface from day one**, with exactly the operations both
  candidates can implement: `bind(conversation_ref) → binding`,
  `send(binding, delivery_id, body) → receipt | AMBIGUOUS`,
  `read_since(binding, cursor) → messages`. `AMBIGUOUS` must be a first-class return value,
  because both candidates can produce it. No transport-specific shape may leak into the ledger.
- **Phase A must not assume send-and-reply within one Pi turn.** `pi-gpt`'s shape is
  synchronous-in-turn; `pi-oracle`'s is detached-worker-plus-best-effort-wake-up; Pi itself
  forbids background processes in-extension. The reply leg must therefore be **pollable from a
  fresh process** against the durable ledger, or Gate P4's restart-survival criterion cannot be
  met by the `pi-oracle` path and cannot be met at all by the `pi-gpt` path.
- **Gate P1 note:** `pi-oracle@0.7.20` validates against `@earendil-works/pi-*` `^0.80.9`;
  Pi core is at **v0.84.4** (2026-08-28). Pin and characterize that drift before relying on it.

---

## 6. Citations

**Live web sources (fetched 2026-09-01):**

- `https://registry.npmjs.org/pi-gpt` — versions, dist, license, repository, `main`, timestamps
- `https://registry.npmjs.org/pi-oracle` — versions, license, repository, timestamps
- `https://registry.npmjs.org/pi-gpt/-/pi-gpt-0.4.3.tgz` — full source (38 files, 227 KB unpacked)
- `https://registry.npmjs.org/pi-oracle/-/pi-oracle-0.7.20.tgz` — full source
- `https://api.github.com/repos/fitchmultz/pi-oracle` — 39 stars, 9 forks, 0 open issues, MIT, pushed 2026-07-29T00:44:36Z, created 2026-04-02
- `https://api.github.com/repos/fitchmultz/pi-oracle/commits` — recent commit log
- `https://api.github.com/repos/davidroman0O/pi-gpt` — **404 Not Found**
- `https://api.github.com/users/davidroman0O` + `/repos?per_page=100` — account exists, 75 public repos, `pi-gpt` absent
- `https://api.github.com/repos/earendil-works/pi` — MIT, 100,299 stars, pushed 2026-09-01
- `https://api.github.com/repos/earendil-works/pi/releases` — v0.84.4 (2026-08-28), v0.84.3, v0.84.2, v0.84.1, v0.84.0
- `https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/extensions.md` — extension API surface, event list, `pi.appendEntry`, "No independent background processes"
- `https://pi.dev/packages/pi-gpt` — listing, download counts, third-party-package warning
- `https://github.com/fitchmultz/pi-oracle` (README) — design, auth, durability, wake-up, limits
- `https://raw.githubusercontent.com/robotlearning123/gpt2agent/main/README.md` — ancestor project's own ToS/ban/stability warnings and endpoint list

**Source files read in full or in relevant part:**

`pi-gpt@0.4.3`: `package.json`, `README.md`, `skills/chatgpt/SKILL.md`, `src/auth.ts`,
`src/client.ts`, `src/conversation.ts`, `src/redact.ts`, `src/registry.ts`,
`extensions/chatgpt.ts`, `extensions/observer.ts` (head),
`tests/conversation-continuation.test.ts`. Not read: `src/pow.ts`, `src/sentinel.ts`,
`src/turnstile.ts` (deliberately out of scope), `src/files.ts`, `src/models.ts`,
`src/observer-*.ts`.

`pi-oracle@0.7.20`: `package.json`, `README.md`, `CHANGELOG.md`, `docs/ORACLE_DESIGN.md`
(targeted sections), `docs/ORACLE_RECOVERY_DRILL.md`, `extensions/oracle/lib/tools.ts`,
`extensions/oracle/lib/jobs.ts` (type surface), `extensions/oracle/lib/poller.ts`,
`extensions/oracle/worker/run-job.mjs` (targeted sections),
`extensions/oracle/worker/chatgpt-flow-helpers.mjs`,
`extensions/oracle/shared/state-path-helpers.mjs`.

**Repository context:** `/Volumes/Data/Developer/commandgovernor/docs/adr/0008-adopt-pi-native-command-governor-harness.md`
(§6 lines 120-134, §7 lines 136-142, §8 lines 144-153, foreman protocol lines 230-258,
Gate P4 lines 273-284).
