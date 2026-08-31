# Command Governor technology review — 2026-08-31

Status: **architecture input**. This report pins the public sources reviewed before
V1 implementation. It is intentionally date- and commit-specific because several
of the interfaces below are private, pre-release, or changing quickly.

## Executive finding

The current architecture hypothesis is substantially correct, with two important
qualifications:

1. **ChatGPT submission should remain browser-owned.** Current independent
   implementations converge on a hybrid: let the authenticated ChatGPT SPA perform
   sensitive writes, while using browser/CDP and carefully bounded authenticated
   reads for observation/reconciliation. Reimplementing Sentinel, proof-of-work,
   Turnstile, or other anti-abuse machinery is both brittle and outside Command
   Governor's intended security posture.
2. **A write-capable ChatGPT MCP surface is a deployment gate, not an assumption.**
   OpenAI's current published ChatGPT developer-mode documentation says custom MCP
   on Pro has read/fetch access, while full MCP actions are currently a beta for
   Business/Enterprise/Edu. An explicit foreman ACK is a state-changing operation;
   Command Governor must feature-detect a write-capable connector on the actual
   bound foreman surface and fail closed if it is unavailable. We will not label a
   mutating ACK as a read operation to work around product policy.

The recommended V1 remains a Rust daemon + CLI, one SQLite authority, native
worker lifecycle signals, a dedicated headed system-Chrome profile controlled via
CDP, a small stable MCP ABI, and explicit at-most-once ambiguity semantics.

## Pinned source snapshot

| Project / source | Reviewed revision / release | Date observed | Relevant evidence |
| --- | --- | --- | --- |
| `DivMode/commandgovernor` | `fd3e5a61425f00ee3b164d2a840708602f972342` | 2026-08-31 | Pre-implementation docs baseline. |
| `miuuyy/codex-chatgpt-web` | main `d7675fc7767a8f19b908f3e5d0e357699d1d9fdf`; release `v4.0.7` | 2026-08-31 | Retained conversations, exact surface ownership, browser lifecycle, stable connector identity, compaction handoff. |
| `ChesterRa/CCCC` | main `5f0b83242d09c88b1e2267d1056fc5bf64feb626` | 2026-08-31 | Append-only daemon authority and `claimed/accepted/failed/ambiguous` delivery protocol. |
| `DivMode/tandem` | main `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee`; PR #6 head `af568233e1aae2d4cc343b38ca0e2a1a248e7857` | 2026-08-30/31 | Completion barrier, stale-client problem, Claude native lifecycle work. PR #6 remains open and review-only. |
| `Maxmedawar/tandem` | main `a98bcafd2c40ae5473b85fe41183e4f391933799` | 2026-08-12 | Upstream fleet/session/MCP/runtime baseline. |
| OpenWeb (`imoonkey/openweb`; former `openweb-org/openweb` URL redirects) | main `a387b50c829d871839a613732e1b97bfa1946124` | 2026-06-27 | Hybrid browser/page transport and ambient authenticated-session handling. |
| `Octo-Lex/ChatGPT-Web2API` | master `497527dceabfa3f95961e23c291e618c5570f1ac` | 2026-07-12 | CDP architecture, backend-read reconciliation, send non-replay, navigation diagnostics. |
| `stufently/gpt-web-gateway` | main `efb01a32e9e4c7fbebb8acff204c8c2a448c476c`; app `2.14.1` | 2026-08-19 | Real-browser write path, passive/backend observation, headed-browser field evidence, pacing/non-retry. |
| `modelcontextprotocol/rust-sdk` / `rmcp` | main `3a2ebbc92034f6711e4eac9e93f5de0423be8dfe`; crate `3.1.4` | 2026-08-29/31 | Official Rust MCP SDK; support for current protocol generations including 2026-07-28 types. |
| `mattsse/chromiumoxide` | main `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`; crate `0.9.1` | 2026-04-03 / 2026-02-25 | Tokio-native CDP, attach to existing Chrome, targets/pages, generated Network domain. |
| `rust-headless-chrome/rust-headless-chrome` | `0a5c307a85debc450378a1f19e4dac1838d7b22d`; `1.0.22` | 2026-06-11 | Mature higher-level CDP alternative; useful fallback, less natural fit for Tokio daemon design. |
| `tauri-apps/wry` | dev `bb69d628a905d65042c71a95e85f6921ec9b3264` | 2026-08-31 | Native OS WebView abstraction, not a uniform Chromium/CDP target model. |
| `tauri-apps/cef-rs` | dev `a2e15ae659c4b3957883e34de879bd8b38360ce5` | 2026-08-23 | Full embedded Chromium option with significant packaging/update/security cost. |

Pinned links:

