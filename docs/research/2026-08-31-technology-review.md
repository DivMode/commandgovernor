# Command Governor technology review — 2026-08-31

Status: **reviewed architecture input / pinned public-source snapshot**.

This report records the public sources re-verified before V1 implementation. It
also records corrections found during the independent architecture review. Exact
SHAs matter because several projects moved during the same day.

No live Command Governor ChatGPT/Claude adapter result is claimed here. Live
behavior remains gated separately.

## Executive conclusion

The strongest V1 design remains:

- Rust daemon + CLI; no conventional GUI;
- one authoritative daemon and one single-writer SQLite database (`rusqlite`);
- immutable event/projection model plus durable obligations;
- separate private result-artifact store;
- managed Claude worker-host that parses structured output online but never
  durably spools the complete provider stream;
- official Rust MCP SDK (`rmcp`) with a four-tool stable foreman ABI;
- dedicated headed system Chrome profile;
- Rust CDP via `chromiumoxide` behind a replaceable browser trait;
- real ChatGPT SPA performs sensitive browser submission;
- CDP/network/message evidence is the primary observation/reconciliation plane;
- no private protected ChatGPT write client or challenge bypass;
- browser delivery uses deterministic dedupe identity plus a separate random
  accepted-wake correlation ID;
- explicit foreman ACK is the only normal closure of processed work.

The independent review found important corrections rather than merely validating
the first draft. Those corrections are now architectural requirements.

## Independent-review corrections

### 1. ChatGPT support must be capability-based, not plan-name-based

The initial review treated OpenAI's published developer-mode plan matrix as a hard
support boundary. Those documents remain important compatibility evidence, but a
live test later on 2026-08-31 disproved the categorical assumption for the actual
target surface.

Using a fresh ChatGPT conversation with the private Tandem app explicitly attached,
the target ChatGPT Pro account/app/surface successfully performed state-changing
MCP operations: it listed sessions, opened a disposable Claude session, sent a
mutation that created/overwrote a host filesystem file, and read the result back as
`MCP WRITE VERIFIED`. No plan/read-only/confirmation/permission rejection occurred
on the writes.

ADR 0006 therefore supersedes plan-name gating. Command Governor support is based
on a harmless synthetic mutation/read-back against the exact bound
account/app/surface, with stale-generation and confirmation checks recorded under a
`capability_epoch`. Plan labels remain diagnostic metadata, and a past successful
probe is not treated as a permanent entitlement guarantee.

OpenAI sources reviewed as dated compatibility evidence:

- <https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt>
- <https://help.openai.com/en/articles/12003714-chatgpt-team-models-limits>
- <https://help.openai.com/en/articles/20001354-gpt-56-in-chatgpt/>

**Architecture consequence:** truthful state-changing MCP remains mandatory, but
support is feature-tested on the exact surface instead of inferred from plan name.

### 2. Claude `PermissionRequest` does run in non-interactive contexts

The earlier draft incorrectly treated `PermissionRequest` as unavailable in
`claude -p`. Current Claude Code hook documentation says it can run in sessions
that cannot display a permission prompt, including non-interactive/background
contexts; when no hook decides, the request is denied.

At the same time, current `PermissionRequest` hook input does not expose the same
exact `tool_use_id` correlation as `PreToolUse`. Current `PreToolUse` supports
`permissionDecision: "defer"` in non-interactive mode and the managed result can
report `tool_deferred`, preserving the call for same-session resume.

Current documentation also states that non-interactive defer is ignored when
Claude emits **multiple tool calls together**.

Primary docs:

- <https://code.claude.com/docs/en/hooks>
- <https://code.claude.com/docs/en/hooks-guide>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/cli-usage>

**Architecture consequence:** `PermissionRequest` is a real permission-decision
signal; confirmed single-tool `PreToolUse` defer remains the preferred exact
durable pause/resume fence. Multi-tool defer cannot fabricate `needs_input`.

### 3. Deterministic browser-delivery identity is not a possession secret

The first draft used an unkeyed deterministic hash of obligation/generation/
revision and then called the result an unguessable possession fence. That was an
internal security contradiction.

V1 now separates:

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)

