# Independent V1 architecture review — 2026-08-31

Reviewer of record: ChatGPT Web foreman  
Scope: `architecture/verified-v1-control-plane` against `main` baseline
`fd3e5a61425f00ee3b164d2a840708602f972342`  
Status: **architecture defects corrected; pure Rust Phase 1 may proceed after this
PR is accepted; live adapters remain gated**

## Review method

This was not a prose-only proofreading pass. The review cross-checked:

- the central durable-obligation/explicit-ACK invariant;
- browser at-most-once ambiguity semantics;
- exact ChatGPT binding and MCP mutation fencing;
- current OpenAI ChatGPT developer-mode/MCP plan behavior;
- current Claude Code hook/non-interactive semantics;
- worker-result durability and privacy boundaries;
- SQLite/store crash ordering;
- same-user local trust boundaries;
- deterministic acceptance tests;
- current upstream revisions/releases and licensing/provenance.

No Tandem/Claude orchestration loop was used to perform this review. No Rust
implementation was introduced.

## Findings

### R1 — BLOCKER — consumer ChatGPT Pro cannot perform the proposed MCP ACK loop

**Original defect:** architecture language treated write-capable MCP as a generic
preflight without stating that the intended consumer ChatGPT Pro surface is
currently documented as read/fetch-only.

**Evidence:** current OpenAI developer-mode documentation says full MCP support,
including modify/write actions, is rolling out to Business, Enterprise, and Edu;
consumer Pro custom MCP is read/fetch-only. Current Business model documentation
shows the Pro model option is powered by GPT-5.6 Sol Pro.

**Risk:** shipping consumer Pro would force one of three invalid behaviors: browser
settlement as fake ACK, a mutation mislabeled read-only, or an obligation that can
never correctly close.

**Fix:** consumer Pro is now explicitly unsupported for end-to-end V1 at this
snapshot. Business/Enterprise/Edu are candidates subject to Gate A mutation and
confirmation behavior. Business can still use the Pro model. The explicit ACK
invariant is unchanged.

**Disposition:** fixed in architecture, ADR 0004, MCP contract, browser transport,
threat model, testing, roadmap, README, SECURITY.

### R2 — BLOCKER — Claude `PermissionRequest` semantics were stale

**Original defect:** the worker contract said `PermissionRequest` did not fire in
non-interactive `claude -p`.

**Evidence:** current Claude Code documentation says PermissionRequest hooks can run
where a prompt UI is unavailable, including non-interactive/background contexts;
if no hook decides, the request is denied. Current `PermissionRequest` input lacks
the same exact `tool_use_id` available to `PreToolUse`. Current PreToolUse supports
non-interactive `defer`, while multi-tool defer is ignored.

**Risk:** missing real permission decisions, inventing a wrong fallback mechanism,
or projecting a resumable input state without an exact tool-call fence.

**Fix:** PermissionRequest is a real permission-decision signal. Exact durable
out-of-band pause/resume prefers a confirmed **single-tool** PreToolUse defer plus
structured `tool_deferred` proof. Multi-tool defer becomes unsupported/manual
reconciliation rather than fake `needs_input`.

**Disposition:** fixed in architecture, ADR 0005, worker lifecycle, state machines,
data model, threat model, testing, roadmap, research.

### R3 — BLOCKER — deterministic delivery ID was falsely used as an unguessable possession fence

**Original defect:** one unkeyed deterministic hash of obligation/generation/
revision was both the delivery identity and the possession value required by
`foreman_resume`.

**Risk:** bootstrap or other observable scheduling metadata could allow an
unrelated connector conversation to reconstruct the supposed possession value.
The design claimed security it did not have.

**Fix:** separate identities:

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)

