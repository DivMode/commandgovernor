# Driving Claude Code as an ACP client: prior art and adapter limits — 2026-09-05

Status: **research, not executed.** Every claim marked *Verified* below was read
from a primary source (repository source, npm registry metadata, GitHub issue
body, or vendor documentation) at its revision on 2026-09-05 and is quoted with
its link. Claims marked *Hypothesis* are reasoning that has not been measured on
this machine. No measurement of cache or usage behaviour was run for this
document; §6 says exactly what would have to be measured.

Version sensitivity is high. `@agentclientprotocol/claude-agent-acp` published
**six releases in five days** (0.71.0 through 0.75.1, 2026-08-31 to 2026-09-05).
Every adapter claim below is pinned to a version.

## The answer

**No, we are not first, on any axis taken separately.**

- Driving Claude Code over ACP as a client is a large, mature ecosystem —
  well over a hundred clients are listed on the protocol's own site.
- Driving Claude Code over ACP *from another agent harness*, as a delegated
  sub-agent, is established prior art: OpenClaw, Mastra and AgentPool all do it,
  and OpenClaw additionally resumes the external agent's own session by id.
- Letting Claude own its session id and transcript is not a design we invented;
  it is what the ACP specification *requires* of any agent, and what the adapter
  implements by handing the ACP session id straight to the Agent SDK's `resume`.
  **The reason is published too**, by the protocol's lead maintainer in 2025:
  a client "can't necessarily reconstruct the state" post-`/compact`, so session
  state "really needs to be owned by the agent" (§2.1). Essentially every serious
  client — Zed, goose, CodeCompanion, agentic.nvim, agent-shell, acpx, patchbay,
  Tidewave — restores via `session/resume`/`session/load` rather than replaying
  its own history.
- Even inside the Pi family it is done: `pi-harness-delegate` 0.6.1 is a
  maintained generic ACP client with verified `session/load` (aimed at OpenCode
  and Devin), and **two Pi extensions already drive `claude-agent-acp`
  directly** — `sathish316/pi-omniagent-extensions` and `@junghanacs/pi-shell-acp`.
  The only piece nobody has assembled is *reattaching to Claude's own session*:
  the extension with `session/load` doesn't point it at Claude, and the ones
  pointed at Claude never resume.

**The motivation is published too — just not pointed at ACP.** cline/cline
Discussion #9892 (2026-03-19, unanswered) states the whole chain: a harness that
re-sends full history gets `cache_read_input_tokens` "always 0", and "Session
limits are hit much faster on Claude Code subscriptions". Its recommended fix is
"Session Resume … the CLI … loads conversation history from its local session
store", i.e. **let Claude own the session** — our argument, one step short of our
conclusion. Anthropic supplies the missing premise in its own words
(2026-04-30): "A high prompt cache hit rate … helps us create more generous rate
limits for our subscription plans." What nobody has written is *therefore drive
it over ACP*, and **nobody anywhere has published a head-to-head measurement.**

So the honest claim is narrow, and narrower than it looked at first draft:
**not first to the mechanism, not first to the reasoning, not first to the
session-ownership rationale, plausibly first only to the specific conjunction —
and first to the numbers only if we actually produce them.** Two of the three
venues most likely to contain a counterexample (Reddit, X) were unreachable, so
even that is provisional (§4).

One correction to the premise before anything else, because it changes the case
we should make: the bridge does **not** lose the prompt cache on every turn. It
has an explicit reuse path that keeps Claude's cache warm. What it loses the
cache on is *divergence* — and Pi-side `/compact` is a divergence. §1 has the
source.

---

## 1. What is actually being replaced (grounding the premise)

*Verified.* Read from the vendored tarball in this repository,
`pins/packages/pi-claude-agent-sdk-0.8.6.tgz` (`pi-claude-agent-sdk` 0.8.6, npm
integrity `sha512-nVyqRVm5yu78K8Tb…`, `gitHead`
`5293c03fc1e250725c9e23472eec767a5a302caf`, pinned in `pins/pins.json` under
authority `claude-model-provider`).

The bridge does not merely re-send history. It **writes Claude Code's own
session JSONL file** and then calls Claude with `--resume` against it. From
`package/src/index.ts`, the comment block above `syncSharedSession` (lines
602–626):

> Two semantic paths:
>   REUSE — pi's history is in sync with the existing sharedSession … Returns
>   the existing sessionId. **Keeps CC's prompt cache warm.**
>   REBUILD — no session yet, or pi's history has diverged (non-trailing missed
>   messages, e.g. another provider took a turn). **Wipes the existing session
>   file (if any) and writes a fresh one containing all prior messages**,
>   reusing the same sessionId across rebuilds so UUIDs stay stable …

and on why a rebuild rather than a patch:

> Injecting deltas into an existing session creates a branch that CC's
> `--resume` doesn't follow (documented attempt prior to this). A complete
> overwrite at the same path is simpler and correct.

The REBUILD path is not hypothetical bookkeeping — it calls
`deleteSession(previousSessionId, cwd, …)` and then `createSession(…)` and
re-imports every prior message (`convertAndImportMessages`).

**When does it rebuild?** From the same file (lines 665–667):

> Only reachable when `needsRebuild` is false — user-facing history rewrites
> (**`/compact`**, `session_tree`, `/new`, fork) always set `needsRebuild` or
> clear `sharedSession` before the next `syncSharedSession` call.

plus a post-abort rotation and a steer-miss (`provider: steer never reached CC,
marked session for rebuild`, line 1350).

So the accurate statement of the problem is:

| | Bridge (`pi-claude-agent-sdk` 0.8.6) | ACP (`claude-agent-acp` 0.75.1) |
| --- | --- | --- |
| Who mints the session id | the bridge (`createSession`, preserved across rebuilds) | **Claude** — ACP requires the agent to return it |
| Who writes Claude's transcript | **the bridge**, wholesale, on divergence | Claude only; the adapter reads it |
| Who compacts | **Pi**, and the result forces a rebuild | **Claude**, reported outward as a tool lifecycle |
| Steady-state turn | REUSE — cache stays warm | cache stays warm |
| After a compaction | full wipe + full re-import + cold prefix | Claude's own compaction; new prefix, no re-import |

*Hypothesis (unmeasured).* The cost difference is therefore concentrated at
divergence events, not spread over every turn. The dominant one in real use is
compaction, and it is doubly expensive under the bridge: Pi compacts (paying its
own summarisation), then the next Claude turn discards and rewrites Claude's
entire session file, so Claude re-reads a history Pi has already paid to
summarise. Under ACP, compaction happens once, inside Claude, on Claude's own
prefix. **Nobody has measured this, us included.**

*Verified, and it removes one axis from the comparison.* Authentication is
**not** a differentiator. Upstream, the bridge "requires an Anthropic OAuth
credential (or API key) configured in Pi" and "Claude Code login and inherited
Claude/Anthropic authentication settings are deliberately ignored"
(`package/README.md`). This repository's committed patch
(`pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch`, recorded in
`pins/pins.json`) already inverts that: it strips every inherited
Claude/Anthropic variable from the child and refuses any Anthropic credential
the harness resolves, so "the child can only authenticate with Claude Code's own
login (macOS Keychain)". The ACP adapter reaches the same place by default
(§5.4). Both ends of the migration are subscription-only already.

---

## 2. Who already drives Claude Code through ACP as a client

*Verified.* The adapter is **`@agentclientprotocol/claude-agent-acp`**, v0.75.1,
published 2026-09-05. It was renamed out of the Zed namespace: npm lists
`@zed-industries/claude-agent-acp` (last 0.23.1) and the older
`@zed-industries/claude-code-acp` (last 0.16.2) as superseded. Repository
`github.com/agentclientprotocol/claude-agent-acp`, Apache-2.0, created
2025-08-27, 2,493 stars, **164 open issues**, pushed 2026-09-05.

