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
- DOM interaction only for structural controls the real SPA requires;
- CDP/network and narrowly bounded authenticated reads for stronger observation
  and reconciliation;
- the real ChatGPT SPA as the authority for sensitive message submission.

`headless_chrome` is the first fallback driver if a measured driver issue appears.
Wry and CEF are not V1 dependencies.

## Current MCP surface dependency

A browser wake is useful only if the selected ChatGPT foreman surface can call the
state-changing MCP contract after waking. As of the 2026-08-31 review, consumer
ChatGPT Pro custom MCP is documented read/fetch-only, so it is not an end-to-end V1
target. Business/Enterprise/Edu candidate workspaces must pass Gate A before this
browser transport can be considered a complete foreman loop. Business currently
exposes GPT-5.6 Sol Pro, so using the Pro model does not require weakening the MCP
write requirement.

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
"composer timeout". Safe diagnostics may include readiness class, route class,
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

If app selection cannot be proved, delivery fails before the activation fence.
Command Governor must not send a wake that merely tells ChatGPT in prose to use a
connector that is not actually available to the turn.

## Delivery identity and wake payload

Do not overload one identifier with two jobs.

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)

delivery_id = CSPRNG(>=192 bits)
```

`delivery_key` is the deterministic non-secret idempotency/deduplication key for
one scheduled revision. `delivery_id` is generated once when the durable delivery
is created and is the random anti-confusion correlation value carried in the wake.
It is not returned by bootstrap/status and cannot be derived from the deterministic
inputs.

Wake text is tiny and non-sensitive. Initial shape:

```text
[command-governor wake v1] obligation=<opaque-id> delivery=<random-opaque-id>. Use the Command Governor app now. Resume this obligation, reconcile the owning worker/result, perform the required review/action, then ACK only after processing.
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
only the minimal events needed for safety.

Candidate Target/Page evidence:

- target creation/destruction;
- navigation and frame changes;
- execution-context loss/recreation;
- page lifecycle/readiness.

Candidate Network evidence:

- request initiation (`Network.requestWillBeSent` and redirect metadata);
- response receipt;
- streamed byte/activity evidence (`Network.dataReceived`);
- loading completion/failure;
- request/response IDs for exact in-memory correlation.

Private endpoint names and JSON layouts are **observations**, not interfaces. The
adapter may inspect the SPA-generated request in memory to extract a provider
message ID/conversation ID needed for proof. It must not log or persist whole
request bodies, headers, cookies, or private protocol dumps.

## Accepted submission evidence

The goal is to prove:

> The exact intended wake user message was submitted by the exact bound page to
> the exact bound ChatGPT conversation.

Preferred evidence is a conjunction such as:

- correlated SPA submission request from the owned target;
- provider-generated user message identifier or equivalent exact message-tree
  identity;
- conversation identifier matches the current binding;
- random `delivery_id` matches the intended staged wake in memory;
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
started and completed. A direct conversation read may corroborate the final node
if the read-only path remains robust. DOM completion indicators are fallback
evidence. If the adapter cannot determine whether a turn is active, the safe state
is `observation_lost`; no new wake overlaps it.

## Private read/observation layer

Allowed:

- passive interpretation of requests/responses produced by the real page;
- browser-context authenticated reads;
- direct authenticated conversation/message-tree reads only when tests show them
  stable and requiring no protective-mechanism reproduction;
- model/account metadata reads useful for diagnostics.

Not allowed:

- a direct private message-submit client;
- copied browser bearer/cookie jar as the primary API identity;
- Sentinel/Turnstile/PoW implementation;
- CAPTCHA solving/bypass;
- entitlement/model/rate-limit bypass;
- retry strategies intended to evade abuse controls.

When a read endpoint drifts, reconciliation may become less convenient. That must
not cause fallback to an unofficial direct write.

## At-most-once Send boundary

Before any browser I/O, SQLite records the attempt `claimed`.

Immediately before the exact composer-local Send action:

1. re-verify target conversation ID;
2. re-verify app selection;
3. re-verify staged payload and random `delivery_id`;
4. re-verify target obligation version/source/binding generation;
5. commit `activation_armed` to SQLite;
6. invoke exactly one Send action.

