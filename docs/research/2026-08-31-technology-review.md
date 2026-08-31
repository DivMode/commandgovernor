# Command Governor technology review — 2026-08-31

Status: **architecture input / pinned research snapshot**.

This report records the public sources verified before V1 implementation. Dates,
releases, and commits are intentional because the browser, MCP, and agent surfaces
below change quickly.

## Executive findings

The original architecture hypothesis is substantially correct, with three
important refinements discovered during review:

1. **ChatGPT writes should remain browser-owned.** Current independent projects
   converge on a hybrid approach: let the authenticated ChatGPT SPA perform
   sensitive submission while CDP/passive network evidence and narrowly bounded
   authenticated reads provide observation/reconciliation. Command Governor should
   not become a Sentinel/Turnstile/proof-of-work/private-write emulator.
2. **Write-capable ChatGPT MCP is a deployment gate, not an assumption.** OpenAI's
   current published developer-mode documentation does not make arbitrary
   state-changing custom MCP actions available on every ChatGPT plan/surface.
   Explicit foreman ACK is a mutation; the real target account must pass a
   capability preflight. We will not disguise a write as a read or substitute
   assistant settlement for ACK.
3. **A Claude `Stop` hook firing is not definitive completion.** Current Claude
   Code docs say matching hooks run in parallel and a Stop hook can block stopping,
   causing Claude to continue. Managed V1 should therefore use Claude's structured
   programmatic final `result` plus matching child-process completion as terminal
   success evidence. Stop becomes a bounded `stop_candidate`. A small Rust
   worker-host/private spool preserves that structured result if the authoritative
   daemon restarts while Claude is still running.

The recommended V1 remains Rust daemon + CLI, one SQLite authority, dedicated
headed Chrome + CDP, a small stable MCP ABI, explicit obligation/ACK semantics, and
native/structured worker evidence that outranks stale PTY/runtime inference.

## Pinned source snapshot

| Project / source | Reviewed revision / release | Evidence used |
| --- | --- | --- |
| `DivMode/commandgovernor` | baseline `fd3e5a61425f00ee3b164d2a840708602f972342` | Pre-implementation docs baseline. |
| `miuuyy/codex-chatgpt-web` | main `d7675fc7767a8f19b908f3e5d0e357699d1d9fdf`; release `v4.0.7` | Retained conversations, exact browser surface ownership, send/settlement separation, connector ABI, reconnect/no replay. |
| `ChesterRa/CCCC` | main `5f0b83242d09c88b1e2267d1056fc5bf64feb626` | Append-only daemon authority and `claimed/accepted/failed/ambiguous` delivery semantics. |
| `DivMode/tandem` | main `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee`; PR #6 head `af568233e1aae2d4cc343b38ca0e2a1a248e7857` | Claude lifecycle deposit, stale client, completion barrier, real stale-runtime lessons. PR remains open/review-only. |
| `Maxmedawar/tandem` | main `a98bcafd2c40ae5473b85fe41183e4f391933799` | Upstream Herdr/session/MCP/fleet baseline. |
| OpenWeb (`imoonkey/openweb`; former org URL redirects) | main `a387b50c829d871839a613732e1b97bfa1946124` | Browser/session hybrid ChatGPT transport research. |
| `Octo-Lex/ChatGPT-Web2API` | master `497527dceabfa3f95961e23c291e618c5570f1ac` | CDP/browser architecture, backend-read reconciliation, non-replay on uncertain send. |
| `stufently/gpt-web-gateway` | main `efb01a32e9e4c7fbebb8acff204c8c2a448c476c`; app `2.14.1` | Real-browser sensitive writes, passive/backend observation, headed-browser field bias. |
| `modelcontextprotocol/rust-sdk` / `rmcp` | main `3a2ebbc92034f6711e4eac9e93f5de0423be8dfe`; crate `3.1.4` | Official Rust MCP SDK/current protocol support. |
| `mattsse/chromiumoxide` | main `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`; crate `0.9.1` | Tokio CDP, attach/launch Chrome, Target/Page/Network domains. |
| `rust-headless-chrome/rust-headless-chrome` | `0a5c307a85debc450378a1f19e4dac1838d7b22d`; `1.0.22` | Rust CDP fallback candidate. |
| `tauri-apps/wry` | dev `bb69d628a905d65042c71a95e85f6921ec9b3264` | Platform WebView comparison. |
| `tauri-apps/cef-rs` | dev `a2e15ae659c4b3957883e34de879bd8b38360ce5` | Embedded Chromium comparison. |