delivery_id = CSPRNG(>=192 bits)
```

`delivery_key` is deterministic, non-secret, and dedupe-only. `delivery_id` is
random, generated once, carried in the exact bound browser wake, omitted from
bootstrap/status, and required by `foreman_resume` along with connector auth and
all normal durable fences.

**Disposition:** fixed in architecture, ADR 0003, ADR 0004, data model, state
machines, browser transport, MCP contract, threat model, testing, README, roadmap.

### R4 — BLOCKER — raw Claude structured-stream spool violated the privacy contract

**Original defect:** daemon-outage durability relied on a private spool of the
complete `claude -p --output-format stream-json` stream.

**Evidence/risk:** current structured streams can contain tool-use/tool-result and
other intermediate provider records. A complete spool can therefore persist raw
tool arguments/results, commands, prompt-derived data, or transcript-like content,
contradicting the explicit storage prohibition.

**Fix:** the worker-host parses structured output online and never persists a
complete provider stream. Durable worker-host data is limited to:

- allowlisted sanitized run/lifecycle receipts;
- one bounded complete final assistant-result candidate required for review;
- sanitized child-exit receipt.

Intermediate records are discarded after in-memory processing. The final result is
then promoted through the immutable result-artifact durability sequence before
`completed_unprocessed` is visible.

**Disposition:** fixed in architecture, ADR 0005, worker lifecycle, data model,
threat model, testing, roadmap, README, SECURITY.

### R5 — HIGH — same-user file permissions were overclaimed as a worker security boundary

**Original defect:** some design language implied owner-private state/staging was
sufficient protection from worker processes even though Claude/tools normally run
as the same OS user.

**Risk:** `0600` protects from other OS principals, not a deliberately malicious
same-user process. Treating it as a sandbox would be a false security guarantee.

**Fix:** V1 explicitly defines the local OS account as the administrative trust
root. The general Governor state-root path is not intentionally exported to
Claude; managed workers get narrow opaque IDs/per-turn inbox locators only. All
imported staging is validated as untrusted. Strong hostile-worker containment is
a future separate-user/sandbox/broker feature and is out of scope for V1.

**Disposition:** fixed in architecture, worker lifecycle, threat model, testing,
README, SECURITY, roadmap.

### R6 — HIGH — bootstrap exposed more metadata than necessary without a trusted conversation principal

**Original defect:** bootstrap could expose obligation/project/worker metadata even
though MCP does not currently provide a documented trustworthy ChatGPT conversation
identity to the server.

**Risk:** another conversation in the same authenticated workspace could learn
work details and combine them with weak delivery identity assumptions.

**Fix:** bootstrap is now low-information: compatibility, health, active binding
generation, aggregate attention kinds/counts/priority/age/wake state only. It does
not disclose repository/project refs, task/session/worker refs, result content,
raw obligation detail, or the accepted random `delivery_id`.

**Disposition:** fixed in MCP contract, ADR 0004, architecture, threat model,
testing, README.

### R7 — MEDIUM — same-day upstream freshness had drifted during the architecture work

**Original defect:** research could be read as if the initially inspected source
SHA was still the latest head at review completion.

**Fix:** the research/provenance documents now distinguish current repository head,
release, and the exact source blob actually inspected. Notable final verification:

- Claude Code main `f275fa282e76c5e5456912268f2c367a7f4f4797`, release
  `v2.1.252`;
- codex-chatgpt-web main `06637f97a68faaa636986dad7514c7e2b3449347`,
  release `v4.0.7`; architecture blob
  `4367828fae8ad0a53e4adb0af19c1589640cb37c` remained unchanged;
- CCCC main `5f0b83242d09c88b1e2267d1056fc5bf64feb626`;
- DivMode Tandem main `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee` and PR #6
  head `af568233e1aae2d4cc343b38ca0e2a1a248e7857`;
- upstream Tandem main `a98bcafd2c40ae5473b85fe41183e4f391933799`;
- Rust MCP SDK main `ad9832ec212baf526e1a69d73ee04cd8305ae331`,
  workspace version `3.1.4`;
- chromiumoxide main `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`;
- headless_chrome `1.0.22` / `0a5c307a85debc450378a1f19e4dac1838d7b22d`;
- Wry dev `bb69d628a905d65042c71a95e85f6921ec9b3264`;
- CEF Rust dev `a2e15ae659c4b3957883e34de879bd8b38360ce5`.

**Disposition:** fixed in research and THIRD_PARTY_NOTICES.

## Cross-document consistency after fixes

The reviewed documents now agree on these invariants:

1. Worker completion does not close work.
2. The final reviewable result is durable before `completed_unprocessed` is
   published.
3. The complete provider stream is not durably spooled.
4. Claude Stop callback alone is not successful completion.
5. Clean Claude non-interactive pause requires confirmed exact single-tool defer;
   multi-tool defer does not fake `needs_input`.
6. Current non-interactive PermissionRequest is modeled as permission-decision
   evidence, not a falsely precise pause identity.
7. Herdr runtime state cannot override stronger fenced structured worker facts.
8. Browser `claimed` is durable before any browser I/O.
9. Send ambiguity fence is durable before exact Send activation.
10. Accepted/ambiguous delivery is never automatically replayed.
11. Deterministic `delivery_key` never acts as a possession secret.
12. Random accepted wake `delivery_id` is not disclosed by bootstrap/status.
13. Browser accepted != ChatGPT physical settlement != explicit foreman ACK.
14. Old binding generation/claim/source/obligation version cannot close current
    work.
15. Consumer Pro's current MCP limitation does not weaken ACK semantics.
16. Same-user owner-only files are not described as hostile-worker containment.
17. No GUI or human completion notification is required for correctness.

## Acceptance-test review

The deterministic suite now explicitly covers:

- obligation restart/ACK/claim/generation/source fencing;
- artifact file-before-DB crash ordering and retention;
- daemon-offline final-result recovery without raw stream persistence;
- browser deterministic key/random ID identity and non-derivability;
- claimed/activation crash ambiguity and no replay;
- low-information bootstrap and unrelated-conversation claim rejection;
- ChatGPT physical settlement without ACK;
- Claude Stop-veto race;
- stale Herdr working conflict;
- single-tool defer and multi-tool defer failure;
- non-interactive PermissionRequest handling;
- worker-answer delivery ambiguity;
- same-user trust-model assertions;
- sentinel byte scans across DB/WAL/inbox/staging/logs/diagnostics.

Live services are intentionally separated into Gates A/B/C so deterministic core
correctness does not require credentials.

## Remaining blockers are empirical, not unresolved architecture contradictions

### Gate A — write-capable ChatGPT MCP surface

A candidate Business/Enterprise/Edu workspace must prove state-changing MCP actions
and confirmation behavior on the actual account. Consumer Pro is currently
excluded by published capability.

### Gate B — authenticated headed Chrome/CDP

Must prove exact binding, message-scoped app selection, ten unique wakes, strong
accepted evidence, crash-at-Send ambiguity/no replay, restart, random correlation,
and generation fencing. Headless is separate and cannot use stealth/protection
bypass to pass.

### Gate C — Claude managed execution

Pinned real Claude must prove final-result/exit semantics, no raw stream spool,
Stop-veto correctness, actual settings-source behavior, single-tool defer/resume,
multi-tool defer failure, current PermissionRequest behavior, daemon-offline
result recovery, stale-Herdr reconciliation, and forbidden-data scans.

## Verdict

### GO — after this documentation PR is accepted

Proceed with **Phase 1 only**:

- clean Cargo workspace;
- pure `governor-core` state machines/types;
- `governor-store-sqlite` single-writer store/migrations;
- immutable result-artifact store;
- deterministic `governor-testkit` fakes/failpoints;
- local daemon/CLI skeleton only to the degree needed by the core/store tests;
- Rust quality/security/license CI from the first implementation commit.

This phase can prove the durable kernel without pretending any external product
integration already works.

### NO-GO — supported live adapters until their gates pass

Do not call these production-supported yet:

- ChatGPT foreman MCP adapter;
- ChatGPT browser transport;
- Claude managed worker adapter.

A spike/conformance implementation may be written specifically to execute Gates
A/B/C, but failure must change the support matrix—not the central invariant.

## Final reviewer decision

**Architecture approved for a small Rust kernel/store/testkit implementation once
the architecture PR is accepted.**

The review does not approve an end-to-end consumer ChatGPT Pro V1, does not approve
headless ChatGPT automation, and does not approve Claude lifecycle assumptions
without Gate C. Those are explicit empirical gates, not hidden follow-up debt.