delivery_id = CSPRNG(>=192 bits)
```

The deterministic key is for idempotency/deduplication only. The random delivery
ID is generated once, carried only in the exact browser wake, omitted from
bootstrap/status, and required by `foreman_resume` in addition to connector auth
and obligation/generation/version fences.

### 4. The complete Claude structured stream must not be durably spooled

Current structured programmatic output may contain intermediate tool-use/tool-
result records. Persisting the whole stream to survive daemon restart would violate
the explicit no-tool-arguments/no-command/no-transcript data boundary.

V1 worker-host instead parses the stream online and may durably keep only:

- sanitized run/lifecycle receipts;
- one bounded complete final assistant-result candidate needed for independent
  review;
- sanitized child-exit receipt.

Intermediate provider records are discarded after in-memory parsing.

### 5. Same-user file modes are privacy, not hostile-worker containment

Claude/tools and Command Governor normally run as the same OS user in V1. Owner-
only filesystem modes protect against other OS principals and accidental exposure,
but do not sandbox a malicious same-user process. The general Governor state-root
path is no longer intentionally exported to Claude; hostile-worker containment is
a future separate-user/sandbox/broker problem and is not claimed by V1.

---

## Pinned source revisions

The table distinguishes current repository head from the exact source blob or
release whose architecture was inspected where they differ.

| Project/source | Verified revision/release on 2026-08-31 | Relevance |
| --- | --- | --- |
| `anthropics/claude-code` | main `f275fa282e76c5e5456912268f2c367a7f4f4797`; latest release `v2.1.252` published 2026-08-31 | current worker CLI/release context; hook semantics are taken from current official docs |
| `miuuyy/codex-chatgpt-web` | current main `06637f97a68faaa636986dad7514c7e2b3449347`; latest release `v4.0.7` published 2026-08-31 | strongest current public retained-conversation/browser ownership reference |
| `miuuyy/codex-chatgpt-web` architecture blob | `4367828fae8ad0a53e4adb0af19c1589640cb37c` at current head | the architecture file inspected remained byte-identical while main advanced later that day |
| `ChesterRa/CCCC` | main `5f0b83242d09c88b1e2267d1056fc5bf64feb626` | append-only event/delivery/read/reply semantics; claimed/accepted/failed/ambiguous inspiration |
| `joseym/salvor` | main `dd9eb49f6bf854dc1c96b1b1ad7accbc509807b0`; Apache-2.0 | Rust event/replay kernel; write-ahead tool intent; dangling-write reconciliation; crash-exact tests |
| `PrimeIntellect-ai/prime-agent` | main `9f5edc192cfe3d4737205a2f551d2b6b6e34fe09`; MIT | daemon mutation journal; uncertain-result no-replay; generation cursors; process-safe session leases |
| `ralphkrauss/agent-orchestrator` | main `8b2f3b967e90877c3abac07061dbb2b1e67d2035`; MIT | daemon-owned orchestration truth; durable notification/list/ACK; request-id idempotency; short-lived reviewer turns |
| `DivMode/tandem` | main `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee` | current fork baseline / orchestration lessons |
| `DivMode/tandem` PR #6 | open, head `af568233e1aae2d4cc343b38ca0e2a1a248e7857`; explicitly `DO NOT MERGE` | Stop-hook/lifecycle work and known stale-Herdr failure class |
| `Maxmedawar/tandem` | main `a98bcafd2c40ae5473b85fe41183e4f391933799` | upstream runtime/fleet/MCP architecture reference |
| `imoonkey/openweb` | main `a387b50c829d871839a613732e1b97bfa1946124` | browser-context authenticated operations and fail-closed rate behavior |
| `Octo-Lex/ChatGPT-Web2API` | master `497527dceabfa3f95961e23c291e618c5570f1ac` | CDP navigation/send/reconciliation/stall lessons; never-auto-retry send on ambiguous phase |
| `stufently/gpt-web-gateway` | main `efb01a32e9e4c7fbebb8acff204c8c2a448c476c`, version `2.14.1` | measured headed-real-Chrome bias and account pacing/anti-abuse lessons |
| `modelcontextprotocol/rust-sdk` | main `ad9832ec212baf526e1a69d73ee04cd8305ae331`; workspace/crate version `3.1.4` | official Rust MCP SDK; main includes same-day protocol-negotiation fixes |
| `mattsse/chromiumoxide` | main `afcc3a4313f2087249b4490d94e54bf8e3bfaccf` | selected Rust CDP driver candidate |
| `rust-headless-chrome/rust-headless-chrome` | main/release `0a5c307a85debc450378a1f19e4dac1838d7b22d`, `1.0.22` | fallback Rust CDP driver |
| `tauri-apps/wry` | dev `bb69d628a905d65042c71a95e85f6921ec9b3264` | OS-WebView alternative; rejected for V1 transport |
| `tauri-apps/cef-rs` | dev `a2e15ae659c4b3957883e34de879bd8b38360ce5` | embedded Chromium alternative; deferred due packaging/security burden |

Rust stable was re-verified as **1.98.0**, released 2026-08-20. The scaffold must
pin the exact stable toolchain at the implementation commit rather than assume this
report remains current.

---


## Durable-orchestration implementation references

A second implementation-oriented pass found three especially relevant projects:
Salvor, Prime Agent, and Agent Orchestrator. None is a drop-in Command Governor,
but together they independently validate the most important failure semantics.

- **Salvor — ADAPT.** Its pure Rust replay cursor separates recorded outcomes from
  typed permission to execute live. A tool intent is persisted before external
  execution; a dangling non-idempotent write becomes `NeedsReconciliation` rather
  than a retry. Command Governor should independently reimplement the pure-reducer,
  write-ahead-intent, explicit effect-class, divergence, and kill/failpoint test
  patterns. Reject Salvor's broader durable tool/model payload storage because
  Command Governor's privacy boundary is intentionally narrower.
- **Prime Agent — ADAPT.** Its daemon `CommandRecoveryJournal` fsyncs a `received`
  record keyed by `(clientId, commandId)` before mutation dispatch, records the
  result before reply, returns a stored result for completed retries, and reports
  a pending receipt as uncertain without replay. Its session lease additionally
  fences PID reuse with process-start identity. Command Governor should implement
  equivalent semantics transactionally in SQLite, not copy the JSONL/TypeScript
  store or transcript architecture.
- **Agent Orchestrator — ADAPT.** It moved orchestration truth out of a long-lived
  chat supervisor into the daemon, persists durable notifications before advisory
  push hints, exposes list/reconcile plus explicit ACK, uses stable mutation
  `request_id` values, and runs short-lived structured orchestrator/reviewer turns.
  Command Governor should adopt those separations while keeping its stronger
  semantic `foreman_ack` and stricter result/privacy boundaries.

The complete ADOPT/ADAPT/REJECT mapping and Phase 1 Rust blueprint are in
[`2026-08-31-durable-orchestration-pattern-review.md`](2026-08-31-durable-orchestration-pattern-review.md).
No source from these projects is vendored or copied by this review.

---

## ChatGPT Web transport research

### codex-chatgpt-web V4/current architecture

The current public architecture retains one persistent authenticated Electron
partition, owns exact task-bound surfaces, uses a connector ABI identity, maintains
turn capabilities, and explicitly supervises browser/tunnel lifecycle. Current
main moved after the initial inspection, but the architecture file remained the
same blob at final verification.

The important semantics to reimplement independently in Rust where useful are:

- exact surface ownership/leases;
- retained logical agent/surface continuity;
- explicit connector ABI identity rather than invisible schema mutation;
- no replay across reconnect/repair boundaries;
- explicit physical browser-turn lifecycle;
- separate browser profile/session ownership;
- bounded parallelism and account-abuse awareness.

Do not copy its Electron/Bun/TypeScript implementation into Command Governor.

### ChatGPT-Web2API

The current repository contains field-driven CDP hardening: staged navigation
readiness, target/tab isolation, send-selector scoping, reconciliation before
raising stalls, and the crucial rule that a phase-2 send/stall uncertainty does
**not** automatically resend the user message.

These findings support Command Governor's deterministic pre-Send/ambiguous boundary
and CDP observation plane, not a direct private-write architecture.

### gpt-web-gateway

Current deployment documentation explicitly keeps real Chrome headed by default
because that configuration was measured as the reliable one for its protected web
interaction. Its account-pacing work is additional evidence that automated web
use must fail conservatively rather than hammer/retry through account controls.

Command Governor does not adopt its JS stack or stealth/bypass techniques.

### OpenWeb

Current source shows a general pattern relevant to Command Governor: when a site
blocks plain server-side requests, perform operations in the authenticated browser
context using ambient session state rather than exporting credentials to a
separate client. The exact OpenWeb ChatGPT adapter architecture should not be
assumed stable beyond the pinned source; the broader browser-context lesson is the
useful one.

### Architectural conclusion

For sensitive writes, use the actual ChatGPT SPA. For observation/reconciliation,
use CDP Network/Target/Page and, only when stable and safe, narrow authenticated
reads. Keep all unofficial ChatGPT-specific behavior isolated in
`governor-chatgpt-web`.

Do not implement Sentinel/Turnstile/PoW/CAPTCHA/entitlement/rate-limit bypass.

---

## Browser driver comparison

### chromiumoxide — selected starting point

Strengths for this workload:

- Rust-first and Tokio-oriented;
- launch or attach to Chrome over CDP;
- generated CDP protocol types;
- Target/Page/Network domain access;
- headed/headless Chrome launch configuration;
- event streams fit a dedicated async browser supervisor.

Weakness: maintenance cadence is lower than the rapidly moving JS browser
libraries, so isolate it behind `governor-browser` and keep a driver conformance
suite.

### headless_chrome — fallback

Mature direct Chrome/CDP control and useful API coverage, but its concurrency model
is more blocking/thread-oriented than the selected daemon architecture. Keep as a
fallback if `chromiumoxide` exposes a measured blocker.

### Wry — reject for V1

Wry is a good Rust WebView abstraction but uses platform webviews. Command
Governor specifically needs one consistent system-Chrome/CDP target and Network
observation model, normal Chrome authentication/passkey behavior, and exact target
lifecycle. A cross-platform WebView abstraction works against those goals.

### CEF Rust — defer

CEF can provide deep Chromium ownership, but would make Command Governor own a
large embedded browser distribution, security-update cadence, packaging and
platform integration. That cost is unjustified until a headed system-Chrome/CDP
spike proves inadequate.

### Headed versus headless

Headed Chrome is the V1 hypothesis because multiple current public ChatGPT-web
projects report real headed Chrome as their measured reliable configuration.
Headless remains a separate Gate B experiment. Command Governor will not add
stealth/challenge bypass merely to make headless pass.

---

## MCP / ChatGPT app conclusions

Use official `modelcontextprotocol/rust-sdk` / `rmcp`. At review completion the
workspace version is `3.1.4`; repository main advanced the same day with protocol-
negotiation fixes, so pin a released crate intentionally rather than depending on
main by accident.

Keep the public ABI to:

- `foreman_bootstrap`
- `foreman_resume`
- `foreman_ack`
- `foreman_answer_input`

Bootstrap is low-information. Resume/ACK/input answer are truthful mutations.
Current ChatGPT app selection is message-scoped and must be proved per browser wake.
Current local MCP usage requires OpenAI's supported Secure MCP Tunnel/connectivity
path rather than assuming ChatGPT can dial an arbitrary localhost server.

Breaking public tool semantics require a new connector ABI/explicit refresh.

---

## Claude lifecycle conclusions

Current Claude Code release at verification is **v2.1.252**; provider behavior
must still be feature-detected and conformance-tested rather than inferred solely
from the version number.

Key current properties:

- matching hooks can run in parallel;
- Stop can be blocked, so our Stop callback is only `stop_candidate`;
- successful managed completion should use final structured result + child exit;
- `PermissionRequest` can run in non-interactive/background contexts;
- `PreToolUse` has exact tool-use correlation and can `defer` in non-interactive
  mode;
- confirmed defer returns a `tool_deferred` style structured result and can be
  resumed in the same Claude session;
- defer is not a clean pause when several tool calls are emitted together;
- progress should use structured/native/tool events, not terminal repaint;
- stale Herdr `working` cannot outrank a stronger confirmed structured state.

The current Tandem PR #6 remains valuable evidence of the stale-runtime class but
its "Stop is terminal" approach is not copied blindly; current Claude hook
semantics require a stronger completion proof.

---

## SQLite: rusqlite versus sqlx

V1 chooses `rusqlite` unless the scaffold review uncovers a new requirement.

Why:

- Command Governor deliberately has one authoritative daemon writer;
- SQLite serializes writers even in WAL mode;
- critical lifecycle transactions benefit from explicit synchronous transaction
  boundaries and short no-I/O sections;
- no ORM is needed;
- a dedicated DB actor can keep SQLite work off async executor hot paths;
- SQLx pooling does not create multi-writer SQLite semantics and adds an async
  abstraction around the most correctness-sensitive layer.

Use WAL, foreign keys, bounded busy timeout, `synchronous=FULL` initially,
deterministic migrations, replay validation, and a separate daemon/state-root lock.

---

## Rust foundation

Candidate initial foundation:

- stable Rust 1.98.0 at this review snapshot, edition 2024;
- Tokio;
- serde;
- thiserror;
- tracing;
- clap;
- `rmcp`;
- `rusqlite` bundled SQLite;
- uuid;
- a deliberate time crate after implementation review;
- axum/tower only if a loopback transport endpoint is actually required.

No Node, Bun, Electron, React, TypeScript or JavaScript service is introduced.
Small page-context JavaScript through CDP is acceptable only when the current DOM
requires it.

`anyhow` may be used at binary/application edges, not as the domain error model.

Quality gates from the first Rust commit:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
cargo deny check
```

