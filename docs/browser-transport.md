# ChatGPT Web browser transport

Status: **architecture selected; live transport gate not yet executed**.

The ChatGPT browser is a narrow transport for waking one explicitly bound
foreman conversation. It is not Command Governor's UI, not a general web browser,
and not a private ChatGPT API emulator.

## Decision

V1 starts with:

- system Google Chrome/Chromium;
- headed mode;
- one dedicated Command Governor-owned profile;
- daemon-owned browser process/supervision;
- CDP controlled from Rust through `chromiumoxide` behind an internal trait;
- DOM interaction only for the structural controls the real SPA requires;
- CDP/network and narrowly bounded authenticated reads for stronger observation
  and reconciliation;
- the real ChatGPT SPA as the authority for sensitive message submission.

`headless_chrome` is the first fallback driver if a measured driver issue appears.
Wry and CEF are not V1 dependencies.

## Why a hybrid

### DOM-only

A DOM-only automation can type and click, but its strongest naive evidence is
usually UI shape: composer cleared, Stop appeared, assistant text changed. Those
are useful structural signals and poor transaction evidence. A DOM-only design
cannot reliably distinguish several post-Send crash cases.

### Fully reversed private API

Current public research shows that direct ChatGPT Web writes are entangled with
changing browser/session/protection machinery. Reproducing those mechanisms would
make Command Governor brittle, move security-sensitive auth material out of its
normal browser context, and create pressure to implement challenge/anti-abuse
workarounds that are explicitly outside project scope.

### Browser-backed hybrid

The real SPA already knows how to authenticate and construct a valid current
submission. CDP lets Command Governor observe the browser's own navigation,
request, response, and target lifecycle without pretending those private details
are a stable product API.

That makes the browser the write mechanism and CDP the evidence plane.

## Browser ownership

Command Governor does not attach to the user's ordinary daily Chrome profile.
`command-governor chatgpt login` launches or adopts only the dedicated governor
profile and lets the user authenticate normally in the visible browser.

The profile directory is credential-equivalent:

- private owner-only directory permissions;
- never committed, synced through project artifacts, or included in diagnostics;
- never parsed into the SQLite event ledger;
- never copied into an API client as the normal design;
- never used by two Chrome processes concurrently.

The daemon stores only a non-secret profile identity/fingerprint sufficient to
fence accidental profile changes.

## Browser process and target model

V1 supports one managed browser process and one active foreman conversation
surface. Auth/login popups may exist transiently during `chatgpt login`, but wake
delivery owns one exact target at a time.

The adapter records/observes:

- browser process incarnation;
- CDP target ID;
- page/frame identity as needed;
- resolved URL;
- canonical ChatGPT conversation ID;
- binding generation.

A Chrome/target restart does not mutate the logical foreman binding. It creates a
new browser process/target incarnation that must re-prove the canonical
conversation before any composer mutation.

## Binding verification before touching the composer

For an existing `/c/<id>` binding:

1. navigate the owned target to the canonical stored URL;
2. wait for document/app-shell readiness with a bounded staged probe;
3. parse the resolved route using a strict ChatGPT URL parser;
4. require resolved canonical conversation ID == stored ID;
5. reject login/access-denied/deleted/displaced/project-wrong routes;
6. verify the composer belongs to that exact conversation surface;
7. verify the Command Governor app/connector is available for the current account;
8. only then may the adapter stage a wake.

Wrong target/redirect is a definite pre-submit failure when observed before the
Send fence. The composer remains untouched.

A staged probe should report which invariant failed rather than one generic
"composer timeout". Useful safe diagnostics include readiness class, route class,
target identity, and element counts/boolean presence; diagnostics must not copy
conversation text or cookies.

## Per-message Command Governor app selection

Current ChatGPT app behavior makes app/tool selection message-scoped rather than a
property Command Governor may assume forever from the conversation.

Therefore every wake must prove, immediately before send, that the **Command
Governor app is selected/mentioned for this specific message** through the
current UI affordance. The implementation may need to use the ChatGPT app picker,
`@` mention flow, or another first-party UI control; the exact selector is an
adapter detail discovered by the live spike.

If the app token/selection cannot be proved, delivery fails before the activation
fence. Command Governor must not send a wake that merely tells ChatGPT in prose to
use a connector that is not actually available to the turn.

## Wake payload

Text is deterministic, tiny, and non-sensitive. Initial shape:

```text
[command-governor wake v1] obligation=<opaque-id> delivery=<opaque-id>. Use the Command Governor app now. Resume this obligation, reconcile the owning worker/result, perform the required review/action, then ACK only after processing.
```

The app-selection UI token is separate from the payload when the ChatGPT surface
represents app selection structurally.

Never include:

