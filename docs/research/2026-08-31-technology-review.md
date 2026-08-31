# Command Governor technology review — 2026-08-31

Status: **architecture input / pinned research snapshot**.

This report records public sources verified before V1 implementation. Dates,
releases, and commits are intentional because browser, MCP, and agent surfaces
change quickly.

## Executive findings

The original architecture hypothesis is substantially correct, with four important
refinements discovered during review:

1. **ChatGPT writes should remain browser-owned.** Current independent projects
   converge on a hybrid: let the authenticated ChatGPT SPA perform sensitive
   submission, while CDP/passive network evidence and narrowly bounded reads
   provide observation/reconciliation. Command Governor should not become a
   Sentinel/Turnstile/proof-of-work/private-write emulator.
2. **Write-capable ChatGPT MCP is a deployment gate, not an assumption.** OpenAI's
   current published developer-mode documentation limits full custom MCP actions by
   plan/surface. Explicit foreman ACK is a mutation; the real target account must
   pass a capability preflight. We will not disguise a write as a read or
   substitute assistant settlement for ACK.
3. **A Claude `Stop` hook firing is not definitive completion.** Current Claude
   docs say matching hooks run in parallel and a Stop hook can block stopping,
   causing Claude to continue. Managed V1 should use the structured programmatic
   final `result` plus matching child-process completion as terminal success
   evidence. Stop becomes bounded `stop_candidate` evidence.
4. **Managed `claude -p` permission policy belongs at `PreToolUse`.** Current Claude
   hook guidance explicitly says `PermissionRequest` hooks do not fire in
   non-interactive `-p` mode and directs automated permission decisions to
   `PreToolUse`. V1 must not build a durable managed-input path around an event that
   is absent in its preferred execution mode.

The recommended V1 remains Rust daemon + CLI, one SQLite authority, dedicated
headed Chrome + CDP, a small stable MCP ABI, explicit obligation/ACK semantics, and
structured worker evidence that outranks stale PTY/runtime inference.

## Pinned source snapshot

| Project / source | Reviewed revision / release | Evidence used |
| --- | --- | --- |
| `DivMode/commandgovernor` | baseline `fd3e5a61425f00ee3b164d2a840708602f972342` | Pre-implementation docs baseline. |
| `miuuyy/codex-chatgpt-web` | main `d7675fc7767a8f19b908f3e5d0e357699d1d9fdf`; release `v4.0.7` | Retained conversations, exact browser surface ownership, send/settlement separation, connector ABI, reconnect/no replay. |
| `ChesterRa/CCCC` | main `5f0b83242d09c88b1e2267d1056fc5bf64feb626` | Append-only daemon authority and `claimed/accepted/failed/ambiguous` semantics. |
| `DivMode/tandem` | main `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee`; PR #6 head `af568233e1aae2d4cc343b38ca0e2a1a248e7857` | Claude lifecycle deposit, completion barriers, stale-runtime lessons. PR remained open/review-only. |
| `Maxmedawar/tandem` | main `a98bcafd2c40ae5473b85fe41183e4f391933799` | Upstream Herdr/session/MCP/fleet baseline. |
| OpenWeb (`imoonkey/openweb`) | main `a387b50c829d871839a613732e1b97bfa1946124` | Browser/session hybrid ChatGPT transport research. |
| `Octo-Lex/ChatGPT-Web2API` | master `497527dceabfa3f95961e23c291e618c5570f1ac` | CDP/browser architecture, backend-read reconciliation, non-replay after uncertain send. |
| `stufently/gpt-web-gateway` | main `efb01a32e9e4c7fbebb8acff204c8c2a448c476c`; app `2.14.1` | Real-browser writes, passive/backend observation, headed-browser field bias. |
| `modelcontextprotocol/rust-sdk` / `rmcp` | main `3a2ebbc92034f6711e4eac9e93f5de0423be8dfe`; crate `3.1.4` | Official Rust MCP SDK/current protocol support. |
| `mattsse/chromiumoxide` | main `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`; crate `0.9.1` | Tokio CDP, attach/launch Chrome, Target/Page/Network. |
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