The August 2026 Rust ecosystem supply-chain incident is a concrete reason to pin
and audit dependencies rather than blindly auto-merge updates.

---

## Licensing and provenance

Command Governor remains MIT licensed.

Architectural inspiration/reference:

- Tandem — MIT;
- codex-chatgpt-web — MIT;
- CCCC — Apache-2.0;
- `rmcp` — Apache-2.0;
- Rust browser libraries under their respective licenses.

The current repository is an independent implementation. No third-party
implementation source is presently vendored/copied. If future code is copied or
substantially adapted, preserve the applicable license/NOTICE and record exact
provenance in `THIRD_PARTY_NOTICES.md`.

Do not imply any upstream author endorses Command Governor.

---

## OpenAI public-project risk

Sources reviewed:

- <https://openai.com/policies/terms-of-use/> — Terms of Use, effective
  2026-01-01;
- <https://openai.com/policies/app-developer-terms/> — App Developer Terms,
  updated 2026-07-09;
- current ChatGPT developer-mode/MCP Help Center documentation above.

The project should be described as unofficial ChatGPT Web automation. The design
uses normal user authentication, local browser state, supported MCP connectivity,
and no auth/entitlement/CAPTCHA/rate/protective-measure bypass. That reduces
engineering/security risk but is not a legal conclusion that every use is allowed.