- <https://github.com/miuuyy/codex-chatgpt-web/tree/d7675fc7767a8f19b908f3e5d0e357699d1d9fdf>
- <https://github.com/ChesterRa/CCCC/tree/5f0b83242d09c88b1e2267d1056fc5bf64feb626>
- <https://github.com/DivMode/tandem/pull/6>
- <https://github.com/Maxmedawar/tandem/tree/a98bcafd2c40ae5473b85fe41183e4f391933799>
- <https://github.com/imoonkey/openweb/tree/a387b50c829d871839a613732e1b97bfa1946124>
- <https://github.com/Octo-Lex/ChatGPT-Web2API/tree/497527dceabfa3f95961e23c291e618c5570f1ac>
- <https://github.com/stufently/gpt-web-gateway/tree/efb01a32e9e4c7fbebb8acff204c8c2a448c476c>
- <https://github.com/modelcontextprotocol/rust-sdk/tree/3a2ebbc92034f6711e4eac9e93f5de0423be8dfe>
- <https://github.com/mattsse/chromiumoxide/tree/afcc3a4313f2087249b4490d94e54bf8e3bfaccf>

## ChatGPT Web transport research

### What current implementations agree on

The write path is the unstable and security-sensitive part of ChatGPT Web. The
current private flow includes authenticated browser/session state plus protected
request machinery that has changed over time. Public reverse-engineering projects
have encountered Sentinel-related requirements, proof/challenge fields,
Turnstile/Cloudflare behavior, private request-schema drift, model-slug drift, and
streaming differences.

The useful convergence is architectural rather than protocol-specific:

- **OpenWeb:** uses browser/page context where ambient browser identity matters;
  its ChatGPT work documents the distinction between comparatively tractable reads
  and protected write submission.
- **ChatGPT-Web2API:** deliberately drives sensitive sends through real Chrome/CDP,
  reconciles through backend observations where useful, and explicitly avoids
  replaying a send when a generation/stall leaves the outcome uncertain.
- **gpt-web-gateway:** uses real-browser interaction for the sensitive path and
  passive/backend observation as an optimization; its current field configuration
  defaults to headed Chrome.
- **codex-chatgpt-web v4:** owns exact browser surfaces, retains conversation
  identity across sequential work, separates submission from physical turn
  settlement, and treats connector identity/schema as an ABI rather than a bag of
  ad-hoc tools.

### Decision implication

Command Governor must **not** become a private ChatGPT API emulator. The
`governor-chatgpt-web` adapter may:

- control a locally authenticated browser;
- observe CDP Network/Page/Target events;
- passively inspect requests/responses produced by the real SPA;
- perform narrowly bounded authenticated reads when they prove stable; and
- reconcile an ambiguous delivery using exact message/conversation evidence.

It must not:

- reproduce or bypass Sentinel, Turnstile, proof-of-work, CAPTCHA, rate limits,
  entitlement checks, or anti-abuse controls;
- export browser credentials into a standalone unofficial API client as the
  normal architecture; or
- automatically replay a send whose side effect is uncertain.

All unofficial ChatGPT behavior stays behind one replaceable crate boundary.

## Browser-control comparison

### `chromiumoxide` — recommended V1 driver

Strengths:

- Rust and Tokio-native async model;
- launches or attaches to Chrome through CDP;
- explicit target/page access;
- generated CDP protocol types, including `Network` events needed for request,
  response, and streaming evidence;
- no GUI framework or embedded-browser application architecture required.

Risks:

- lower-level API than Playwright;
- we will own selector/readiness/reconnect behavior;
- project cadence is slower than the JS browser-automation ecosystem.

Mitigation: define an internal `BrowserTransport` trait and keep ChatGPT-specific
logic out of the generic browser boundary so a fallback can be substituted.

### `headless_chrome` — fallback candidate

Strengths include mature high-level Rust CDP APIs, headed operation, network
interception, and a known-good Chromium path. It is a credible fallback if the
first spike reveals a material `chromiumoxide` reliability gap.

It is not the first choice because its threading/blocking ergonomics are a less
natural fit for a Tokio daemon and Command Governor needs unusually explicit CDP
state/evidence handling rather than only high-level page automation.

### Wry — reject for V1 correctness path

Wry gives a Rust wrapper over platform WebViews (WKWebView on macOS, WebView2 on
Windows, WebKitGTK on Linux). That is attractive for an application UI but weak
for this job: browser/profile behavior differs by OS, target ownership is not a
uniform Chrome/CDP concept, and the exact Network/Target evidence we want is not a
single cross-platform contract. V1 has no conventional GUI, so Wry adds no needed
value.

### CEF — defer