Pinned project links:

- <https://github.com/miuuyy/codex-chatgpt-web/tree/d7675fc7767a8f19b908f3e5d0e357699d1d9fdf>
- <https://github.com/ChesterRa/CCCC/tree/5f0b83242d09c88b1e2267d1056fc5bf64feb626>
- <https://github.com/DivMode/tandem/pull/6>
- <https://github.com/Maxmedawar/tandem/tree/a98bcafd2c40ae5473b85fe41183e4f391933799>
- <https://github.com/imoonkey/openweb/tree/a387b50c829d871839a613732e1b97bfa1946124>
- <https://github.com/Octo-Lex/ChatGPT-Web2API/tree/497527dceabfa3f95961e23c291e618c5570f1ac>
- <https://github.com/stufently/gpt-web-gateway/tree/efb01a32e9e4c7fbebb8acff204c8c2a448c476c>
- <https://github.com/modelcontextprotocol/rust-sdk/tree/3a2ebbc92034f6711e4eac9e93f5de0423be8dfe>
- <https://github.com/mattsse/chromiumoxide/tree/afcc3a4313f2087249b4490d94e54bf8e3bfaccf>

A same-day freshness pass rechecked codex-chatgpt-web main, CCCC main, Tandem PR #6,
and rust-sdk main; the pinned revisions above were still current at that check.

## ChatGPT Web transport research

### Current private-write reality

Public ChatGPT Web reverse-engineering projects continue to encounter changing
private browser/session/security machinery: authenticated session state,
Sentinel/challenge fields, proof-of-work behavior, Turnstile/Cloudflare responses,
private request schema/model identifiers, and evolving streaming behavior.

The architecture lesson is more durable than any captured request body:

- OpenWeb's ChatGPT work distinguishes browser/session-sensitive writes from more
  tractable observation/read paths.
- ChatGPT-Web2API uses Chrome/CDP for protected interactions and explicitly avoids
  blindly replaying sends whose outcome is uncertain.
- gpt-web-gateway uses real browser interaction for sensitive writes and
  passive/backend observation where cleaner evidence is available.
- codex-chatgpt-web V4 owns exact browser surfaces/conversation identity, separates
  submission from physical settlement, retains/reconnects conversations without
  replay, and treats connector identity/schema as a compatibility boundary.

### Decision implication

`governor-chatgpt-web` may:

- control a locally authenticated dedicated browser;
- observe CDP Target/Page/Network events;
- passively interpret requests/responses generated by the real SPA;
- perform narrowly bounded authenticated reads when proven robust; and
- reconcile ambiguous delivery against exact conversation/message evidence.

It must not:

- reproduce/bypass Sentinel, Turnstile, proof-of-work, CAPTCHA, entitlements, rate
  limits, or anti-abuse controls;
- export browser credentials into a standalone unofficial write client as the
  normal architecture; or
- automatically replay an uncertain sensitive send.

The ChatGPT adapter remains one replaceable crate so an official provider API can
supersede it later.

## Browser-control comparison

### `chromiumoxide` — recommended V1 driver

Strengths:

- Rust/Tokio-native asynchronous model;
- launch or attach to Chrome through CDP;
- explicit targets/pages;
- generated CDP domains including Network events;
- no GUI framework or embedded-browser packaging requirement.

