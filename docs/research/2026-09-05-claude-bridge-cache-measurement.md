# Does a custom ACP client for Claude earn its existence? — cache measurement, 2026-09-05

Status: **executed.** Everything in §2–§6 was measured on this machine on
2026-09-05, three runs per path, in disposable roots, on `claude-haiku-4-5`
only, against the pinned Prime Agent 0.9.1 and the vendored
`pi-claude-agent-sdk` 0.8.6 as patched by
`pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch`. Raw per-run
material — bridge debug logs, pty logs, Prime session JSONLs and the JSON
results the tables are rebuilt from — is preserved with the session that
produced it. §7 records a probe whose output was not kept as a file and says
so.

The prior-art half of this question is a separate document:
[`2026-09-05-claude-acp-client-prior-art.md`](2026-09-05-claude-acp-client-prior-art.md),
which found the design is not novel and named the one measurement that would
decide it. This is that measurement.

## The answer

**A custom ACP client for Claude fails ADR 0010's existence test and is
withdrawn.**

The case for it was that letting Claude own its own session — the ACP design,
where the agent keeps the transcript and the client resumes by id — would keep
the prompt cache warm where the bridge's rebuild-and-resume design cannot. On
clean turns there is nothing to win: the bridge reuses the Claude Code session
and gets **97%** where native `claude -p --resume` gets **99–100%** (§3, §4).
Two or three points on a Haiku turn is not a genuine gap, and ADR 0010 §2
admits no fifth category for "a custom subsystem would be more rigorous".

The measurement did not come back empty. It found two real defects, one of
which is fixed in this branch:

| # | Defect | Disposition |
| --- | --- | --- |
| a | Prime `/compact` failed 3/3 under the bridge; no `compaction` entry ever reached the transcript | **fixed** — fourth seam in the vendored patch; `BRIDGE-005` |
| b | every mid-turn abort costs the whole prompt cache and a new Claude Code session id | **documented cost** — inherent to the bridge's design; README |

and, incidentally, that Prime's `--mode rpc` cannot admit input after either an
abort *or* a compaction (§8), which is filed as
[`docs/upstream/2026-09-05-prime-rpc-queue-suspended.md`](../upstream/2026-09-05-prime-rpc-queue-suspended.md).

The withdrawn spike is preserved as the branch **`acp/claude-session-owner`**
(`f557728`, `d0549a2`), pushed to `origin` with no pull request. It is evidence,
not a plan: nothing on it is proposed for merge.

## 1. Method

One sequence, run on both paths, three times each, on `claude-haiku-4-5`:

```text
1  a prompt that reads two files in the working directory
2  a plain follow-up
-  a long turn, started and interrupted mid-flight
3  a follow-up AFTER that interruption
-  a bulk turn, so the conversation is long enough to be compactable
-  a forced compaction
4  a follow-up AFTER the compaction
```

**Path A — native.** `claude -p --output-format json` with `--resume` against a
session id the harness generated, `/compact` through the CLI. The figures are
the result JSON's own `usage`, and the session id is the one the CLI reports.

**Path B — Prime + the vendored bridge.** The same sequence through the stock
`prime-agent` interactive client on a real pty, with the bridge installed as a
project package and `compaction.keepRecentTokens` lowered to 500 (the supported
way to make a short session compactable; it moves where the cut lands and
nothing else). The figures are the bridge's own debug log: every `usage:` line
and every `syncResult:` line, summed over the model calls a Prime turn makes.
A Prime turn is several Claude Code calls, because Prime's tools round-trip
through the bridge, so `calls` is reported rather than hidden.

Two facts about path B are measurement decisions, not incidental:

- **The interactive client, not `--mode rpc`.** The bridge's cache-preserving
  REUSE path only exists inside one long-lived Prime process, and rpc cannot
  continue past the interruption (§8). Escape in the interactive client is the
  abort a user actually performs, and it recovers.
- **Per-turn figures are sums.** `cachePct` in a single `usage:` line describes
  one Claude Code call. The `hit` column below is
  `cache_read / (input + cache_creation + cache_read)` over the whole turn,
  which is the same definition the bridge's own `cachePct` uses for one call.

Neither path was given an API key. Claude Code ran on its own Max-plan login on
both, which is the only Claude path this product has (`BRIDGE-001…004`).

## 2. Path A — native `claude -p --resume`