### Private-write reality

Public reverse-engineering projects continue to encounter changing private
browser/session/security machinery: authenticated browser state, Sentinel/challenge
fields, proof-of-work behavior, Turnstile/Cloudflare responses, private request
schema/model identifiers, and changing stream behavior.

The useful convergence is architectural:

- OpenWeb distinguishes browser/session-sensitive writes from more tractable
  observation/read paths.
- ChatGPT-Web2API uses Chrome/CDP for protected interaction and avoids blind replay
  where send outcome is uncertain.
- gpt-web-gateway uses real-browser interaction for sensitive writes and
  passive/backend observation when cleaner evidence is available.
- codex-chatgpt-web V4 owns exact browser surfaces/conversation identity, separates
  submission from physical settlement, reconnects without replay, and treats
  connector identity/schema as a compatibility boundary.

### Decision implication

`governor-chatgpt-web` may:

- control a locally authenticated dedicated browser;
- observe CDP Target/Page/Network events;
- passively interpret SPA-generated requests/responses;
- perform narrowly bounded authenticated reads where proven robust;
- reconcile ambiguous delivery against exact conversation/message evidence.

It must not:

- reproduce/bypass Sentinel, Turnstile, proof-of-work, CAPTCHA, entitlements, rate
  limits, or anti-abuse controls;
- export browser credentials into a standalone unofficial write client as normal
  architecture;
- automatically replay an uncertain send.

All unofficial ChatGPT behavior stays behind one replaceable crate.

## Rust browser-control comparison

### `chromiumoxide` — recommended first driver

Strengths:

- Rust/Tokio-native async model;
- launch or attach to Chrome through CDP;
- explicit target/page access;
- generated CDP domains including Network events;
- no GUI or embedded-browser packaging requirement.

Costs: lower-level API, Command Governor owns selector/readiness/reconnect logic,
and project cadence is smaller/slower than mainstream JS browser automation.
Mitigation is a narrow internal `BrowserTransport` trait.

### `headless_chrome` — fallback

Credible Rust CDP fallback with headed operation/network interception, but its
blocking/threading ergonomics are less natural for the Tokio daemon.

### Wry — reject for V1 correctness

Wry wraps platform WebViews. V1 has no GUI and needs a uniform Chrome/CDP
Target/Network/profile model, so a cross-platform WebView abstraction is the wrong
correctness boundary.

### CEF — defer

Full embedded Chromium ownership costs browser distribution, update/security,
packaging/signing, sandboxing, and cross-platform lifecycle work. Do not pay that
cost before system Chrome + CDP fails a real requirement.

### Headed vs headless

Current public ChatGPT browser projects provide enough field evidence to support
headed system Chrome first. `--headless=new` is a separate live experiment; do not
add stealth/challenge bypass to make it pass.

## Claude Code research — corrected managed execution model

Primary docs reviewed:

- <https://code.claude.com/docs/en/hooks>
- <https://code.claude.com/docs/en/hooks-guide>
- <https://code.claude.com/docs/en/cli-usage>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/settings>

### Current lifecycle surface

Current Claude documents events including UserPromptSubmit, PreToolUse,
PermissionRequest, PermissionDenied, PostToolUse, PostToolUseFailure,
PostToolBatch, Notification, subagent/task lifecycle, Stop, StopFailure,
SessionStart/End, Elicitation/ElicitationResult, plus programmatic `system/init` and
final `result` messages.

Availability is mode-specific. In particular, the hook guide currently says
`PermissionRequest` hooks **do not fire in non-interactive `-p` mode**.

### Stop-hook finding

Current docs allow Stop hooks to block stopping and state that all matching hooks
run in parallel. `stop_hook_active` exists to prevent continuation loops.

Therefore:

```text
Stop callback -> stop_candidate only
final structured `claude -p` result + matching child exit -> terminal success proof
```

`SessionEnd` is strong session-termination evidence but not successful-result
proof. `StopFailure` is strong failure evidence and must be reconciled with managed
process outcome.