Costs:

- lower-level than Playwright;
- Command Governor owns selector/readiness/reconnect logic;
- browser/project release cadence is slower than mainstream JS automation.

Mitigation: keep a narrow internal `BrowserTransport` trait and keep ChatGPT SPA
logic out of the generic browser crate.

### `headless_chrome` — fallback driver

A credible Rust CDP fallback with headed operation/network interception. It is not
first choice because its blocking/threading ergonomics are less natural for the
Tokio daemon and V1 needs explicit low-level evidence handling anyway.

### Wry — reject for V1 correctness path

Wry abstracts native OS WebViews (WKWebView/WebView2/WebKitGTK). That is useful for
a desktop UI, but V1 has no GUI and needs a uniform Chrome/CDP target/network
model, browser-profile ownership, and exact target semantics.

### CEF — defer

CEF provides complete embedded Chromium ownership, but at the price of a large
browser distribution, packaging/signing, browser security updates, sandboxing,
and cross-platform lifecycle burden. Do not pay that cost unless system Chrome +
CDP fails a measured requirement.

### Headed vs headless

Current public ChatGPT browser projects report enough practical difference that
V1 should support headed system Chrome first. `--headless=new` is a separate live
experiment. Do not add stealth/challenge bypass to make headless pass.

## Claude Code research — corrected terminal model

Primary current docs reviewed:

- <https://code.claude.com/docs/en/hooks>
- <https://code.claude.com/docs/en/cli-reference>
- <https://code.claude.com/docs/en/headless>
- current settings documentation linked from those pages.

### Current lifecycle/input surface

Relevant documented events/features include:

- `UserPromptSubmit`
- `PreToolUse`
- `PermissionRequest`
- `PermissionDenied`
- `PostToolUse`
- `PostToolUseFailure`
- `PostToolBatch`
- `Notification`
- `SubagentStart` / `SubagentStop`
- `TaskCreated` / `TaskCompleted`
- `TeammateIdle`
- `Stop`
- `StopFailure`
- `SessionStart` / `SessionEnd`
- `Elicitation` / `ElicitationResult`
- programmatic `system/init` and final structured `result` messages.

### Critical Stop-hook finding

Current docs explicitly allow a Stop hook to block stopping. Matching hooks run in
parallel; another Stop hook can return a blocking decision and Claude continues.
The `stop_hook_active` field exists to prevent continuation loops.

Therefore the earlier simplistic rule "Stop beats Herdr working" is incomplete.
The correct rule for managed non-interactive V1 is:

```text
Stop hook callback -> stop_candidate only
final structured `claude -p` result + matching child exit -> terminal result proof
```

`SessionEnd` is strong/non-blockable evidence of session termination but does not
prove a successful result. `StopFailure` is strong failure evidence and must be
reconciled with the managed command outcome.

This model still fixes the reproduced Tandem class: a confirmed structured final
or deferred Claude state beats stale Herdr `working / idle:false`. It simply avoids
inventing a new false-completion bug from a vetoable hook.

### Programmatic output is the stronger V1 primitive

Current Claude programmatic docs describe:

- `claude -p` success/nonzero exit behavior;
- structured `stream-json` output;
- a `system/init` record with session metadata/capabilities in current releases;
- a final `result` stream record carrying the final response/session metadata.

That is a better completion/result interface than PTY scraping and better terminal
proof than one vetoable hook callback.

### Why a Rust worker-host/spool is needed

If the daemon alone reads Claude stdout, a daemon crash can still lose the final
structured result while Claude continues. V1 therefore adds a tiny Rust transport
mode under the process runtime:

```text
command-governor worker-host claude <opaque-turn-id>
```

It launches/resumes `claude -p`, writes the sensitive structured stream to a
private bounded spool, persists a sanitized child-exit receipt, and owns no
orchestration state. The daemon later reconciles it and creates the bounded
immutable result artifact.