| turn | run | input | cache_creation | cache_read | hit | session |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 first turn (reads two files) | 1 | 18 | 14142 | 40146 | 73.9% | `d56aedd5` |
|  | 2 | 18 | 14143 | 40146 | 73.9% | `531e0061` |
|  | 3 | 18 | 14352 | 40146 | 73.6% | `bfece47a` |
| 2 clean follow-up | 1 | 10 | 125 | 27757 | 99.5% | `d56aedd5` |
|  | 2 | 10 | 140 | 27758 | 99.5% | `531e0061` |
|  | 3 | 10 | 126 | 27967 | 99.5% | `bfece47a` |
| 3 follow-up after an abort | 1 | 10 | 196 | 27882 | 99.3% | `d56aedd5` |
|  | 2 | 10 | 214 | 27898 | 99.2% | `531e0061` |
|  | 3 | 10 | 183 | 28093 | 99.3% | `bfece47a` |
| — bulk turn (grows the conversation) | 1 | 10 | 107 | 28078 | 99.6% | `d56aedd5` |
|  | 2 | 10 | 99 | 28112 | 99.6% | `531e0061` |
|  | 3 | 10 | 110 | 28276 | 99.6% | `bfece47a` |
| — forced compaction (`/compact`) | 1 | 0 | 0 | 0 | — | `d56aedd5` |
|  | 2 | 0 | 0 | 0 | — | `531e0061` |
|  | 3 | 0 | 0 | 0 | — | `bfece47a` |
| 4 follow-up after a compaction | 1 | 10 | 12004 | 17797 | 59.7% | `d56aedd5` |
|  | 2 | 10 | 15659 | 17797 | 53.2% | `531e0061` |
|  | 3 | 10 | 12258 | 17797 | 59.2% | `bfece47a` |

One session id for the whole run, every time. The interruption costs nothing:
turn 3 resumes at 99.3%. The compaction costs a re-warm — turn 4 pays 12–15 k
cache_creation for a rewritten prefix — which is the price of compaction
itself, not of any client design. `/compact` succeeded **3/3**; the compaction
rows carry no usage because the CLI reports none for that command.

## 3. Path B — Prime + the vendored bridge

| turn | run | calls | input | cache_creation | cache_read | hit | syncResult | session |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| 1 first turn (reads two files) | 1 | 4 | 40 | 9158 | 8514 | 48.1% | clean-start | (created) |
|  | 2 | 4 | 40 | 9134 | 8514 | 48.1% | clean-start | (created) |
|  | 3 | 4 | 40 | 9114 | 8514 | 48.2% | clean-start | (created) |
| 2 clean follow-up | 1 | 2 | 20 | 254 | 9158 | 97.1% | reuse | `52041100` |
|  | 2 | 2 | 20 | 254 | 9134 | 97.1% | reuse | `b035666a` |
|  | 3 | 2 | 20 | 230 | 9114 | 97.3% | reuse | `8bc306db` |
| — the interrupted turn (Escape) | 1 | 2 | 20 | 324 | 9412 | 96.5% | reuse | `52041100` |
|  | 2 | 2 | 20 | 270 | 9388 | 97.0% | reuse | `b035666a` |
|  | 3 | 2 | 20 | 320 | 9344 | 96.5% | reuse | `8bc306db` |
| 3 follow-up after an abort | 1 | 2 | 20 | 10026 | **0** | **0.0%** | **rebuild** | `278598f7` |
|  | 2 | 2 | 20 | 10128 | **0** | **0.0%** | **rebuild** | `33312493` |
|  | 3 | 2 | 20 | 9898 | **0** | **0.0%** | **rebuild** | `d6b4c81f` |
| — bulk turn (grows the conversation) | 1 | 8 | 80 | 8228 | 41764 | 83.4% | reuse | `278598f7` |
|  | 2 | 4 | 40 | 3780 | 20472 | 84.3% | reuse | `33312493` |
|  | 3 | 8 | 80 | 8322 | 41424 | 83.1% | reuse | `d6b4c81f` |
| — forced compaction (`/compact`) | 1 | 0 | 0 | 0 | 0 | — | — | **failed** |
|  | 2 | 0 | 0 | 0 | 0 | — | — | **failed** |
|  | 3 | 0 | 0 | 0 | 0 | — | — | **failed** |
| 4 follow-up after a compaction | 1 | 2 | 20 | 760 | 14522 | 94.9% | reuse | `278598f7` |
|  | 2 | 2 | 20 | 222 | 13908 | 98.3% | reuse | `33312493` |
|  | 3 | 2 | 20 | 648 | 14484 | 95.6% | reuse | `d6b4c81f` |