An official future foreman/push API should replace `governor-chatgpt-web` without
changing the durable kernel.

---

## Remaining empirical gates

### Gate A — MCP mutation

The exact bound ChatGPT account/app/surface must prove state-changing MCP
actions with a harmless synthetic mutation/read-back, stale-generation
rejection, and usable confirmation behavior. The target Pro surface
demonstrated the required mutation class through Tandem on 2026-08-31,
but Command Governor still requires its own current `capability_epoch`.
Plan name alone neither accepts nor excludes a surface.

### Gate B — headed Chrome/CDP

Dedicated authenticated headed Chrome must prove exact binding, per-message app
selection, 10/10 unique wakes, semantic accepted evidence, crash-at-Send
ambiguity/no replay, restart, and generation fencing. Headless is measured
separately.

### Gate C — Claude managed execution

Pinned Claude Code must prove final-result/exit semantics, no raw stream
persistence, Stop-veto correctness, actual settings-source behavior, single-tool
defer/resume, multi-tool defer failure behavior, non-interactive
`PermissionRequest`, daemon-offline final-result recovery, stale-Herdr
reconciliation, and forbidden-data scans.

## Recommendation after independent review

**Proceed with the small pure Rust kernel/store/testkit scaffold after the
architecture PR is accepted.** The event/obligation/storage/security model is now
internally consistent enough to implement deterministically.

Do **not** claim or implement a supported live ChatGPT or Claude adapter beyond
spike/conformance work until its corresponding gate passes. In particular, do not
ship consumer ChatGPT Pro as an end-to-end V1 foreman while its custom MCP surface
is read/fetch-only.