- Claude/Codex output;
- code/diffs;
- prompts;
- cwd;
- terminal text;
- session transcript;
- tool arguments;
- browser/session secrets;
- GitHub credentials.

The real content is fetched through authenticated MCP after the foreman wakes.

## CDP evidence model

`chromiumoxide` exposes generated CDP domains. The implementation should consume
only the minimal events needed for safety. Candidate evidence includes:

### Target/Page

- target creation/destruction;
- navigation and frame changes;
- execution-context loss/recreation;
- page lifecycle/readiness.

### Network

- request initiation (`Network.requestWillBeSent` and related redirect metadata);
- response receipt;
- streamed byte/activity evidence (`Network.dataReceived`);
- loading completion/failure;
- request/response IDs that allow exact in-memory correlation.

Private endpoint names and JSON layouts are **observations**, not interfaces. The
adapter may inspect the SPA-generated request in memory to extract a provider
message ID/conversation ID needed for proof. It must not log or persist whole
request bodies, headers, cookies, or wake-bearing protocol dumps.

## Accepted submission evidence

The goal is to prove:

> The exact intended wake user message was submitted by the exact bound page to
> the exact bound ChatGPT conversation.

Preferred evidence is a conjunction such as:

- correlated SPA submission request from the owned target;
- provider-generated user message identifier or equivalent exact message-tree
  identity;
- conversation identifier matches the current binding;
- message content/opaque delivery ID matches in memory;
- later conversation/message-tree observation contains that exact new user
  message.

The live spike determines the strongest stable subset currently observable.

Weak signals alone never qualify:

- composer cleared;
- Send button changed to Stop;
- URL changed;
- assistant started generating;
- text appeared somewhere in the document.

## Physical assistant turn evidence

Physical settlement is useful only to decide when a bounded resume might be
considered; it never closes the obligation.

Prefer correlated network/message-tree evidence that the assistant turn has
started and completed. Current ChatGPT streaming mechanics may expose response
headers and `Network.dataReceived` without giving a convenient complete SSE body.
That is acceptable: Command Governor needs lifecycle evidence, not a copied
transcript.

A direct conversation read may corroborate the final assistant node if the
read-only path remains robust. DOM completion indicators are fallback evidence.
If the adapter cannot determine whether a turn is active, the safe state is
`observation_lost`; no new wake overlaps it.

## Private read/observation layer

Allowed:

- passive interpretation of requests/responses produced by the real page;
- browser-context authenticated reads;
- direct authenticated conversation/message-tree reads only when they are shown
  by tests to be stable and require no protective-mechanism reproduction;
- model/account metadata reads useful for diagnostics.

Not allowed:

- a direct private message-submit client;
- copied browser bearer/cookie jar as the primary API identity;
- Sentinel/Turnstile/PoW implementation;
- CAPTCHA solving/bypass;
- entitlement/model/rate-limit bypass;
- retry strategies intended to evade abuse controls.

When a read endpoint drifts, reconciliation may become less convenient. That must
not cause the adapter to fall back to an unofficial direct write.

## At-most-once Send boundary

Before any browser I/O, SQLite records the attempt `claimed`.

Immediately before the exact composer-local Send action:

1. re-verify target conversation ID;
2. re-verify app selection;
3. re-verify staged payload identity;
4. commit `activation_armed` to SQLite;
5. invoke exactly one Send action.

The commit in step 4 deliberately precedes physical I/O. If the daemon dies
between 4 and 5, recovery says ambiguous even though zero messages may have been
sent. This is the safe side of the impossibility boundary.

After Send activation there is no generic "try click again" path.

## Send activation mechanism

The live adapter should use one exact composer-local control. Candidate hierarchy:

1. a stable first-party/test-id Send control scoped to the verified composer;
2. an accessible-name Send control scoped to the verified composer;
3. a structurally unique submit button inside the verified composer form.

Never use a page-global generic submit selector.

Keyboard Enter may be an implementation fallback only if the live spike proves it
is the exact first-party submit behavior and all multiline/composition states are
fenced. The ambiguity boundary is the invocation of whichever exact action is
chosen.

## Single-flight behavior

V1 has one browser wake worker. It serializes:

- target reconciliation;
- app selection;
- staging;
- Send activation;
- accepted/ambiguous evidence collection.

This prevents two obligations from racing the same composer and simplifies exact
message correlation. Concurrency belongs in worker execution, not in the one
foreman wake surface.

## Reconnect and crash recovery

Browser process death:

- does not close obligations;
- does not reset delivery IDs;
- does not cause accepted/ambiguous sends to replay;
- creates a new browser incarnation;
- requires profile/binding verification before any new delivery.

Daemon restart first converts orphaned `claimed`/`activation_armed` attempts to
`ambiguous`, then starts Chrome/CDP reconciliation. It never starts the browser
and "continues where it left off" by clicking Send.