Read the `session` column across a run: the id changes exactly once, at the
turn after the abort, and that is the whole of defect (b). Turn 1's absolute
figures are not comparable with path A's — the bridge runs Claude Code with
`tools: []` and `includeGitInstructions: false`, so it ships a smaller prefix —
which is why the comparison that decides the question is the **hit rate on a
clean follow-up**, not the token counts.

Note that turn 4 shows a healthy hit rate on both this table and path A's. On
this path it is not the price of a compaction, because **no compaction
happened** (§5): it is an ordinary reuse turn.

## 4. The verdict on the ACP client

The existence test (ADR 0010 §2) asks what existing capability was evaluated
and whether a genuine gap is proven. The bridge is the existing capability, and
on the axis the ACP client was proposed to improve:

| | clean follow-up (turn 2) |
| --- | --- |
| native `claude -p --resume` | 99.5%, 99.5%, 99.5% |
| Prime + vendored bridge | 97.1%, 97.1%, 97.3% |

The bridge already reuses the Claude Code session and already keeps the cache
warm. A client that let Claude own the session could at best close a 2.4-point
gap on a path that is already 97% warm — and would do it by adding a general
agent-transport subsystem beside Prime, exactly the ownership Command Governor
does not take (ADR 0010 §1). **Disposition: DELETE / DO NOT BUILD.**

The one place the bridge does lose the cache outright is an abort (§6), and an
ACP client does not obviously fix that either: the client would still have to
decide what the agent's session contains after an interrupted turn, which is
the problem the rebuild exists to solve.

## 5. Defect (a) — Prime `/compact` failed under the bridge (fixed here)

**Observed 3/3.** Every forced compaction failed with:

```text
Compaction failed: prompt-capture: no capture for this 317-char system prompt,
and it embeds none of the 1 known. Closest known match diverges at offset 10
(20982-char key). Claude Code would receive none of this turn's context files,
skills or custom instructions.
```

and no `compaction` entry ever reached Prime's session JSONL (0 of 3 runs;
58, 47 and 45 transcript entries respectively, none of type `compaction`). The
error's own suggested cause — another extension rewriting the system prompt —
does not apply: the bridge was the only extension loaded.

**Root cause,** from the bridge's log and both sources:

The bridge routes Pi's nested completions away from the resumable provider
path, and recognises them by a single marker
(`pins/packages/pi-claude-agent-sdk-0.8.6/src/index.ts`, `isStandaloneRequest`):

```ts
return options?.cacheRetention === "none"
    && context.tools === undefined
    && context.messages.length === 1
    && context.messages[0]?.role === "user";
```

Prime's compaction has the shape but not the marker. `prime-agent`
`dist/core/compaction/compaction.js` builds exactly one user message with no
tools and calls

```js
const response = await completeSimple(model,
    { systemPrompt: SUMMARIZATION_SYSTEM_PROMPT, messages: summarizationMessages },
    completionOptions);   // { maxTokens, signal, apiKey, headers }
```

`cacheRetention` is never set, and pi-ai's `StreamOptions` documents its
default as `"short"`. The string `cacheRetention` does not occur anywhere under
`prime-agent/dist/core`. So the request enters `streamClaudeAgentSdk`'s
resumable path, which resolves the system prompt against the capture table
`before_agent_start` fills — and `SUMMARIZATION_SYSTEM_PROMPT` is 317
characters that Prime never assembles through `before_agent_start`. Measured
identity: the constant in `dist/core/compaction/utils.js` is exactly 317
characters, and the bridge log records

```text
prompt-capture: no match for 317-char system prompt. closest known (20982-char)
shares its first 10 chars ... known keys=1
```

immediately after the provider was entered — the `provider: routing standalone`
line never appears.

**Four** more Prime call sites have the same shape and would have failed the
same way: `generateTurnPrefixSummary` (a split-turn compaction, `compaction.js:586`),
`core/compaction/branch-summarization.js:196`, and *both* refinement completions
— `core/refinement/refinement.js:723` (`/refine`) and `:771` (the auto-refine
review). Every one of them passes `{ maxTokens, signal, apiKey, headers }` and a
system prompt of Prime's own.

**Fix — the fourth seam.** `isStandaloneRequest` keeps upstream's marker and
adds a second one: a system prompt the capture table cannot account for is, by
construction, not an agent turn.