The commit deliberately precedes physical I/O. If the daemon dies between 5 and
6, recovery says ambiguous even though zero messages may have been sent. This is
the safe side of the external-I/O impossibility boundary.

After Send activation there is no generic "try click again" path.

## Send activation mechanism

Use one exact composer-local control. Candidate hierarchy:

1. a stable first-party/test-id Send control scoped to the verified composer;
2. an accessible-name Send control scoped to the verified composer;
3. a structurally unique submit button inside the verified composer form.

Never use a page-global generic submit selector.

Keyboard Enter may be a fallback only if the live spike proves it is the exact
first-party submit behavior and all multiline/composition states are fenced. The
ambiguity boundary is invocation of whichever exact action is chosen.

## Single-flight behavior

V1 has one browser wake worker. It serializes target reconciliation, app selection,
staging, Send activation, and accepted/ambiguous evidence collection. This
prevents two obligations from racing one composer and simplifies exact message
correlation.

## Reconnect and crash recovery

Browser process death:

- does not close obligations;
- does not reset delivery rows/IDs;
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

- Gate A has identified a write-capable ChatGPT workspace surface; consumer Pro is
  not currently eligible under published product policy;
- dedicated Command Governor Chrome profile;
- normal user login completed manually;
- test Command Governor MCP app/connector installed through the supported ChatGPT
  path;
- disposable ChatGPT conversation explicitly bound;
- fake local obligations/results so no real worker is needed;
- browser/daemon failpoints enabled;
- no Tandem orchestration dependency.

### Headed Chrome run

Record exact Chrome version, OS, `chromiumoxide` version/commit, ChatGPT workspace
plan/surface/model, connector ABI, and test timestamp.

Tests:

1. **Login persistence** — restart browser three times; session remains usable or
   fails with explicit `auth_required`, never credential scraping.
2. **Exact bind** — bind `/c/A`; navigation to `/c/B`, `/`, deleted chat, project
   redirect, and auth page all fail before composer mutation.
3. **App selection** — ten wake turns each independently prove the Command Governor
   app is selected for that message.
4. **Ten unique wakes** — submit ten random `delivery_id` values sequentially.
   Expected: ten and only ten user-message identities in the bound conversation.
5. **Network evidence** — every accepted wake yields safe extracted target +
   conversation + user-message evidence without persisting request secrets/body.
6. **Pre-Send failure** — deliberately break composer/app readiness. Expected
   `failed`, safe retry allowed, zero message.
7. **Crash after claim** — kill daemon after durable `claimed`; restart marks
   attempt ambiguous before browser recovery and does not Send.
8. **Crash after activation fence / before CDP command** — ambiguous, zero or one
   message, never automatic retry.
9. **Crash immediately after CDP Send activation** — ambiguous unless exact
   reconciliation proves accepted; never a second message.
10. **Ambiguous reconciliation** — exact user-message identity promotes to accepted
    without Send; absent/unclear evidence leaves ambiguous.
11. **Physical settlement != ACK** — let ChatGPT finish while fake MCP never ACKs;
    obligation remains outstanding.
12. **Bounded resume** — after policy delay, one new delivery revision/new random
    `delivery_id` may wake the same obligation; never overlap a live/unknown turn.
13. **Rebind generation** — bind chat B; delayed generation-A tool call cannot ACK.
14. **Browser crash/restart** — kill Chrome after accepted/settled wake; no replay.
15. **MCP outage** — connector unavailable; wake is not processed and obligation
    persists.
16. **Correlation non-derivability** — a caller given bootstrap output plus
    obligation/generation/revision cannot reconstruct the random `delivery_id`.

### Headless comparison

With Chrome fully stopped and the same dedicated profile used sequentially, run an
equivalent `--headless=new` matrix for auth, navigation, app selection, ten wakes,
network evidence, challenge frequency, and restart/reconciliation.

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
- random wake correlation cannot be derived from bootstrap/deterministic metadata;
- no credential/protocol-body leakage in safe logs/state.

A single duplicate Send is a **gate failure**, not an acceptable flaky test.

## Current spike result

**Not executed as of 2026-08-31.** This architecture review has no authorized local
Command Governor Chrome profile and does not fabricate a live result.

The first implementation milestone that touches real ChatGPT must produce a
versioned spike report with raw *safe* evidence summaries and exact software
versions before the adapter can be marked supported.