CEF can provide complete embedded Chromium ownership, but it also makes Command
Governor responsible for a large browser binary, browser security updates,
packaging, signing, sandbox integration, and multi-platform lifecycle. That is a
very high price before system Chrome + CDP has failed a real requirement.

### Headed versus headless

Current public ChatGPT automation projects disagree in implementation detail, but
multiple current projects report materially better field behavior with a real
headed browser. Therefore V1 support is **headed Google Chrome/Chromium with a
dedicated profile**. `--headless=new` is an experiment until the required live
spike proves equivalent behavior; it is not a correctness promise.

## Claude Code lifecycle research

Current Claude Code documentation was reviewed at:

- <https://code.claude.com/docs/en/hooks>

Important current events include `UserPromptSubmit`, `PreToolUse`,
`PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`,
`Notification`, task/subagent events, `Stop`, and `StopFailure`.

For Command Governor:

- `Stop` is authoritative evidence that Claude ended a response; stale PTY
  `working` state must not override it.
- `StopFailure` is authoritative failure evidence for that turn.
- `PermissionRequest` is immediate blocking evidence when permission is required.
- `PostToolUse`/batch events can update only bounded `last_progress_at` metadata.
- `Notification` values such as `agent_needs_input` are useful corroborating
  evidence, but delayed notifications must not be the sole blocking detector.
- `PreToolUse` currently supports a `defer` decision. In non-interactive
  `claude -p`, an `AskUserQuestion` call can be deferred, the process exits with
  the pending tool call preserved, and the same session can later be resumed.
  This is a much stronger primitive for deterministic V1 `needs_input` handling
  than scraping a terminal prompt.

The hook payload is treated as untrusted input. Command Governor extracts only
bounded identifiers/event classes/timestamps required for lifecycle fencing. It
must not persist prompt text, tool arguments, shell commands, cwd, transcript
path, terminal transcript, or secrets.

## Tandem findings to carry forward — and not carry forward

Current `DivMode/tandem` PR #6 is still open at
`af568233e1aae2d4cc343b38ca0e2a1a248e7857`. Its useful ideas include:

- native Claude completion evidence;
- a durable lifecycle store;
- completion barriers that prevent declaring success over unread worker work;
- recognition that stale MCP conversations can retain stale tool schemas; and
- the rule that an apparently unchanged send result is not permission to resend.

The reproduced failure class remains a mandatory regression case: Claude may be
at a real input/interruption boundary while a runtime such as Herdr still reports
`working`. Command Governor's native lifecycle projection must win.

We do **not** inherit Tandem's process-status vocabulary, JavaScript stack, or
human-notification paths as control-plane truth.

## CCCC semantics to independently implement

CCCC's current Collaboration Standard is especially relevant because it states a
transport-independent delivery fact model. Its `runtime.delivery` semantics use
`claimed`, `accepted`, `failed`, and `ambiguous`; require `claimed` before
external I/O; turn an orphaned startup `claimed` into `ambiguous`; and prohibit
automatic retry after `accepted` or `ambiguous`.

Command Governor will independently implement the same safety property in Rust,
with an additional internal `activation_armed` ambiguity fence immediately before
invoking the exact browser send action. This intentionally permits a crash window
that can produce **zero** sends rather than risking two sends.

CCCC is Apache-2.0. We are adopting documented semantics, not copying its code.
Any future copied Apache-licensed material requires the original license and
NOTICE/provenance handling.

## MCP research and current ChatGPT constraint

The preferred server SDK remains the official Rust SDK:

- <https://github.com/modelcontextprotocol/rust-sdk>
- crate `rmcp` `3.1.4` at the time of this review.

Command Governor will negotiate the protocol supported by the connecting client
rather than assuming one hard-coded date. The current SDK contains types for the
2026-07-28 protocol generation.

OpenAI's current ChatGPT developer-mode/custom-app documentation was also
reviewed. Two product behaviors are architecture-significant:

1. local MCP is not simply connected as an arbitrary localhost URL; current
   guidance uses OpenAI's Secure MCP Tunnel for local development/connectivity;
2. custom app/tool changes are not an invisible live schema mutation: connector
   refresh/action enablement matters, and the selected app applies to the message
   being sent.

Therefore the browser wake spike must verify not just that text can be submitted,
but that the **Command Governor app is selected/mentioned for that exact wake
message** and its stable tools are available to the resulting foreman turn.

Current published availability also means a Pro-only deployment cannot be
assumed to support `foreman_ack` today. The daemon's `doctor`/binding preflight
must test the actual surface and report an explicit unsupported capability rather
than weakening the ACK invariant.

Relevant official sources:

- <https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt-beta>
- <https://developers.openai.com/apps-sdk/>

## OpenAI terms / public-project risk

Reviewed current public terms:

- OpenAI Terms of Use, effective 2026-01-01:
  <https://openai.com/policies/terms-of-use/>
- OpenAI App Developer Terms, updated 2026-07-09:
  <https://openai.com/policies/app-developer-terms/>

The Terms include restrictions around reverse engineering, programmatic
extraction, circumvention of rate limits/restrictions, and bypassing protective
measures. A user-controlled local browser, normal authentication, and no
challenge bypass is a materially safer architecture, but it does **not** make an
unofficial ChatGPT-Web automation officially supported or eliminate account/ToU
risk.

The public project must say this plainly:

- ChatGPT Web browser transport is unofficial and may break without notice;
- users authenticate through the normal service UI;
- Command Governor does not bypass CAPTCHA/Turnstile, entitlements, rate limits,
  or anti-abuse protections;
- browser credentials remain in the user's private local profile;
- no OpenAI endorsement is implied; and
- if an official wake/foreman API becomes available, the unofficial adapter is
  designed to be replaceable.

## Rust foundation

Recommended initial toolchain and dependency baseline, to be re-verified at the
actual scaffold commit:

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
- axum/tower only if a loopback HTTP endpoint is actually required.

`anyhow` is acceptable at binary/process boundaries; domain crates should expose
structured errors with `thiserror`.

A contemporaneous Rust ecosystem supply-chain incident affected malicious
releases of `arrayref 0.3.10`, `internment 0.8.7`, and
`append-only-vec 0.1.9`. The initial lockfile/CI must explicitly reject known-bad
versions in addition to normal `cargo audit` and `cargo deny` checks.

## SQLite decision

`rusqlite` is preferred over SQLx for V1 because Command Governor deliberately
has one authoritative daemon and can serialize writes through one database actor.
SQLite WAL improves reader concurrency but still has one writer; a general async
connection pool does not create additional write concurrency and can obscure the
transaction boundary we care about most.

The proposed store uses:

- bundled SQLite for deterministic application packaging;
- one dedicated DB actor/thread and typed requests from async tasks;
- WAL;
- `foreign_keys=ON`;
- a bounded busy timeout;
- `synchronous=FULL` until crash-injection evidence justifies relaxing it;
- explicit migrations and schema epoch checks; and
- explicit transactions, using an early write lock for state transitions where
  compare-and-swap/uniqueness fencing matters.

No ORM is required.

## New requirement discovered: durable result artifacts

A `completed_unprocessed` row is not sufficient if the worker's only useful
result is still trapped in a terminal/session that can disappear. The control
ledger must not become a transcript archive, but the result needed to process an
obligation must survive the runtime.

V1 therefore needs a **private result-artifact store** outside the event payloads:

- daemon-owned private directory (`0700`, artifact files `0600` on Unix);
- immutable artifact reference, size, digest, creation event, and retention state
  in SQLite;
- payload may contain the final worker result required for review, but never gets
  copied into wake messages or general logs;
- MCP fetch treats it as untrusted repository/worker content;
- retention/deletion is explicit and cannot precede the closing obligation ACK.

Git commits/PRs/session references can be additional result references, not the
only durability guarantee.

## Live browser spike: required, not fabricated

A real authenticated ChatGPT browser spike is necessary before the ChatGPT
transport can be declared supported. It was **not executed by this research
session** because this environment has no authorized local Command Governor Chrome
profile and the user explicitly prohibited using the currently unreliable Tandem
loop to bootstrap the project.

That is a release gate, not a missing claim. The exact spike protocol is in
[`../browser-transport.md`](../browser-transport.md). It must cover at least:

- normal login and auth persistence;
- exact `/c/<id>` binding and wrong-chat fencing;
- exact Command Governor app selection for every wake;
- ten unique wake submissions;
- CDP request/user-message evidence;
- assistant physical settlement without equating it to ACK;
- kill/restart at the activation boundary;
- reconciliation of an ambiguous delivery without resend;
- browser restart and profile reuse;
- rebind generation fencing; and
- a separate `--headless=new` comparison.

## Recommendation

**Proceed with the architecture and pure core/store/testkit foundation, but do
not claim the ChatGPT-Pro V1 loop is deployable until two red gates pass:**

1. the actual bound ChatGPT surface proves write-capable MCP actions for explicit
   ACK/input response; and
2. the headed system-Chrome + CDP spike proves exact binding, app selection,
   accepted-delivery evidence, and crash reconciliation.

If gate 1 fails on the target Pro account, preserve the architecture and mark
that platform combination unsupported; do not replace explicit ACK with browser
settlement or a disguised read operation. If gate 2 fails, evaluate
`headless_chrome` or, only after lighter options are exhausted, a heavier browser
ownership strategy. The durable domain model should not change in either case.