This still fixes the reproduced stale-Herdr class: a **confirmed structured final
or deferred** Claude state beats stale Herdr `working / idle:false`; a lone Stop
callback does not.

### Programmatic output is the stronger primitive

Current Claude programmatic docs describe print/non-interactive operation,
structured stream-json, session metadata/capabilities in the initialization stream,
final result output, and process success/failure semantics. This is a better
completion/result interface than PTY scraping and stronger than one vetoable hook.

### Worker-host/spool

If the daemon itself is the only stdout reader, daemon restart can still lose the
final structured result while Claude continues. V1 therefore introduces a narrow
Rust transport mode:

```text
command-governor worker-host claude <opaque-turn-id>
```

It launches/resumes managed Claude, writes the structured provider stream to an
owner-private bounded spool, writes a sanitized child-exit receipt, and owns no
orchestration state. The daemon later validates it and creates the bounded
immutable result artifact.

### AskUserQuestion / durable input

Current `PreToolUse` supports `defer`; in non-interactive mode this can preserve a
pending tool call such as `AskUserQuestion` for later same-session resume. V1
should project `needs_input` only after the managed structured result confirms the
defer actually occurred.

Unsupported multi-tool shapes remain reconciliation attention rather than a fake
clean pause.

### Permission decisions in managed `-p`

Current Claude hook guidance says `PermissionRequest` hooks do not fire in
non-interactive mode and directs automated permission decisions to `PreToolUse`.
Therefore preferred managed V1 uses `PreToolUse` to classify the exact tool call
before execution:

- already delegated work proceeds only as current settings/permission semantics
  permit;
- out-of-band decisions use confirmed defer when safely supported;
- destructive, credential-sensitive, broader, or unknown actions remain user-owned.

Current docs also indicate a PreToolUse allow cannot necessarily override deny/ask
settings, so Command Governor never treats hook output as authority to widen the
user/managed permission model.

`PermissionRequest` belongs only to a separate interactive/future adapter mode
unless Claude changes the non-interactive contract.

### Settings/hook source behavior

Current settings can merge hooks/configuration across scopes. `--settings` alone
must not be assumed to isolate user/project/plugin hooks. Live Gate C must measure
the actual settings/hook behavior of the selected invocation. The Stop-result model
remains correct even with another Stop hook present.

## Tandem findings

Current `DivMode/tandem` PR #6 remains open at
`af568233e1aae2d4cc343b38ca0e2a1a248e7857` and explicitly says not to merge.
Useful concepts/lessons:

- worker/provider lifecycle evidence is stronger than screen inference when
  available;
- lifecycle deposits can survive daemon restart;
- stale clients/tool schemas are real;
- completion barriers prevent unread worker work from disappearing;
- uncertain sends are not permission to resend.

Command Governor does not inherit Tandem's JavaScript stack, runtime-status
vocabulary, or human notification paths.

## CCCC semantics

CCCC's current Collaboration Standard documents delivery states `claimed`,
`accepted`, `failed`, and `ambiguous`, requiring intent before external I/O and no
blind replay after accepted/ambiguous.

Command Governor independently implements the same safety property in Rust and
adds a durable internal `activation_armed` fence immediately before exact browser
Send. A crash can conservatively produce a zero-send ambiguity; at-most-once safety
wins over guessing.

CCCC is Apache-2.0. We are adopting documented semantics, not copying source. Any
future copied Apache material requires license/NOTICE/provenance handling.

## MCP research and current ChatGPT constraint

Preferred SDK:

- <https://github.com/modelcontextprotocol/rust-sdk>
- `rmcp` `3.1.4` at review time.

The server negotiates the MCP protocol supported by the connecting client.

OpenAI developer-mode/app docs were reviewed. Architecture consequences:

1. local MCP uses the supported OpenAI tunnel/connectivity mechanism rather than
   assuming arbitrary localhost reachability;
2. full custom MCP action availability differs by current ChatGPT plan/surface;
3. app/tool schema compatibility and action availability require explicit handling.

V1 public ABI remains:

- `foreman_bootstrap`
- `foreman_resume`
- `foreman_ack`
- `foreman_answer_input`

Resume/ACK/answer are truthful mutations. A read/fetch-only target surface is
unsupported for the durable automatic foreman loop; no mutation is mislabeled.

Official sources:

- <https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt-beta>
- <https://developers.openai.com/apps-sdk/>

## OpenAI terms / public-project risk

Reviewed:

- OpenAI Terms of Use, effective 2026-01-01:
  <https://openai.com/policies/terms-of-use/>
- OpenAI App Developer Terms, updated 2026-07-09:
  <https://openai.com/policies/app-developer-terms/>

Restrictions relevant to reverse engineering, automated/programmatic extraction,
rate/restriction bypass, and protective measures make a protected private-write
client a poor public-project architecture. Normal local browser login and no
bypass is materially safer but does not make unofficial ChatGPT Web automation
officially supported or remove account/ToU risk.

Public posture:

- normal user authentication;
- no auth/entitlement/CAPTCHA/Turnstile/rate/anti-abuse bypass;
- browser credentials remain local;
- no OpenAI endorsement claim;
- unofficial adapter may break without notice;
- adapter is replaceable if an official API appears.

This is engineering/security risk documentation, not a legal conclusion.

## Rust foundation

Initial candidate baseline, to re-verify at scaffolding:

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
- axum/tower only if loopback HTTP is actually needed.

`anyhow` is acceptable at binary/process boundaries; domain crates use structured
errors.

A contemporaneous August 2026 Rust supply-chain incident included malicious
releases `arrayref 0.3.10`, `internment 0.8.7`, and
`append-only-vec 0.1.9`; initial dependency policy should explicitly reject
known-bad versions in addition to `cargo audit`/`cargo deny`.

## SQLite decision

Prefer `rusqlite` over SQLx SQLite for V1 because the architecture intentionally
has one authoritative daemon and serialized writer. WAL improves reader
concurrency but does not make concurrent writes commit concurrently. A general
async pool does not improve the central transaction model.

Initial store policy:

- bundled SQLite;
- one dedicated DB actor/thread;
- WAL;
- foreign keys;
- bounded busy timeout;
- `synchronous=FULL` initially;
- explicit migrations/schema epoch;
- no ORM.

## Durable result boundary

V1 uses two private content stores outside the safe event ledger:

1. worker-host transport spool — temporary structured provider output surviving
   daemon restart, no control-plane authority;
2. immutable result artifact — bounded final worker result pinned until the open
   obligation is dispositioned and retention permits deletion.

Neither content is put in browser wake text or routine logs.

## Required live gates — not fabricated

### Gate A — ChatGPT MCP mutations

Actual target ChatGPT account/surface must prove genuine state-changing foreman
tools.

### Gate B — headed Chrome + ChatGPT

Dedicated authenticated headed Chrome must prove exact conversation binding,
message-scoped app selection, ten unique wakes, semantic accepted evidence,
crash-at-Send ambiguity/no replay, restart/rebind, and MCP outage behavior.

### Gate C — Claude managed execution

Pinned Claude invocation must prove:

- structured init/capability behavior;
- final result + child exit;
- controlled parallel Stop-hook veto without false completion;
- actual settings/hook source behavior;
- confirmed AskUserQuestion/PreToolUse defer and same-session resume;
- permission decisions at PreToolUse in `-p`, not unavailable PermissionRequest
  hooks;
- daemon-offline worker-host spool recovery;
- stale Herdr working reconciliation;
- forbidden-data non-persistence.

The architecture session did **not** fabricate these live results and did not use
the currently unreliable Tandem/Claude orchestration loop to bootstrap them.

## Recommendation

**Proceed with the small pure Rust core/store/testkit foundation after architecture
review.** Do not yet claim the end-to-end ChatGPT foreman loop or Claude adapter is
supported. Gates A, B, and C remain real provider/product conformance gates.

If a gate fails, replace/disable the adapter instead of weakening durable
obligations, at-most-once ambiguity, or explicit ACK.