```ts
if (options?.cacheRetention === "none") return true;
...
return Boolean(context.systemPrompt) && !promptCaptures.canAccount(context.systemPrompt);
```

with a new non-throwing `PromptCaptures.canAccount` next to `resolveOrDerive`.
The condition is exactly "`resolveOrDerive` would throw on this": a prompt it
would have served — absent, empty, exact, revived or embedded — still takes the
provider path and behaves as upstream.

This does not weaken the prompt-capture guard for the provider path, and the
reason is structural rather than a judgement call. `context.tools === undefined`
already excludes every agent turn: pi-agent-core materialises the agent state's
tools as an array (`createMutableAgentState`: `initialState?.tools?.slice() ?? []`)
and `agent-loop.js` hands that same array to `streamSimple`, so an agent turn is
`tools: []` at worst, never `undefined`. And the standalone path passes
`context.systemPrompt` to Claude Code **verbatim**, so nothing the guard exists
to protect (context files, skills, custom instructions) can be dropped by taking
it.

**An earlier draft of this seam also required `promptCaptures.size > 0`, and
that was wrong.** Prime reaches `emitBeforeAgentStart` only from its user-turn
preparation (`core/agent-session.js`), so a **cold worker** — `prime-agent -r
<sessionFile>` and then `/compact` as its first action — has an empty table. The
size term would have left exactly that path throwing the same 317-char error.
It is removed, and the case is now asserted rather than argued (§9).

**Verified.** `BRIDGE-005`
(`conformance/runtime/claude-bridge-compaction.test.ts`) drives a real Prime
session under the bridge with `keepRecentTokens` lowered and compacts twice: on
the **warm** worker that just ran turns, and on a **cold** worker reopened with
`-r` after the resident worker was SIGKILLed, with `/compact` as its first
action. Each must land a `compaction` entry carrying a non-empty summary in
Prime's own transcript, and the warm phase must answer a turn afterwards. Three
consecutive passes and two negative controls in §9.

**Not covered by `tsc --noEmit`.** `tsconfig.json` has `"include": ["harness/**/*.ts",
"conformance/**/*.ts"]` and `"exclude": [..., "pins"]`, so the vendored source
this seam edits is outside the repository's typecheck — as all three earlier
seams already were. It matters enough to say: a purely static type error in a
seam would reach `main`. What does cover it is that Prime executes these files
under Node's type stripping, so `BRIDGE-005` runs the exact patched module; and
typechecking the vendored `src/` standalone (its own bundled `tsc`, `strict`,
`nodenext`) reports **15 errors, all pre-existing** — two in `convert.ts`, three
in `session-verify.ts`, and ten in `index.ts` of which nine are an
`AssistantMessage | null` cluster and one is the existing `getModels` seam,
whose symbol Prime's `pi-ai` exports at runtime but types differently. **Zero
are in `prompt-capture.ts`, and none is at a line this seam touches.** Making
that tree typecheck clean is an upstream change, not this branch's.

## 6. Defect (b) — an abort costs the whole cache and the session id

**Observed 3/3, and it is by design.** The turn after a mid-turn abort takes
the bridge's REBUILD path with `forceRotate`: `cache_read` is 0, the entire
prior conversation is re-imported into a **new** Claude Code session id, and
the next turn pays ~10 k `cache_creation` (path B, turn 3).

This is not a bug to fix in this branch. The bridge rotates the id deliberately
— an aborted Claude Code child may still be writing to the old session file,
and rebuilding in place would race an orphan writer. The correct response is
operational, and it is now stated in the README: **a Claude worker is allowed
to finish, or is killed — not interrupted.** Interrupting one costs the whole
prompt cache and starts a new Claude Code session.

## 7. 1M context on this account

Probed with `one-m-probe.mjs` (one `claude -p --output-format json` call per
case, reading the context window the result reports rather than inferring it).
The probe's stdout was read at the time and not preserved as a file; what is
recorded here is that reading, and it is the one claim in this document without
a retained artefact.

| model id | result |
| --- | --- |
| `claude-fable-5-1[1m]` | works with no credential: `contextWindow` 1000000, no error |
| `claude-sonnet-4-6[1m]` | fails: "Usage credits required for 1M context" |
| `claude-fable-5-1[1m]` with `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` | clamped to 200K |

Sonnet 4.6's 1M window needs extra usage, which is disabled on this account
(the rate-limit events in the bridge logs report
`overageStatus: "rejected"`, `overageDisabledReason: "org_level_disabled_until"`).
Fable 5.1's does not. `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` clamps an explicit
`[1m]` id, which is the answer to whether the bridge's long-context setting can
be turned off from the environment: it can.