This is one authoritative daemon plus a durable transport shim, not multiple
orchestration daemons.

### Input/permission

Current `PreToolUse` supports a `defer` decision. In non-interactive mode this can
preserve a pending tool call such as `AskUserQuestion` for later same-session
resume. V1 should project `needs_input` only after the managed structured outcome
confirms the defer took effect.

`PermissionRequest` is important evidence that Claude wants a permission decision,
but current docs do not make it a generic durable pause/resume-later primitive.
For out-of-band authorization, prefer policy classification/defer at `PreToolUse`
before execution, subject to live conformance.

Current settings can merge hooks/configuration across scopes. `--settings` alone
must not be assumed to isolate all user/project/plugin hooks. The live Claude gate
must measure active settings/hook behavior for the chosen invocation.

## Tandem findings

Current `DivMode/tandem` PR #6 remains open at
`af568233e1aae2d4cc343b38ca0e2a1a248e7857` and explicitly says not to merge.
Useful lessons:

- lifecycle evidence should come from the worker/provider rather than screen
  inference when possible;
- a private lifecycle deposit can survive process restart;
- stale clients/tool schemas are real;
- a completion barrier prevents the foreman from declaring success over unread
  worker work;
- an apparently unchanged send result is not permission to resend.

Command Governor does not copy Tandem's JavaScript stack, runtime status vocabulary,
or human notification mechanisms.

The known regression remains mandatory, refined by the Stop-veto discovery:

```text
confirmed Claude structured final/deferred state
Herdr still says working / idle=false
```

The confirmed worker fact wins. A lone Stop callback does not.

## CCCC semantics to independently implement

CCCC's current Collaboration Standard documents transport-independent delivery
states `claimed`, `accepted`, `failed`, and `ambiguous`, requiring intent to be
recorded before external I/O and preventing blind replay after accepted/ambiguous.

Command Governor independently implements that safety property in Rust, including
an internal durable `activation_armed` fence immediately before exact browser Send.
A crash can conservatively create a zero-send ambiguity; at-most-once safety wins
over guessing.

CCCC is Apache-2.0. This project is adopting documented semantics rather than
copying source. Future copied Apache material requires the original license and
NOTICE/provenance handling.

## MCP research and ChatGPT capability constraint

Preferred SDK:

- <https://github.com/modelcontextprotocol/rust-sdk>
- crate `rmcp` `3.1.4` at review time.

The server should negotiate the MCP protocol supported by the client rather than
hard-code one date.

Current OpenAI ChatGPT developer-mode/app documentation was reviewed. Two
architecture consequences matter:

1. local MCP uses OpenAI's supported tunnel/connectivity path rather than simply
   assuming ChatGPT can call arbitrary localhost;
2. current custom MCP action availability differs by ChatGPT plan/surface, so the
   actual target account must prove state-changing tool capability.

The public foreman ABI is therefore small and stable:

- `foreman_bootstrap`
- `foreman_resume`
- `foreman_ack`
- `foreman_answer_input`

`foreman_resume`, ACK, and answer are truthful mutations. If the target ChatGPT
surface is read/fetch-only, V1 foreman automation is unsupported there; no tool is
mislabeled to bypass product policy.

Official sources reviewed:

- <https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt-beta>
- <https://developers.openai.com/apps-sdk/>

## OpenAI terms / public-project risk

Current public sources reviewed:

- OpenAI Terms of Use, effective 2026-01-01:
  <https://openai.com/policies/terms-of-use/>
- OpenAI App Developer Terms, updated 2026-07-09:
  <https://openai.com/policies/app-developer-terms/>

Restrictions relevant to reverse engineering, automated/programmatic extraction,
rate/restriction bypass, and protective measures make a protected private-write
client a poor public-project architecture. A user-controlled normal-login browser,
local credential storage, and no bypass is materially safer, but it does **not**
make unofficial ChatGPT Web automation officially supported or remove account/ToU
risk.