Its README's own framing is the tell that a client ecosystem is assumed:

> Use Claude Agent SDK from **any ACP client**

*Verified.* The protocol's client directory
(<https://agentclientprotocol.com/overview/clients>) lists well over a hundred
clients across editors, CLI/TUI, desktop, notebooks, mobile and messaging —
including JetBrains, CodeCompanion, avante.nvim, agentic.nvim, hermes.nvim,
`agent-shell.el` (Emacs), several VS Code extensions, and CLI/TUI entries
`acpx`, `Hash`, `Hydra`, `Martty`, `Nori CLI`, `pool` and `Toad`. **Being an ACP
client that drives Claude is unremarkable.**

The clients that matter for us are the ones that are *harnesses themselves*, and
those that resume Claude's own session:

| Client | ACP client? | Drives `claude-agent-acp`? | Session ownership / `session/load` | Stated rationale |
| --- | --- | --- | --- | --- |
| **Zed** | yes (reference) | yes | yes; adapter advertises `loadSession: true` | editor integration; no cache/usage rationale found |
| **JetBrains** | yes | yes | via the ACP registry entry | — (see auth note below) |
| **OpenClaw** | yes | yes | **yes, explicitly** | "specialized capabilities beyond OpenClaw's native sub-agent runtime" |
| **Mastra** | yes | yes (`npx -y @agentclientprotocol/claude-agent-acp`) | not documented | delegate to "a harness with specialized tools you'd otherwise have to build yourself" |
| **AgentPool** | yes (and server) | yes | not documented | "acts as BOTH an ACP server AND an ACP client simultaneously" |
| **Alas** | yes (native macOS) | yes | yes — `session/load`, persistence, forks, remote broker | workspace/worktree UX; no cache rationale found |

**OpenClaw is the closest published prior art to our design.** Its ACP docs
(<https://open-claw.bot/docs/tools/acp-agents/>) say:

> Agent Client Protocol (ACP) sessions let you run **external coding harnesses
> like Pi, Claude Code, Codex, OpenCode, and Gemini CLI** through an ACP backend
> plugin.

and, on resumption:

> If you want to continue where you left off, use `resumeSessionId`. This tells
> the agent to **replay its conversation history using `session/load`**.

That is a harness driving Claude Code over ACP *and* resuming Claude's own
session by id — the mechanism we are proposing. It states no cache or
usage-limit reason for it.

**Mastra** (blog post dated 2026-06-02,
<https://mastra.ai/blog/introducing-agent-client-protocol>) runs Claude as a
delegated sub-agent under a supervisor whose description is "Plans and delegates
coding tasks to Claude Agent", spawned with
`"npx", ["-y", "@agentclientprotocol/claude-agent-acp"]`. Rationale quoted:
"Delegate coding tasks to a harness with specialized tools you'd otherwise have
to build yourself: codebase reasoning, file editing, shell execution,
self-correcting test loops, and Git operations." No session, cache or usage
discussion.

**Alas** (<https://github.com/mrmans0n/alas>, Swift, pushed 2026-09-05) is a
non-editor ACP host with `ACPSessionPersistence`, `ACPSessionStore`,
`ACPSessionHydrator`, fork persistence and a Rust `acp_broker_process`. Code
search across the repository for "prompt cache", "cache_read", "session
ownership" and "usage limit" returns **nothing** — it implements session
ownership without ever citing the reason we are citing.

### 2.1 The rationale *is* published — by the protocol's own maintainer

*Verified, and it corrects this document's first draft.* I claimed no one states
why session ownership matters. That was wrong, and the statement is the clearest
articulation of our own compaction argument. From
[claude-agent-acp#80](https://github.com/agentclientprotocol/claude-agent-acp/issues/80)
("Thread persistence", opened 2025-10-01, closed 2026-02-18), Ben Brandt (Zed,
ACP lead maintainer), **2025-10-09**, answering "Why not store the threads in zed
rather than relying on the backend for persistence?":

> Because the agent may have state required to resume that isn't just the chat
> representation over ACP. Since it is a design decision that the ACP
> representation isn't necessarily the same as the agent's own representation, to
> allow for flexibility for agent authors, **it really needs to be owned by the
> agent**.
>
> For example: **post `/compact` in multiple agents they can handle this
> differently, and we can't necessarily reconstruct the state based on the UI
> state**

That is §1's compaction-ownership argument, made ten months before us, by the
person who maintains the protocol. It is argued from *correctness* — the client
cannot faithfully reconstruct post-compaction state — not from cache economics.
Which is, as it happens, where §6 recommends we put our own emphasis.

*Verified.* Other clients state the same property in their own words:

- **Zed**: "the External Agent usually owns its own runtime, auth, model
  selection, tools, and native configuration" (<https://zed.dev/docs/ai/external-agents>).
- **CodeCompanion.nvim**: "ACP adapters are **stateful**. The agent maintains the
  conversation context, so CodeCompanion only sends new messages with each prompt."
- **agentic.nvim**: "**Sessions are interchangeable** — start a conversation in
  Neovim and continue it in the terminal… Your ACP provider manages sessions
  natively."
- **acp-patchbay**: "The agent owns the sessions — 100%… patchbay persists no
  session records at all."
- **acpx** (an ACP CLI whose stated primary user is "**another agent,
  orchestrator, or harness**").

*Verified, and it is the closest published statement of our economic motive.*
**goose** (block) drives `claude-agent-acp` as a provider
(`crates/goose/src/providers/claude_acp.rs`) and says: "ACP providers let you use
goose with your existing Claude Code or ChatGPT Plus/Pro subscriptions — **no
per-token API costs**." Its PR #10379 (2026-08-10) is titled "Resume
provider-native ACP sessions **instead of replaying Goose's stored transcript**".

*Verified — and this is the only real measurement of the replay failure mode
found anywhere.* goose issue
[#10764](https://github.com/aaif-goose/goose/issues/10764) ("ACP: uncapped
conversation replay on session restore makes long sessions permanently
unresumable", closed 2026-07-28):

> When an ACP session is restored after being evicted, `AcpProvider` replays the
> entire prior conversation into the next prompt as a single "handoff context"
> text block. There is no token budget, no truncation, and no summarization — a
> long session produces one enormous `session/prompt`, which the provider rejects
> with `Prompt is too long`.
>
> Because the same memo is rebuilt on every restore, **the session is permanently
> unrecoverable**: each attempt to resume fails identically.

That is the harness-owns-history architecture failing hard, reproduced, with a
fix that is precisely "let the agent own the session". It is *not* a cache
measurement — but it is a stronger argument against transcript replay than the
cache argument is, because the failure is total rather than economic.

### 2.2 `session/load` and `session/resume` are different methods

*Verified, and it changes the recommendation.* The spec defines two restore
paths, and the distinction is exactly the one that matters for cost:
`session/load` replays the whole conversation to the client as `session/update`
notifications, whereas `session/resume` **"MUST NOT replay the conversation
history"** (stabilised in the ACP spec 2026-04-22 via the Session Resume RFD).
The adapter advertises both — `loadSession: true` plus `sessionCapabilities`
including `resume`, `list`, `fork`, `close` — and both funnel into the same
`createSession(…, { resume: params.sessionId })`.

Clients have independently converged on a ladder: `resume` → `load` → bounded
handoff. `acpx` calls `load` with `suppressReplayUpdates: true` because it has no
transcript UI; Emacs `agent-shell` defaults to `resume` because "no message
replay… restore is fast and quiet"; `acp-patchbay` always uses `load` because it
deliberately persists nothing.

*Verified.* **JetBrains is the informative negative case**, and it is about
licensing, not capability. In adapter issue
[#517](https://github.com/agentclientprotocol/claude-agent-acp/issues/517)
("Authentication required: This integration does not support using claude.ai
subscriptions", open since 2026-04-07, 21 comments), a JetBrains engineer states:

> unfortunately JetBrains is not allowed to distribute Claude Code version
> that's logging in via Claude subscription. … it goes against Anthropic's user
> agreement

while the same thread records that Zed users drive the same adapter on a
subscription without trouble, and that the workaround inside JetBrains is to
register a custom agent running the adapter directly:

> ```json
> { "agent_servers": { "Claude Code": { "command": "npx",
>   "args": ["@agentclientprotocol/claude-agent-acp"] } } }
> ```
> It will default to using your claude's login method so you must first
> `/login` and select "Claude account with subscription"

The constraint JetBrains hit is a *redistribution* constraint. It does not
apply to a local, first-party install driving the user's own login.

---

## 3. Pi-family ACP client prior art

*Verified.* **Prime Agent's ACP support is the agent (server) side only.** Its
own documentation (`packages/coding-agent/docs/acp.md`) says:

> ACP mode makes Prime Agent an Agent Client Protocol **agent**, speaking
> JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout.

There is no `session/load` in its supported-method table and no client side.
Prime accepts `session/new.mcpServers`, but that makes it an MCP *consumer*, not
an ACP client. This matches this repository's own earlier survey
(`docs/research/2026-09-01-agent-harness-landscape-and-substrate-bakeoff.md`,
line 333: "`prime-agent --mode acp` exposes stable ACP over NDJSON/stdin/stdout")
and ADR 0009's framing of ACP as the *public agent-client boundary* — i.e. the
edge where Prime is driven, not where it drives.

So a Prime-as-ACP-*client* extension is **new code in the Pi family**, and the
direction of the arrow is the whole point: today Prime is the thing at the far
end of an ACP pipe (Alas and OpenClaw both list Pi among the harnesses they
drive); we want Prime holding the near end.

*Verified, and this is the most important find in the document.* **A Pi-family
generic ACP client already exists — and it deliberately excludes Claude.**

`pi-harness-delegate` v0.6.1 (published 2026-08-30, `github.com/yorch/pi-harness-delegate`)
is a Pi extension described as:

> Delegate work to any harness (Claude Code, Muse, OpenCode, Amp) from the pi
> coding agent — code reviews, plans, implementation, security audits, docs, or
> your own custom templates.

It contains a real, general ACP **client**: `extensions/acp-runner.ts`, 370
lines, whose header reads:

> Sibling to `runner.ts` for harnesses whose `transport` is `'acp'` … ACP is
> bidirectional and stateful: the runner must drive a handshake (`initialize` ->
> `session/new` -> `session/set_mode` -> `session/prompt`) and hold stdin open
> for the session's lifetime … It must also answer requests the agent sends back
> to us (permission prompts, fs reads) so the session doesn't hang
>
> Deliberately general: an agent's mode ids and result shape live in its
> `Harness` … this file only knows the ACP wire protocol.

It implements `session/load` resume, and documents the shape difference we would
also have to handle:

> `session/load` resumes a prior session by id (its response carries no
> `sessionId` of its own … `session/new` mints a fresh one. **Verified live:
> `loadSession` is advertised in `agentCapabilities` and a real `session/load` +
> follow-up prompt round-trips**

**But it does not drive Claude over ACP, and the reason is a mistake we can
learn from.** In `extensions/harnesses/claude.ts`:

```ts
  // No `acp` subcommand exists (docs/acp-harness-assessment.md §2) — confirmed against the full
  // `claude --help` output, not just an earlier probe.
  supportsTransports: ['stdout'],
```

and the README's transport section confirms it:

> Every harness runs over its native CLI's stdout (`stdout`, the default and
> **only option for `claude`/`codex`/`amp`**). `opencode` and `devin` also speak
> ACP … Devin ships ACP-only … `amp`/`omp` has a real `acp` subcommand too

The check is correct about the `claude` binary and wrong about Claude. Every
other harness in that table exposes ACP as a **CLI subcommand** — `devin acp`,
`opencode acp`, `omp acp` — so the author probed `claude --help` for the same
shape, found nothing, and closed the question. Claude's ACP support does not ship
as a subcommand: it ships as a **separate npm package**,
`@agentclientprotocol/claude-agent-acp`, spawned as `npx @agentclientprotocol/claude-agent-acp`
(§2 quotes the JetBrains thread doing exactly that). The pattern-match failed on
the one harness that packaged it differently.

*Verified.* The other Pi-family ACP package runs the other way. `pi-acp` v0.0.33
(published 2026-07-30) is the **agent** side:

> `pi-acp` communicates ACP JSON-RPC 2.0 over stdio **to an ACP client (e.g. Zed
> editor)** and spawns `pi --mode rpc`, bridging requests/events between the two.

Notably it solves the session-ownership problem in the opposite direction, and
by hand: "pi stores its own sessions in `~/.pi/agent/sessions/…`; `pi-acp` stores
a small mapping file at `~/.pi/pi-acp/session-map.json` so `session/load` can
reattach to a previous pi session file." That is a side-table because Pi's
session ids are not ACP session ids. The Claude adapter needs no such table
(§5.1) — Claude's id *is* the ACP id.

*Verified, and it corrects a claim this document made in an earlier draft.*
**Two Pi extensions already drive `@agentclientprotocol/claude-agent-acp` as ACP
clients.** The "Pi × Claude over ACP" square is *not* unclaimed.

- **`sathish316/pi-omniagent-extensions`** — `claude-code-acp.ts`, 910 lines,
  last pushed 2026-07-26, 5 stars, 2 open issues, **no LICENSE file**. Its header
  says "Bridges pi to Claude Code through `@agentclientprotocol/claude-agent-acp`".
  It imports `ClientSideConnection` from `@agentclientprotocol/sdk` and
  `nodeToWebReadable, nodeToWebWritable` from the adapter package, spawns the
  adapter (line 409) and connects (line 432). The repository description is the
  closest anyone comes to our motivation in public:

  > Pi coding agent extensions that helps you connect to all coding agents from
  > one place using ACP and **maximize your AI credits** across Cursor, Codex,
  > Claude Code, Rovo

  It also independently reaches this repository's subscription-only rule, at
  line 412–417:

  ```ts
  // …unlike the interactive CLI — honours ANTHROPIC_API_KEY without asking.
  env: { ...process.env, ANTHROPIC_API_KEY: undefined },
  ```

- **`@junghanacs/pi-shell-acp`** v0.11.1 (published 2026-06-29) — "ACP bridge
  providing Claude Code, Codex, and Gemini CLI backends to pi-coding-agent". It
  depends *directly* on `@agentclientprotocol/claude-agent-acp@0.39.0` and
  `@agentclientprotocol/sdk@0.22.1`. Both pins are far behind (adapter 0.75.1,
  SDK 1.4.0) and the package is superseded by the author's `entwurf`.

**The one thing neither does is resume.** `grep -c "loadSession\|session/load"`
over `claude-code-acp.ts` returns **0** — it calls `newSession` only (lines 464,
592, 640, 686). So Claude owns its session *within a process lifetime*, and
nothing reattaches to it afterwards. `pi-harness-delegate` has the resume
machinery but points it at Devin and OpenCode; the Claude extensions have the
target but no resume.

**So the honest Pi-family position is:** every component of the design exists
somewhere in the Pi ecosystem, and no one has assembled them. A generic ACP
client with live-verified `session/load` (`pi-harness-delegate`, but not aimed at
Claude); two ACP clients aimed at Claude (but with no `session/load`); and a
credit-economics motivation stated as a repo tagline but never as an argument or
a measurement. The unclaimed square is not "Pi drives Claude over ACP" — it is
**"Pi drives Claude over ACP *and reattaches to Claude's own session*"**.

*Verified, and it makes a claim in this repository wrong.* `pins/pins.json`
carries an unassigned concern that now misstates the landscape:

```json
{ "concern": "acp-boundary", "status": "unassigned", "phase": "interop",
  "plannedOwner": "Prime's stable ACP v1 server when a shipped path needs it",
  "note": "Interoperability, not internal authority. Prime has no ACP client;
           driving another ACP agent from Prime is an upstream contribution." }
```

The first half ("Prime has no ACP client") is **confirmed**. The second half
("an upstream contribution") is **wrong**, and wrong in a way that would have
mis-scoped the work: upstream Pi declined ACP twice on the record — issue #836
(2026-01-19, a working `--mode acp` PR, closed same day: "I do not want to a
dedicated apc.mode in pi for now. I think this should be build as a project
separate from pi-mono") and issue #7320 (2026-07-30, the **client-side** ask
specifically, closed `not_planned`). There is no upstream path to contribute to.
The supported seam is the **extension** mechanism, and at least eight published
packages already use it. **This entry should be retargeted** as a separate change;
this document does not touch it.

*Verified.* **There is also a first-class client library to pin.**
`@agentclientprotocol/sdk` v1.4.0 (Apache-2.0, published 2026-08-20, repository
`github.com/agentclientprotocol/typescript-sdk`) ships the client half, not just
the agent half. From `dist/acp.d.ts`:

```
export declare class ClientSideConnection implements Agent { … }
    newSession(params: schema.NewSessionRequest): Promise<schema.NewSessionResponse>;
    /** This method is only available if the agent advertises the `loadSession` capability. */
    loadSession(params: schema.LoadSessionRequest): Promise<schema.LoadSessionResponse>;
export declare class ClientApp { … }
export declare class ClientContext extends AcpContext { … }
export declare class SessionBuilder { … }
export declare class ActiveSession { … }
```

**Pinning note, and it matters more than the API surface.** The two things move
at completely different speeds:

| Package | Version | Published | Cadence |
| --- | --- | --- | --- |
| `@agentclientprotocol/sdk` | 1.4.0 | 2026-08-20 | stable 1.x, semver |
| `@agentclientprotocol/claude-agent-acp` | 0.75.1 | 2026-09-05 | **0.x, six releases in five days** |

Pin the SDK with confidence. The adapter is a 0.x moving daily against a
`@anthropic-ai/claude-agent-sdk` that also moves (0.3.257 in 0.75.1; the
changelog shows SDK bumps in 0.72.0, 0.73.0, 0.71.0, and 0.66.x). Treat the
adapter as a tracked dependency with a re-verified pin, not a set-and-forget one.

*Verified by inspection, not by execution.* Everything in this section was read
from the published tarballs (`npm pack pi-harness-delegate@0.6.1`,
`pi-acp@0.0.33`) and from Prime's own docs. **Nothing here was run.** The claim
that `pi-harness-delegate`'s ACP runner would work against `claude-agent-acp` if
given the right spawn command is a *hypothesis* — its `acpView`/`Harness`
abstraction looks general enough, but Claude's adapter has permission and mode
dialects (§5.5) that no existing harness in that extension exercises.

*Unverified.* Oh My Pi's `omp-claude-bridge` is not published on npm under that
name (404). `omp acp` is real — `pi-harness-delegate`'s README says so — but that
is again the agent side.

---

## 4. Has anyone published this motivation?

*Verified, in the negative sense that a search can support.* No statement of the
motivation — "drive Claude Code from another harness over ACP so Claude keeps
its own session, to preserve the prompt cache or a subscription allowance" — was
found in:

- the adapter's tracker (164 open issues; searches for `prompt cache`, `cache`,
  and auth/keychain terms return **no** matching open issues, while `session/load`
  and `cancel` return many),
- Alas's repository (no hits for "prompt cache", "cache_read", "session
  ownership", "usage limit"),
- the public write-ups of OpenClaw, Mastra or AgentPool,
- the bridge's own README, which discusses billing but not cache economics.

**Correction, and it is the biggest one in this document: the argument *is*
published — just not with ACP as the answer.**

*Verified.* [cline/cline Discussion
#9892](https://github.com/cline/cline/discussions/9892), "Enable Prompt Caching
for Claude Code Provider (Persistent Session Architecture)", by `cmaga`,
**2026-03-19**, **0 replies**. It is a Discussion rather than an issue, which is
why issue-only searches miss it. It states the mechanism, the cost, and the fix:

> When using "Claude Code" as the API provider in Cline, **prompt caching does
> not work**. This results in: Session limits are hit much faster on Claude Code
> subscriptions (**Max $200/mo users can lose ~2hrs/day to rate limits**) …
> Suboptimal experience compared to Claude Code's native tools … which all
> achieve ~90-96% cache hit rates

> The current implementation spawns a **new** `claude` CLI process for every
> single message … **Full context re-sent every time**

> It correctly reads cache stats from responses (`cache_read_input_tokens`,
> `cache_creation_input_tokens`) — **but these are always 0** due to the
> architecture above.

and proposes exactly our remedy: "Option A: Session Resume with Incremental
Messages (Recommended) … the CLI … loads conversation history from its local
session store … The API request it constructs will have an identical prefix";
"Option B: Persistent Process via Agent SDK Protocol".

So the published state is: *spawn-per-message → `cache_read` = 0 → subscription
limits hit faster* is **published**; *fix = let Claude Code own the session* is
**published, in the same post**; *therefore use ACP* is **still unpublished**.
The "~2hrs/day" figure carries no logs and should not be repeated as a
measurement. Note also that this describes a **worse** architecture than our
bridge: cline spawns per message, whereas `pi-claude-agent-sdk` holds a session
and reuses it (§1). The published problem is not quite the problem we have.

*Verified, and it is the citation that actually earns the argument.* Anthropic's
own engineering post, "Lessons from building Claude Code: prompt caching is
everything", Thariq Shihipar, **2026-04-30**:

> A high prompt cache hit rate decreases costs and **helps us create more
> generous rate limits for our subscription plans**, so we run alerts on our
> prompt cache hit rate and declare SEVs if they're too low.

That is Anthropic linking cache hit rate to subscription rate limits in its own
words — the load-bearing premise, from the vendor, rather than from us.

*Verified.* The only quantified third-party "a middlebox destroyed the cache"
datapoint found: LiteLLM's incident report for 4–10 July 2026 (v1.91.0/1.91.1),
where moving `role: "system"` entries invalidated cache breakpoints —

> warm-session cache hit rates dropped from roughly **90% to 25-45%** and team
> daily spend rose **2-3x** for the same usage. Requests kept returning 200s

It is request-mangling rather than history replay, so it does not transfer
directly. It does establish the order of magnitude at stake when a layer between
the harness and the API disturbs the prefix.

Two further near misses deserve credit rather than being written out:

- `sathish316/pi-omniagent-extensions` (§3) sells itself on ACP to "maximize your
  AI credits across Cursor, Codex, Claude Code, Rovo". That is a subscription-
  economics motivation for ACP, published. It is about **spreading load across
  several vendors' subscriptions**, not about preserving prefix lineage inside
  one, and it carries no measurement — but it is the closest published statement
  found, and anyone claiming novelty here should quote it first.
- The bridge's own code comment — "Keeps CC's prompt cache warm" — is an
  implementation note defending the REUSE path, not a published architectural
  argument for ACP.

**Scope limit on that negative, stated so it is not over-read — and it is a real
hole, not a formality.** Two of the three likeliest places were **unreachable**,
not searched-and-empty:

- **Reddit** (r/ClaudeAI, r/ClaudeCode, r/LocalLLaMA): domain blocked,
  `search.json` returned 403, mirrors empty. **Unsearched.**
- **X/Twitter**: 402; the usual read-only mirror is dead. **Unsearched.** One
  on-topic-looking piece (Paweł Huryn, ~2026-04-25, "Claude Code's Limits Are
  Generous. The Problem Is Your Harness.") could not be read at all.
- Hacker News was reachable and sampled.

Note that the strongest find in this section — cline #9892 — was a *Discussion*,
invisible to issue search, and sat unanswered for five months. That is direct
evidence that this literature hides in exactly the venues we could not reach.
Treat "unpublished" as **"not found where we could look"**. A single Reddit
thread with real cache numbers would override this section and save us the
measurement in §6; someone should check by hand.

*Verified.* **No measurement of cache or usage-limit impact was found anywhere.**
The bridge ships integration tests named `int-cache.sh` and
`int-session-resume.mjs` (referenced in `index.ts` line 625 and the README's test
section), so the author instrumented the behaviour, but no numbers are published.

**The primary sources that make the argument checkable** — and that bound how
strong it can be:

*Verified.* Anthropic, "How Claude Code uses prompt caching"
(<https://code.claude.com/docs/en/prompt-caching>, read 2026-09-05):

> The API caches by matching the start of each request, called the prefix …
> **The match is exact, so a change anywhere in the prefix recomputes everything
> after it.** There is no per-file or per-segment caching.

> `cache_read_input_tokens` — Tokens served from cache on this turn, **billed at
> roughly 10% of the standard input rate**

> **Compacting the conversation** … By design, this invalidates the conversation
> layer, since the next request has a new, shorter history that doesn't share a
> prefix with the old one.

> In Claude Code, **the cache is effectively scoped to one machine and
> directory.** The system prompt embeds the working directory, platform, shell,
> OS version, and auto memory paths …

And the TTL table, which is the fact most likely to be got wrong:

> | Request bucket | Claude subscription, within plan usage | Usage credits, API key, or cloud provider |
> | Main conversation | **One hour** | Five minutes |
> | Everything else | Five minutes, except the server-controlled helper requests, which get one hour |  Five minutes |

with the bucket definition that decides which side of the migration gets it:

> **Main conversation**: your interactive turns, non-interactive `-p` runs, and
> **Agent SDK turns**, plus the helpers Claude Code runs inline with them

*This cuts against a naive version of our argument.* The bridge drives the Agent
SDK, so its turns are already in the one-hour bucket on a Max plan. The ACP
adapter drives the Agent SDK too. **Neither side gains or loses TTL by the
migration.** The difference is confined to prefix invalidation, which is §1's
divergence events — not to cache lifetime.

*Verified.* On whether any of this is free: Anthropic's support article "Use the
Claude Agent SDK with your Claude plan"
(<https://support.claude.com/en/articles/15036540-…>) currently reads:

> Update June 15: We're pausing the changes to Claude Agent SDK usage described
> below.

leaving the prior policy in force:

> nothing has changed: Claude Agent SDK, `claude -p`, and third-party app usage
> **still draw from your subscription's usage limits**.

So both the bridge and the ACP adapter bill the same Max allowance. The
migration cannot move usage off the plan; it can only reduce the tokens spent.
A cache *read* still consumes the allowance, at roughly a tenth the input rate.

*Verified timeline, and it carries a risk larger than the caching question.* The
change was announced 2026-05-13/14 for a 2026-06-15 effective date and paused on
that date. The pre-pause text (Wayback, 2026-05-13) read: "Starting June 15,
2026, Claude Agent SDK and `claude -p` usage no longer counts toward your Claude
plan's usage limits", with the plan's included usage "reserved for **interactive
use of Claude Code**, Claude Cowork, and Claude", and third-party apps moved onto
a metered credit and then extra usage.

**The article never says which side of that line an ACP-driven Claude Code falls
on**, and nothing public answers it. Is `claude-agent-acp` "interactive use of
Claude Code" (stays on plan) or a "third-party app that authenticates … through
the Agent SDK" (metered)? Given this repository's subscription-only rule, if the
split re-lands, that unwritten classification decides whether the design works at
all — and it applies to the bridge equally. *Hypothesis:* the migration is
neutral on this risk, because both paths drive the Agent SDK. **It is not a
reason to prefer ACP, and it should not be written into the ADR as one.** It is a
reason to keep both paths buildable until Anthropic clarifies.

*Hypothesis (unmeasured, and the honest size of the prize).* Because TTL and
billing pool are identical on both sides, the saving is bounded by (number of
divergence events) × (full uncached re-read of the conversation at that moment)
− (what Claude's native compaction would have cost anyway). For a session that
never compacts and never aborts, the saving is near zero. For a long-lived
foreman session that compacts repeatedly, it could be large. **This is the
measurement that does not exist and that we would be first to publish.**

---

## 5. Limitations of `claude-agent-acp` that matter here

All source quotes below are from `main` as of 2026-09-05 (v0.75.1) unless a
version is given. Issue states are as of 2026-09-05.

### 5.1 Session load — supported, and Claude genuinely owns the id

*Verified, and this is the load-bearing good news.* The adapter advertises the
capability (`src/acp-agent.ts` ~line 2083):

```
loadSession: true,
```

`loadSession` reads Claude's **own** transcript — not a client-side copy. From
`src/resumed-session.ts`:

```ts
import { getSessionMessages, type SessionMessage } from "@anthropic-ai/claude-agent-sdk";
…
// Deliberately search all project directories, matching replaySessionHistory.
const messages = await getSessionMessages(sessionId);
```

and the reopened query is created with the ACP session id handed straight to the
SDK as the resume target (`src/acp-agent.ts` ~line 7640):

```ts
const response = await this.createSession(
  { cwd: params.cwd, mcpServers: params.mcpServers ?? [], … },
  { resume: params.sessionId, resumedModelHint },
);
```

This is exactly the property the redesign is for: **the ACP session id *is*
Claude's native session id**, and the transcript lives in Claude's project
directories, written only by Claude. The spec requires this shape —
"The Agent **MUST** respond with a unique Session ID", and on `session/load`
"the Agent **MUST** replay the entire conversation to the Client in the form of
`session/update` notifications"
(<https://agentclientprotocol.com/protocol/session-setup>).

**Open defects on this path, all unresolved:**

| Issue | Opened | What breaks |
| --- | --- | --- |
| [#1019](https://github.com/agentclientprotocol/claude-agent-acp/issues/1019) | 2026-08-21 | Resuming a native `claude --session-id` conversation "silently completes without its history instead of loading it or failing" |
| [#1024](https://github.com/agentclientprotocol/claude-agent-acp/issues/1024) | 2026-08-22 | Resumed over-limit session permanently stuck: "Prompt is too long", auto-compaction never triggers, `/compact` a silent no-op |
| [#1011](https://github.com/agentclientprotocol/claude-agent-acp/issues/1011) | 2026-08-17 | Orphaned `claude` subprocess children accumulate across repeated `session/load` resumes |
| [#998](https://github.com/agentclientprotocol/claude-agent-acp/issues/998) | 2026-08-14 | `session/load` drops marker-only user prompts for model-bound slash skills |
| [#1077](https://github.com/agentclientprotocol/claude-agent-acp/issues/1077) | 2026-09-02 | 0.73.0: ExitPlanMode clear-context accept "unrecoverable via `session/load`" |
| [#906](https://github.com/agentclientprotocol/claude-agent-acp/issues/906) | 2026-07-23 | `conversation_reset` drops `new_conversation_id` → stale resume after worker restart |

#1019 and #1024 are the two that matter for a long-lived foreman: the first says
a session Claude created outside the adapter may resume *empty and silently*,
the second says a session that went over the limit before resume can be
unrecoverable. Both are directly on our intended path.

### 5.2 Cancel semantics — the weakest area

*Verified.* Cancellation has several open, well-documented defects, and one of
them has a concrete blast radius for an orchestrator that tracks tool state:

- [#1061](https://github.com/agentclientprotocol/claude-agent-acp/issues/1061)
  (0.70.0, opened 2026-09-01) — after `session/cancel` the adapter keeps
  forwarding tool **starts** with no terminals: "31 `tool_call` start updates
  streamed out for the cancelled turn … No terminal updates for any of them",
  and the reporter's stall detector consequently "sat for ~4h50m instead of
  being cancelled after its 30-minute timeout". Any client keeping a ledger of
  open tool calls is poisoned across turns.
- [#994](https://github.com/agentclientprotocol/claude-agent-acp/issues/994) —
  a client stop cancels the turn but "background sub-agents … keep running —
  invisible and unkillable from the client"; the SDK has `stopTask` but the
  adapter exposes no ACP primitive for it.
- [#1027](https://github.com/agentclientprotocol/claude-agent-acp/issues/1027),
  [#1039](https://github.com/agentclientprotocol/claude-agent-acp/issues/1039) —
  steered turns can hang with `session/prompt` never answered.
- [#896](https://github.com/agentclientprotocol/claude-agent-acp/issues/896) —
  `PromptResponse` and final usage withheld on a normal finished turn, flushed
  only by `session/cancel`.

*Hypothesis.* A Prime ACP client must therefore treat cancel as advisory:
reconcile its open-tool ledger from turn boundaries rather than from start/stop
pairing, and not rely on cancel to reclaim background sub-agents. That is client
work we would have to write regardless of what we pin.

### 5.3 Custom tools over MCP — the sharpest risk to the design

This is where "Prime's tools exposed over MCP through ACP" meets a live bug.

*Verified.* The mapping code exists (`src/acp-agent.ts` ~7751):

```ts
for (const server of params.mcpServers) {
  if ("type" in server && (server.type === "http" || server.type === "sse")) {
    …
  } else if (!("type" in server)) {
    // Stdio type MCP server (with or without explicit type field)
    mcpServers[server.name] = { type: "stdio", command: server.command, … };
  }
}
```

**The comment and the guard disagree.** The branch claims to handle stdio "with
or without explicit type field", but its condition is `!("type" in server)`. A
client that sends the stdio variant *with* an explicit `"type": "stdio"` matches
neither branch and is **silently dropped** — no error, no `session/update`, no
diagnostic. This is present in `main` on 2026-09-05.

*Verified.* Independently, the symptom is reported and reproduced without an
explicit `type` field:
[#883](https://github.com/agentclientprotocol/claude-agent-acp/issues/883)
("Session-scoped stdio MCP server from `session/new.mcpServers` never reaches
the model", opened 2026-07-16, **still open**). The reporting host is Alas. A
second reporter confirmed on 2026-08-19:

> still broken on 0.66.0 and on 0.70.0 (latest) … Reproduced fully standalone
> (no ACP host involved), fresh `$HOME`, real Claude subscription token, driving
> `claude-agent-acp` directly over stdio … the model reports no such tool;
> `ToolSearch` finds nothing; Claude Code's own session record shows
> `"pendingMcpServers": [], "needsAuthMcpServers": []`; **the MCP server is
> never even spawned**

The reporter also names the protocol-level gap, which is the part no patch to
our client can fix:

> ACP defines **no confirmation signal** for "the agent successfully connected
> to server X and exposed its tools." So the host UI shows the server as
> attached/requested while the model-facing session behaves as if it was never
> mentioned.

*Verified.* There is an escape hatch that bypasses the ACP `mcpServers` array
entirely. `session/new` accepts `_meta.claudeCode.options`, forwarded to the
Agent SDK, and the documented contract (`src/acp-agent.ts` ~1145) says:

> Those parameters will be used and updated to work with ACP:
>   - hooks (merged with ACP's hooks)
>   - **mcpServers (merged with ACP's mcpServers)**
>   - disallowedTools (merged with ACP's disallowedTools)
>   - **tools (passed through; defaults to `claude_code` preset if not provided)**

confirmed at the merge site (~7936): `mcpServers: { ...(userProvidedOptions?.mcpServers || {}), ...mcpServers, … }`
and at the tools site (~7834): `const tools = userProvidedOptions?.tools ?? …`.

*Hypothesis, and the single most important thing to prove before committing.*
Prime's tools can most likely be delivered by passing them under
`_meta.claudeCode.options.mcpServers` rather than the ACP `mcpServers` array,
and Claude's built-in tools can be gated by passing
`_meta.claudeCode.options.tools`. **Neither is verified to work.** A spike must
send a trivial stdio MCP server both ways and assert the model can actually call
its tool. If both paths fail, the design's tool story fails with them.

Two consequences worth stating plainly:

- The SDK's in-process `createSdkMcpServer` is **not** available to us. The
  adapter runs in its own subprocess, so Prime's tools must be a real stdio or
  HTTP MCP server process, not an in-process object.
- Per §4's cache doc, tool definitions live in the system prompt layer, so the
  tool set must be fixed at session start. "Connecting or disconnecting an MCP
  server" mid-session invalidates the whole prefix when its tools are loaded
  into the prefix rather than deferred.

### 5.4 Authentication with a Claude Code login — supported, and it is the default

*Verified, and this is the second piece of good news.* Subscription login is not
merely tolerated; it is the default, and there is an explicit flag to *opt out*
of it for redistributors. From `src/hide-claude-auth.ts`:

> `--hide-claude-auth`: this integration must never bill a claude.ai
> subscription. The flag hides the claude.ai login method from `initialize` and
> makes the agent behave like a logged-out CLI whenever a subscription would pay
> for a turn.

The changelog entry confirms the direction of travel: **0.74.0 (2026-09-04)**,
"feat(auth): refuse claude.ai subscriptions **under `--hide-claude-auth`**"
([#1079](https://github.com/agentclientprotocol/claude-agent-acp/issues/1079)).
Not passing the flag is the subscription path — which is what §2 records
JetBrains users doing by hand to escape their vendor's build.

*Verified.* 0.75.0 (2026-09-05) added first-class reporting of which identity is
in use ([#1080](https://github.com/agentclientprotocol/claude-agent-acp/issues/1080)).
From `src/auth-status.ts`:

```ts
export type AuthStatusKind = "account" | "api_key" | "gateway" | "external" | "none";
```

> Human-readable and usable as a UI string on its own. The "type" line:
> **"Claude Max"**, "Anthropic API key", "AWS Bedrock".

pushed to the client as `_auth/status_update`, read from `claude auth status --json`.

*Hypothesis, and an attractive one.* This gives the subscription-only invariant
this repository already enforces by patch a **stock, observable** enforcement
point: a Prime ACP client can assert `authStatus.kind === "account"` at
`initialize` and refuse to proceed otherwise — replacing a vendored patch with a
protocol-level check. It requires adapter ≥ 0.75.0, published one day before
this document. Unproven.

### 5.5 Permissions

*Verified.* Permission requests are a first-class ACP surface (the adapter wires
`canUseTool` and an entire `src/permissions/` tree), and 0.71.0 added "expose
permission mode kinds" ([#1025](https://github.com/agentclientprotocol/claude-agent-acp/issues/1025)).
Open defects to expect rather than be surprised by:

- [#1068](https://github.com/agentclientprotocol/claude-agent-acp/issues/1068)
  (regression in 0.71.0) — Bash permission prompts show the model-written
  description instead of the command. For a governor that gates on command text,
  **this is a correctness issue, not cosmetics.**
- [#1050](https://github.com/agentclientprotocol/claude-agent-acp/issues/1050) —
  selecting an option in a permission/question dialog is discarded; only the
  followup notes text is returned.
- [#851](https://github.com/agentclientprotocol/claude-agent-acp/issues/851) —
  background subagent deadlocks the session via permission-request ID desync.
- [#876](https://github.com/agentclientprotocol/claude-agent-acp/issues/876),
  [#585](https://github.com/agentclientprotocol/claude-agent-acp/issues/585) —
  out-of-turn permission asks via `run_in_background`; prompts under bypass mode.

---

## 6. Recommendation

**Do the migration, but stage it behind one spike, and change the justification
you write down.**

*Adopt, and prefer the smallest thing that could work.* There are three options,
in increasing order of code we own — take the first that survives the spike:

0. **Read `sathish316/pi-omniagent-extensions/claude-code-acp.ts` first.** It is
   910 lines of exactly the thing we are proposing to write, minus resume. An
   hour there will retire more unknowns than a week of design.
1. **Add Claude as an ACP transport to `pi-harness-delegate`.** Its ACP runner is
   already general, already resumes via `session/load`, and already answers
   permission requests. The change is a `supportsTransports: ['stdout', 'acp']`
   plus a `buildAcpArgs` spawning `npx @agentclientprotocol/claude-agent-acp`.
   Cheapest by a wide margin, and it fixes an upstream mistake rather than
   routing around it. Risk: it is somebody else's extension, one maintainer, and
   its delegate model is one-shot-per-process — which may not fit a long-lived
   foreman session at all. Check that fit **first**; it is the assumption most
   likely to kill this option.
2. **Pin `@agentclientprotocol/sdk`** (1.4.0, Apache-2.0, zero runtime
   dependencies, ESM-only) and write Prime extension glue on it — roughly the
   spawn plus `Readable.toWeb`/`Writable.toWeb` transport (the TS SDK ships
   `ndJsonStream` but **no** subprocess helper; the Rust crate does), a `Client`
   implementation (`requestPermission`, `sessionUpdate`, optionally
   `readTextFile`/`writeTextFile`), and the provider shim. Use the fluent
   `client()` / `SessionBuilder` / `ActiveSession` API: `ClientSideConnection`
   still works but carries `@deprecated` ("Prefer `client({ name })
   .connectWith(…)`"), and `sathish316/pi-omniagent-extensions` — the best
   worked example — is built on the deprecated one. Read it for design; it has
   **no LICENSE**, so do not copy it.
3. Write a wire implementation. **Don't.**

Name traps, both verified: the unscoped `agent-client-protocol` does not exist on
npm, and all 27 versions of `@zed-industries/agent-client-protocol` are
deprecated in favour of `@agentclientprotocol/sdk`.

*Restore ladder.* Persist only the ACP session id and restore
`session/resume` → `session/load` → (bounded) handoff, the pattern goose, acpx,
patchbay, vscode-acp, nori-cli, hermes and agent-shell converged on
independently (§2.2). Prefer `resume` — it does not replay — and reach for `load`
only if Prime needs to render or audit prior turns. **If you write a
transcript-replay fallback, bound it from the start**: goose #10764 is what an
unbounded one does.

*Operational gotchas worth knowing before the spike, each from a shipped client:*

- **`env_remove: ["CLAUDECODE"]` when spawning the adapter**, or Claude's
  nested-session detection fires. Command Governor runs agents *inside* Claude
  Code, so this one is close to certain to bite.
- **Do not build on `session/fork`.** The registry's own nightly probe
  (2026-09-05) advertises `sessionFork: true` but gets `-32603 Internal error`
  live; `session/stop` and `session/set_model` return `-32601`.
- **Model selection is not `session/set_model`** on this adapter — use
  `session/set_config_option` with `configId: "model"` (cf. adapter issue #1056,
  where `_meta.claudeCode.options.model` is silently overridden by `settings.model`).
- **Pin a PATH binary rather than `npx …@latest`**, given §3's cadence.

In every case, track `@agentclientprotocol/claude-agent-acp` as a version-pinned,
re-verified dependency and expect to move the pin often; a 0.x publishing six
times in five days will break you if you treat it as stable.

*Prove first, in this order — each is falsifiable and each can kill the design:*

1. **Custom tools (§5.3).** Send a trivial stdio MCP server via ACP
   `session/new.mcpServers` **and** via `_meta.claudeCode.options.mcpServers`,
   and assert the model calls its tool. Negative control: assert the failing
   path fails. If neither works, stop — #883 is open and there is no ACP
   confirmation signal to detect the failure at runtime.
2. **Session identity (§5.1).** Create a session, prompt, drop the adapter,
   `session/load` the same id, and assert Claude's own transcript file grew
   rather than being rewritten, and that history is present (the #1019 failure
   mode is a *silent* empty resume — so assert on content, not on success).
3. **Auth (§5.4).** Assert `authStatus.kind === "account"` with label "Claude
   Max" on a Keychain login, and assert refusal when it is not.
4. **The cache claim (§4).** Only then measure. Run the same scripted workload
   through the bridge and through ACP, across at least one compaction, and
   record `cache_read_input_tokens` / `cache_creation_input_tokens` per turn from
   `claude -p … --output-format json` or `/usage`. Set
   `FORCE_PROMPT_CACHING_5M=1` for a controlled comparison. **Predict the
   direction before running it**; if ACP does not show fewer cache-creation
   tokens across a compaction, the stated motivation is wrong and should be
   withdrawn from the ADR.

*Write down a different reason than "prompt cache".* The evidence supports a
narrower and more defensible claim, and §4 shows why the broad one overreaches:
TTL bucket and billing pool are identical on both sides, so the migration's
saving exists only at divergence events. Lead instead with the argument the
protocol's own maintainer already made and goose already proved the hard way
(§2.1) — a harness cannot faithfully reconstruct an agent's post-compaction
state, and a harness that tries eventually cannot resume at all. The durable
arguments are:

- **Correctness, not economics.** The bridge wipes and rewrites Claude's session
  file on divergence. ACP never writes it. One of those two can lose a
  transcript; the other cannot.
- **Compaction ownership.** Under ACP, Claude compacts its own context on its
  own prefix; under the bridge, Pi compacts and Claude then re-reads a history
  Pi already paid to summarise.
- **Deletion of vendored code.** The bridge is a vendored tarball plus a
  three-fix compatibility patch this repository maintains. The adapter is
  upstream-maintained and its subscription-only behaviour is stock (§5.4),
  which suits ADR 0010's composition-first rule better than a patch we own.

*Tradeoffs, stated plainly.*

- **You trade a patch you control for a 0.x you don't.** The bridge's failure
  modes are ones we have read end to end; the adapter's are 164 open issues
  moving daily, including live defects on cancel (§5.2), permission display
  (§5.5) and MCP delivery (§5.3).
- **The MCP tool path is unproven and is the design's single point of failure.**
  Everything else has a workaround; if Prime's tools cannot reach the model,
  there is no product.
- **You give up harness-side control of context.** Claude owning compaction is
  the point, but it also means Prime can no longer decide what survives.
- **The economic case may evaporate under measurement.** Say so now, in the ADR,
  rather than after the spike.
- **Subprocess and lifecycle cost.** #1011 (orphaned children across resumes) and
  #994 (unkillable background sub-agents) are real for a long-lived supervisor,
  and Prime's own per-path lease work does not extend across the ACP boundary.

---

## 7. Are we the first to do this?

**No — not to the mechanism, and not by a wide margin.**

Driving Claude Code over ACP as a client is a commodity: the protocol's own
directory lists well past a hundred clients. Driving it *from another harness*,
as delegated capability, is published prior art in at least OpenClaw, Mastra and
AgentPool. Resuming Claude's own session by id over `session/load` is published
in OpenClaw and implemented in Alas. Letting Claude own the session is not a
choice any of them made — it is what the ACP spec requires of every agent, and
the adapter satisfies it by passing the ACP session id to the Agent SDK's
`resume` and reading Claude's own transcript through `getSessionMessages`.

**Within the Pi family, no — and this document was wrong about that at first
draft.** Three separate Pi extensions already do most of it (§3):
`pi-harness-delegate` 0.6.1 is a maintained generic ACP client with live-verified
`session/load`; `sathish316/pi-omniagent-extensions` and
`@junghanacs/pi-shell-acp` both drive `@agentclientprotocol/claude-agent-acp`
over `ClientSideConnection`. One of them even states a credit-economics tagline
and independently strips `ANTHROPIC_API_KEY` for the same subscription-only
reason we do. "First Pi-lineage ACP client" and "first to drive Claude over ACP
from Pi" are both **not** ours to claim.

The genuinely unclaimed square is narrower than any of those: **reattaching to
Claude's own session.** The delegate extension has `session/load` but aims it at
Devin and OpenCode; the two Claude extensions call `newSession` only and never
resume. Prime itself sits at the far end of other people's ACP pipes — its docs
describe ACP mode as making it "an Agent Client Protocol agent", and both Alas
and OpenClaw list Pi among the harnesses they drive.

So the pieces all exist and nobody has assembled them. Be clear-eyed about what
that implies: assembly is not invention, and the reason nobody has bothered may
simply be that the payoff is small — which is exactly what §6's step 4 exists to
find out.

**Where we would genuinely be first is the part nobody has written down:** the
argument that a harness should hand session ownership to Claude to avoid paying
to rebuild it, and a measurement showing what that is worth. Every implementer
found built the mechanism for integration reasons — editor UX, worktree
management, delegating to a harness with better tools — and none for cache or
usage-allowance reasons. No numbers exist. The honest position is that we are
adopting a well-trodden mechanism for a reason nobody has validated, including
us, and that §6's step 4 is what converts that from a belief into a finding.

---

## Sources

Read 2026-09-05 unless noted.

**Adapter and protocol**
- <https://github.com/agentclientprotocol/claude-agent-acp> — v0.75.1, Apache-2.0, 164 open issues, pushed 2026-09-05
- `src/acp-agent.ts`, `src/resumed-session.ts`, `src/hide-claude-auth.ts`, `src/auth-status.ts`, `src/context-compaction.ts` at `main`
- <https://www.npmjs.com/package/@agentclientprotocol/claude-agent-acp> — 0.75.1; supersedes `@zed-industries/claude-agent-acp` (0.23.1) and `@zed-industries/claude-code-acp` (0.16.2)
- <https://www.npmjs.com/package/@agentclientprotocol/sdk> — 1.4.0, published 2026-08-20
- <https://agentclientprotocol.com/protocol/session-setup> — session id and `session/load` requirements
- <https://agentclientprotocol.com/overview/clients> — client directory

**Adapter issues** — #883, #994, #998, #1011, #1019, #1024, #1027, #1039, #1050, #1061, #1068, #1077, #517, #851, #876, #896, #906, #1025, #1079, #1080

**Other ACP clients**
- <https://open-claw.bot/docs/tools/acp-agents/> — external harnesses, `resumeSessionId` → `session/load`
- <https://mastra.ai/blog/introducing-agent-client-protocol> — 2026-06-02, Claude as delegated sub-agent
- <https://phil65.github.io/agentpool/advanced/acp-integration/> — client and server simultaneously
- <https://github.com/mrmans0n/alas> — Swift ACP host with session persistence and forks

**Pi-family packages** (read from published tarballs, 2026-09-05)
- `pi-harness-delegate` 0.6.1, published 2026-08-30, <https://github.com/yorch/pi-harness-delegate> — `extensions/acp-runner.ts` (generic ACP client, `session/load`), `extensions/harnesses/claude.ts` (`supportsTransports: ['stdout']`), README transport table
- `pi-acp` 0.0.33, published 2026-07-30, <https://github.com/svkozak/pi-acp> — ACP *agent* side for Pi; `~/.pi/pi-acp/session-map.json` side-table
- `pi-claude-bridge` 0.7.0, published 2026-08-09, <https://github.com/elidickinson/pi-claude-bridge> — upstream of the vendored bridge
- `omp-claude-bridge` — not published on npm under that name (404 on 2026-09-05)
- <https://github.com/sathish316/pi-omniagent-extensions> — `claude-code-acp.ts`, 910 lines, pushed 2026-07-26, **no LICENSE**; Pi extension driving `claude-agent-acp` via `ClientSideConnection`; `newSession` only, no `session/load`
- `@junghanacs/pi-shell-acp` 0.11.1, published 2026-06-29 — depends on `@agentclientprotocol/claude-agent-acp@0.39.0` and `@agentclientprotocol/sdk@0.22.1`
- `earendil-works/pi` issues #836 (2026-01-19, ACP mode PR declined) and #7320 (2026-07-30, ACP *client* ask, closed `not_planned`)
- `@zed-industries/agent-client-protocol` — all 27 versions deprecated, renamed to `@agentclientprotocol/sdk`; unscoped `agent-client-protocol` does not exist on npm

**Session-ownership rationale and the replay failure mode**
- [claude-agent-acp#80](https://github.com/agentclientprotocol/claude-agent-acp/issues/80) — Ben Brandt, 2025-10-09: "it really needs to be owned by the agent"; post-`/compact` state cannot be reconstructed. Closed 2026-02-18.
- <https://agentclientprotocol.com/rfds/session-resume> — `session/resume` MUST NOT replay history; stabilised 2026-04-22
- <https://agentclientprotocol.com/rfds/session-list>
- [goose#10764](https://github.com/aaif-goose/goose/issues/10764) — uncapped replay makes long sessions permanently unresumable; closed 2026-07-28. goose PR #10379 (2026-08-10) resumes provider-native ACP sessions instead.
- <https://zed.dev/docs/ai/external-agents>; CodeCompanion, agentic.nvim, acp-patchbay, acpx, agent-shell, Tidewave (per client-landscape sweep)

**Cache and usage-limit motivation**
- [cline/cline#9892](https://github.com/cline/cline/discussions/9892) — 2026-03-19, `cmaga`, 0 replies: `cache_read_input_tokens` "always 0"; "Session limits are hit much faster"; fix = session resume
- <https://claude.com/blog/lessons-from-building-claude-code-prompt-caching-is-everything> — Thariq Shihipar, 2026-04-30: cache hit rate "helps us create more generous rate limits for our subscription plans"
- <https://docs.litellm.ai/blog/bedrock-invoke-prompt-caching-incident> — 4–10 July 2026: 90% → 25-45% hit rate, 2-3x spend, silent (HTTP 200)

**Anthropic primary sources**
- <https://code.claude.com/docs/en/prompt-caching> — prefix matching, invalidation list, TTL buckets, cache scope, `cache_read_input_tokens`
- <https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan> — June 15 pause; SDK usage still draws on subscription limits

**This repository**
- `pins/packages/pi-claude-agent-sdk-0.8.6.tgz` (`package/src/index.ts`, `package/README.md`)
- `pins/pins.json`, `pins/patches/pi-claude-agent-sdk-0.8.6-prime-compat.patch`
- `docs/research/2026-09-01-agent-harness-landscape-and-substrate-bakeoff.md`
- Prime Agent `packages/coding-agent/docs/acp.md`