This is a fact about the account, not a recommendation. Model routing for this
machine is set by policy elsewhere, and Fable is opt-in by name only.

## 8. Incidental — Prime `--mode rpc` cannot admit input after an abort or a compaction

Found while building path B, and confirmed again while building `BRIDGE-005`.
In `--mode rpc`, Prime suspends the session input queue when a turn is aborted,
and again across a compaction. Every later command is refused:

```json
{"id":"t3","type":"response","command":"prompt","success":false,
 "error":"Cannot admit a session action while queued session input is suspended."}
```

`agent_messages_resume` does not lift it; only the daemon protocol exposes
`resume_queue`, which no stock client sends. The interactive client recovers
from both. Filed as
[`docs/upstream/2026-09-05-prime-rpc-queue-suspended.md`](../upstream/2026-09-05-prime-rpc-queue-suspended.md),
and it is why `BRIDGE-005` drives a pty rather than rpc.

## 9. What can come back negative

Each claim above has the observation that would falsify it:

| Claim | What would falsify it |
| --- | --- |
| the bridge keeps the cache warm on clean turns | a `syncResult: path=rebuild` or a 0% hit on turn 2 — did not occur in 3 runs |
| compaction was broken | a `compaction` entry in any of the three transcripts — 0 of 3 |
| the seam fixes it | `BRIDGE-005` passed three times in a row; with the seam disabled in the extracted package and nothing else changed, the **warm** phase timed out at 300 s with `Command failed: prompt-capture: no capture for this 317-char system prompt, and it embeds none of the 1 known` on the client's screen |
| the `size > 0` term had to go | restoring only that term, with the rest of the seam intact, passes the warm phase and fails the **cold** one at 300 s with `it embeds none of the 0 known` — the empty capture table, named by the count |
| an abort loses the cache | a non-zero `cache_read` on turn 3 — 0 in all three runs |

One measurement in this document was itself unable to come back negative for a
while, and it is worth recording. `BRIDGE-005`'s bulk prompt is 2126 bytes, and
`conformance/lib/ptyrun.py` wrote keystrokes to the pty master in one
`os.write()` whose return value was discarded. A raw-mode pty slave holds
**1022** bytes — measured directly on this machine: 1022 goes through, 1023
blocks — so that write parked the runner in the kernel, and the runner is the
only thing draining the TUI's output. Whether the pair deadlocked was a race the
TUI sometimes won. Three passing runs and one correctly-failing control were
therefore luckier than they looked, and an independent reviewer's run of the
same test hung on turn 2 without ever reaching `/compact` — which also silently
disarms the negative control. The runner now buffers keystrokes, writes at most
512 bytes at a time on a non-blocking master (attempting a write every pass and
parking in `select()` only while the queue is full), advances by what
`os.write()` actually took, and keeps draining output between chunks.

## Sources

**This repository**
- `pins/packages/pi-claude-agent-sdk-0.8.6.tgz` → `src/index.ts`
  (`isStandaloneRequest`, `runStandaloneRequest`, `syncSharedSession`),
  `src/prompt-capture.ts` (`resolveOrDerive`, the throw text)
- `pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch` (the four seams)
- `conformance/runtime/claude-bridge-compaction.test.ts` (`BRIDGE-005`)
- `conformance/runtime/claude-bridge-boundary.test.ts` (`BRIDGE-001…004`)

**Prime Agent 0.9.1, as installed under `pins/prime-0.9.1/`**
- `dist/core/compaction/compaction.js` — `generateSummary`,
  `generateTurnPrefixSummary`, `DEFAULT_COMPACTION_SETTINGS`, `findCutPoint`
- `dist/core/compaction/utils.js` — `SUMMARIZATION_SYSTEM_PROMPT` (317 chars)
- `dist/core/compaction/branch-summarization.js`, `dist/core/refinement/refinement.js`
- `@earendil-works/pi-ai` `dist/types.d.ts` — `StreamOptions.cacheRetention`,
  default `"short"`; `dist/stream.js` — `completeSimple`

**Withdrawn spike**
- branch `acp/claude-session-owner` (`f557728`, `d0549a2`) on `origin`, no PR
- [`2026-09-05-claude-acp-client-prior-art.md`](2026-09-05-claude-acp-client-prior-art.md)