Public posture:

- normal user authentication;
- no auth/entitlement/CAPTCHA/Turnstile/rate/anti-abuse bypass;
- browser credentials stay local;
- no claim of OpenAI endorsement;
- unofficial browser adapter may break without notice;
- adapter is replaceable if an official wake/foreman API appears.

This is engineering/security risk documentation, not a legal conclusion.

## Rust foundation

Initial candidate baseline, to re-verify at the scaffold commit:

- stable Rust `1.98.0`, edition 2024;
- Tokio `1.53.x`;
- serde `1.0.x`;
- thiserror `2.0.x`;
- tracing `0.1.x`;
- clap `4.6.x`;
- `rmcp` `3.1.4`;
- `rusqlite` `0.40.x` with bundled SQLite;
- uuid `1.26.x`;
- `time` `0.3.x`;
- axum/tower only if a loopback HTTP endpoint is genuinely needed.

`anyhow` is acceptable at binary/process boundaries; domain crates expose
structured errors.

A contemporaneous August 2026 Rust ecosystem supply-chain incident included
malicious releases of `arrayref 0.3.10`, `internment 0.8.7`, and
`append-only-vec 0.1.9`. Initial lockfile/CI policy should explicitly reject
known-bad versions in addition to `cargo audit`/`cargo deny`.

## SQLite decision

Prefer `rusqlite` over SQLx SQLite for V1 because the architecture deliberately has
one authoritative daemon and one serialized writer. SQLite WAL improves reader
concurrency but does not make multiple writes commit concurrently. A general async
connection pool buys little for the correctness path and can obscure the exact
transaction boundary.

Initial store policy:

- bundled SQLite;
- one dedicated DB actor/thread;
- WAL;
- `foreign_keys=ON`;
- bounded busy timeout;
- `synchronous=FULL` until crash evidence justifies another setting;
- explicit migrations/schema epoch;
- explicit transactions/no ORM.

## Durable result boundary

A `completed_unprocessed` row is not enough if the result only exists in a terminal.
V1 therefore has two explicit private content boundaries:

1. **worker-host transport spool** — temporary sensitive structured provider output
   that survives daemon restart; not lifecycle authority;
2. **immutable result artifact** — bounded final worker result needed by the
   foreman, referenced/pinned by SQLite until the obligation is closed and
   retention permits deletion.

Neither content belongs in browser wakes or routine logs.

## Required live gates — not fabricated

### Gate A — ChatGPT MCP mutations

The actual target ChatGPT account/surface must invoke genuine state-changing
foreman tools. Current published plan differences mean this cannot be assumed.

### Gate B — headed Chrome + ChatGPT

A dedicated authenticated headed Chrome profile must prove exact `/c/<id>` binding,
per-message Command Governor app selection, ten unique wakes, strong accepted
message evidence, crash-at-Send ambiguity, no replay, restart/rebind, and MCP
outage behavior. Headless is tested separately.

### Gate C — Claude managed execution

The pinned Claude invocation must prove:

- actual settings/hook source behavior;
- structured init/capability parsing;
- final result + child-exit semantics;
- controlled parallel Stop-hook veto without false completion;
- confirmed defer/resume;
- daemon-offline worker-host spool recovery;
- stale Herdr working conflict resolution;
- forbidden-data non-persistence.

These live tests were **not fabricated** by this architecture session. The project
explicitly forbids using the currently unreliable Tandem/Claude orchestration loop
to bootstrap them.

## Recommendation

**Proceed with the small pure Rust core/store/testkit foundation after architecture
review.** Do not yet claim the end-to-end ChatGPT foreman loop or Claude adapter is
supported. Those product/provider adapters remain behind Gates A, B, and C.

This separation is intentional: if a current service capability fails, replace or
disable the adapter rather than weakening durable obligations, at-most-once
ambiguity, or explicit ACK.