## Driver comparison

| Requirement | chromiumoxide | headless_chrome | Wry | CEF Rust |
| --- | --- | --- | --- | --- |
| Rust-first | yes | yes | yes | yes |
| Tokio-native | strong | weaker/blocking-oriented | event-loop oriented | custom/heavy |
| Attach/launch Chrome CDP | yes | yes | no uniform system-Chrome model | owns embedded CEF |
| Full Network/Target evidence | strong generated CDP | strong CDP | platform-specific | strong Chromium internals |
| Persistent Chrome profile | yes | yes | platform WebView stores | yes, but we own all CEF lifecycle |
| Headed mode | yes | yes | yes | yes |
| Packaging burden | low | low/medium | medium if UI existed | very high |
| V1 result | **select** | fallback | reject | defer |

## Required authenticated live spike

This spike is a **gate**. Unit tests and source review cannot substitute for it.

### Prerequisites

- dedicated Command Governor Chrome profile;
- normal user login completed manually;
- test Command Governor MCP app/connector installed through the supported ChatGPT
  path;
- a disposable ChatGPT conversation explicitly bound;
- fake local obligations/results so no real worker is needed;
- browser/daemon failpoints enabled;
- no Tandem orchestration dependency.

### Headed Chrome run

Record exact Chrome version, OS, `chromiumoxide` version/commit, ChatGPT account
plan/surface, connector ABI, and test timestamp.

Tests:

1. **Login persistence** — restart browser three times; session remains usable or
   fails with explicit `auth_required`, never credential scraping.
2. **Exact bind** — bind `/c/A`; navigation to `/c/B`, `/`, deleted chat, project
   redirect, and auth page all fail before composer mutation.
3. **App selection** — ten wake turns each independently prove the Command Governor
   app is selected for that message.
4. **Ten unique wakes** — submit ten unique delivery IDs sequentially. Expected:
   ten and only ten user-message identities in the bound conversation.
5. **Network evidence** — for every accepted wake, capture safe extracted evidence
   sufficient to bind target + conversation + user-message identity without
   persisting request secrets/body.
6. **Pre-Send failure** — deliberately break composer/app readiness. Expected:
   `failed`, safe retry allowed, zero message.
7. **Crash after claim** — kill daemon after durable `claimed`. Restart must first
   mark attempt ambiguous and must not Send.
8. **Crash after activation fence / before CDP command** — expected ambiguous,
   zero or one message, never automatic retry.
9. **Crash immediately after CDP Send activation** — expected ambiguous unless
   reconciliation proves accepted; never a second message.
10. **Ambiguous reconciliation** — when exact user-message identity exists,
    promote ambiguous to accepted without Send; when evidence is absent/unclear,
    remain ambiguous.
11. **Physical settlement != ACK** — allow ChatGPT turn to finish while fake MCP
    never ACKs. Obligation must remain outstanding.
12. **Bounded resume** — after policy delay, one new delivery revision may wake
    the same obligation; never overlap a live/unknown ChatGPT turn.
13. **Rebind generation** — bind chat B, increment generation; a delayed tool call
    carrying generation A must receive stale-generation error and cannot ACK.
14. **Browser crash/restart** — kill Chrome after an accepted/settled wake; restart
    profile and confirm no replay.
15. **MCP outage** — make connector unavailable. Wake must not be treated as
    processed; `doctor` exposes capability failure and obligation persists.

### Headless comparison

With Chrome fully stopped and the same dedicated profile used sequentially, run
an equivalent `--headless=new` matrix:

- auth persistence;
- navigation;
- app selection;
- ten wakes;
- network evidence;
- challenge/bot-detection frequency;
- restart/reconciliation.

Do not relax anti-bot protections or add stealth/challenge bypass to make headless
pass. If headless is materially less reliable, V1 remains headed.

### Pass criteria

Headed support requires:

- 10/10 unique accepted wakes with no duplicate user message;
- 100% wrong-chat fencing before composer mutation;
- 100% crash cases preserve at-most-once behavior;
- all ambiguous cases remain non-replayed unless exact reconciliation promotes
  them to accepted;
- 100% stale-generation ACKs rejected;
- app selection proven for every wake;
- no credential/protocol-body leakage in safe logs/state.

A single duplicate Send is a **gate failure**, not an acceptable flaky test.

## Current spike result

**Not executed as of 2026-08-31.** This architecture session has no authorized
local Command Governor Chrome profile, and the project explicitly forbids using
the currently unreliable Tandem/Claude loop as bootstrap machinery. No live
result is fabricated.

The first implementation milestone that touches real ChatGPT must produce a
versioned spike report with raw *safe* evidence summaries and exact software
versions before the adapter can be marked supported.
