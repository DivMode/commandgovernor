# Command Governor composition and de-duplication audit — 2026-09-02

Status: **post-merge architecture correction and salvage research**

This document is the source of truth for one question:

> After choosing Prime Agent as the runtime substrate, which Command Governor
> capabilities should come from Prime/Pi as-is, which should be composed from
> existing packages, which genuinely require a small Command Governor extension,
> which are temporary compatibility workarounds, and which custom code should be
> removed?

This is deliberately not another general framework proposal. It consolidates the
research already done, refreshes the fast-moving external evidence, and applies it
to the code that is actually in the repository now.

---

## Executive conclusion

Command Governor should be a **curated Prime Agent package/distribution**, not a
second orchestration runtime wrapped around Prime.

Target shape:

```text
Prime Agent
  + selected existing Pi/Prime packages
  + @commandgovernor/harness
      - small product-specific policy extension(s)
      - roles / skills / prompts
      - compatibility shim only for a proven Prime defect that still matters
        on the chosen integration path
      - focused conformance tests
```

Every capability gets one of four implementation outcomes:

```text
USE EXISTING
PLUGIN (small, product-specific)
TEMP WORKAROUND (proven upstream defect only)
DELETE / DO NOT BUILD
```

There is no fifth outcome called “build a parallel Governor subsystem because we
can make it safer ourselves.”

---

# 1. Repository state this audit starts from

PR #18 (`Prime-native foundation: Gate S1b adaptation layer`) is already merged.

- reviewed #18 head: `5801a029d3b2be784f641246d9f181f4c61ac953`
- #18 merge commit on `main`: `d9e5ab04b2037b2d2c5ac7c104780a4f6fd4a6a2`
- #18 size: 84 changed files, 18,396 additions, 15 deletions
- ADR-0009 is still `Proposed` after that merge.

The merged code is real repository state. The salvage job is therefore to retain,
shrink, move into a package/extension, or remove it in follow-up work. Passing a
correctness review does not automatically make every component permanent product
architecture.

There is also an open **draft PR #19**, stacked on the old #18 branch:

- title: `Adopt DeepSeek Harness architecture patterns without weakening Prime durability`
- head: `7861fb39dc413cdca164c1f429c16d8ef0fd865e`
- 14 changed files, 2,823 additions
- adds DeepSeek donor research + ADR-0010 + new `governor/composition/*` contracts

PR #19 must **not** be blindly retargeted and merged now. Its research is useful;
its new custom Governor contracts must pass this same de-duplication test first.

---

# 2. Research lineage consolidated here

The audit incorporates the existing Command Governor research instead of treating
this as a clean-slate opinion:

## Original durable-control-plane research

- `docs/research/2026-08-31-technology-review.md`
- `docs/research/2026-08-31-durable-orchestration-pattern-review.md`
- ADRs 0001–0006

These established the reliability requirements but predate the Pi-native pivot.
Their bespoke-daemon implementation direction is no longer controlling.

## Pi-config / session / memory / Stanford research

- `docs/research/2026-08-31-session-memory-and-analytics-review.md`
- ADR-0007
- `amosblomqvist/pi-config`
- `pi-interactive-subagents`
- `pi-observational-memory`
- Stanford `Agent Memory: Characterization and System Implications of Stateful Long-Horizon Workloads`
- Stanford MemoryArena
- compaction research cited by ADR-0007

The requirements retained from this work are valuable: durable lineage, explicit
loadouts, memory that is advisory rather than exact authority, and downstream-action
memory evaluation. The old decision to independently reimplement those mechanisms
in Rust was superseded by ADR-0008.

## Pi-native pivot research

- `docs/research/2026-09-01-pi-native-command-governor-harness-review.md`
- ADR-0008

This got the strategic rule right: **use Pi core, compose packages, extend only for
real gaps, do not own another general runtime.**

## Harness landscape / substrate bake-off

- `docs/research/2026-09-01-agent-harness-landscape-and-substrate-bakeoff.md`
- ADR-0009

This selected Prime Agent because its durable long-running runtime was a better fit
than plain upstream Pi for the hardest session/worker/recovery requirements.

## Rust invariant catalog / S1b adaptation work

- `docs/research/2026-09-01-rust-invariant-catalog.md`
- `docs/prime-native/adaptation-layer.md`
- `docs/upstream/2026-09-01-prime-worker-loss-journal.md`
- merged PR #18

This found real Prime edge cases and built a large external adaptation layer. The
failure evidence remains valuable; whether all of the implementation should remain
is the question this audit now answers.

## DeepSeek Harness donor research

Draft PR #19 adds:

- `docs/research/2026-09-01-deepseek-harness-architecture-donor-review.md`
- ADR-0010 `deepseek-pattern-adoption`

The research reviewed DeepSeek Harness at:

```text
4e84901e6471b79ec0338099867ebb4606d12bb5
0.1.2-alpha.4
```

DeepSeek Harness had already moved again by this audit. Current `master` checked
2026-09-02:

```text
49a606bc5b5934603f22a26957a07dc799ab0291
0.1.2-alpha.5 line
```

The donor research remains useful for capability seams, typed events, workflow
composition, provenance, provider-neutral children, explicit approvals and
component metadata. But PR #19 made the same risky move as #18: it translated donor
patterns into another layer of custom `governor/composition/*` code before proving
Prime/current packages did not already supply the needed product behavior.

**Disposition for PR #19:** preserve the research; freeze the implementation slice
until this audit's package bake-offs decide what, if anything, still needs custom
code. In particular, do not merge another Governor event spine, child runtime,
mailbox, workflow IR, or sandbox contract simply because DeepSeek has a good version
of the idea.

---

# 3. External snapshots refreshed for this audit

## Prime Agent selected pin

```text
Prime Agent v0.8.1
514633727bf26d74f39f3119c2b0e31a5ceb2a9d
```

## Prime current upstream at research time

```text
0ba0423c5c18805c72ad03d03aaf1d9e0cc622d0
```

Repository: <https://github.com/PrimeIntellect-ai/prime-agent>

## Upstream Pi current at research time

```text
e266507b606b9552fa277252644054afd4384b11
```

Repository: <https://github.com/earendil-works/pi>

## DeepSeek Harness current at research time

```text
49a606bc5b5934603f22a26957a07dc799ab0291
```

Repository: <https://github.com/deepseek-ai/deepseek-harness>

These current heads are research inputs, not silent dependency updates. Any re-pin
still requires deliberate compatibility testing.

---

# 4. Prime already owns the general runtime

At the selected pin Prime already provides the runtime mechanics Command Governor
was previously preparing to own externally:

- daemon supervisor and resident workers;
- persistent session JSONL;
- session leases;
- worker recovery and reconnect;
- command journals;
- scheduled prompts and heartbeats;
- persistent goals;
- autonomous continuation / quality gates;
- RLM children and persistent Python state;
- direct agent-to-agent messaging;
- continual-harness refinement;
- ACP mode;
- extensions and packages.

Prime's long-running architecture explicitly makes the resident worker—not the TUI
client—the owner of queue, schedules, session, kernel, and children.

Primary Prime docs:

- `packages/coding-agent/docs/long-running-agents.md`
- `packages/coding-agent/docs/daemon.md`
- `packages/coding-agent/docs/extensions.md`
- `packages/coding-agent/docs/packages.md`

**Disposition:** Command Governor does not build another generic runtime, session
manager, scheduler, goal engine, subagent runtime, or process supervisor.

---

# 5. The Governor-specific layer can be a Prime package

At Prime 0.8.1 an extension can:

- subscribe to lifecycle events;
- intercept/block tool calls;
- inject or modify context;
- register tools and commands;
- persist custom session entries with `pi.appendEntry()`;
- initialize asynchronously;
- use npm dependencies and Node built-ins;
- perform external integrations.

Prime packages bundle extensions, skills, prompts, and themes and can be installed
from npm, Git, or a local path. They intentionally retain compatibility with the
inherited Pi package manifest.

Therefore the default product artifact should look like:

```text
@commandgovernor/harness
  extensions/
    policy.ts
    [only other genuinely necessary extensions]
  skills/
  prompts/
  package.json
```

Primary sources:

- <https://github.com/PrimeIntellect-ai/prime-agent/blob/514633727bf26d74f39f3119c2b0e31a5ceb2a9d/packages/coding-agent/docs/extensions.md>
- <https://github.com/PrimeIntellect-ai/prime-agent/blob/514633727bf26d74f39f3119c2b0e31a5ceb2a9d/packages/coding-agent/docs/packages.md>

**Disposition:** default to Prime package/extension implementation. A separate
Governor service/daemon needs a proven requirement, not architectural inertia.

---

# 6. Refreshed package ecosystem — major Governor overlap

The ecosystem moved quickly enough that the previous day's survey already missed
important options. These are **candidates to bake**, not instructions to install all
of them together.

## `pi-tasks` 0.2.4 — work/evidence contract

Provides:

- evidence-gated completion;
- acceptance criteria;
- decisions and blockers;
- ordered plans;
- scope-drift detection;
- branch-aware persistent events;
- compaction-safe resume;
- refusal to complete while evidence/criteria/blockers are incomplete.

Source: <https://pi.dev/packages/pi-tasks>

**Overlap:** old Governor durable obligations, evidence, completion gates, decisions.

## `pi-squad` — independent acceptance semantics

Especially relevant because squad agents cannot mark their candidate accepted.
After candidate tasks finish:

- persisted state becomes `review`, not `done`;
- main Pi must independently re-read the contract and inspect actual diff/source;
- it must rerun verification and submit `squad_review`;
- failed review stays review-blocked and rework happens in the same authoritative
  squad;
- pending/failed review gates survive restart.

Source: <https://pi.dev/packages/pi-squad>

This is the closest package found so far to the core Governor rule:

```text
worker finished != work accepted
```

## `pi-subagents` 0.57.0 — generic delegation/review

Ships worker, reviewer, oracle, scout, and researcher roles; foreground/background
runs; parallel reviewers; review loops; and implement-then-review workflows.

Source: <https://pi.dev/packages/pi-subagents>

**Use:** strong generic delegation candidate. Its ordinary review policy is still
prompt/config driven, so do not confuse it alone with a hard acceptance authority.

## `pi-pr-review` 1.17.5 — GitHub PR review

Provides multi-lane parallel review, host-owned structured findings, exact
reviewed-head/staleness checks, optional verification, and gated COMMENT/APPROVE
publishing.

Source: <https://pi.dev/packages/pi-pr-review>

**Overlap:** much of the planned Governor GitHub reviewer plumbing.

## `pi-governance-pipeline` 1.0.14 — multi-model governance

Separates implementation from independent reviewers, routes roles across models
/providers, severity-gates findings, enforces a minimum reviewer panel, and keeps
hard run budgets outside model context.

Source: <https://pi.dev/packages/pi-governance-pipeline>

**Disposition:** candidate/donor; likely more opinionated than our minimal product.
Do not adopt merely because the name sounds aligned.

## Other current donors found

- `@agwab/pi-workflow` — deep review / spec review / impact review;
- `gentle-pi` — adversarial review/finalization and receipts;
- `@misunders2d/pi-goal` — goal execution with independent completion audit;
- `pi-team`, `pi-agentteam`, `pi-agents-team`, `pi-teams` — alternative team models;
- `pi-background-tasks` — evidence-oriented background work and validation.

These reinforce the composition direction. They are **not** a recommendation to
load overlapping task/review authorities simultaneously.

---

# 7. ChatGPT transport — use an existing Pi-native implementation first

## `pi-oracle` — first exact-thread candidate

`pi-oracle` already supports:

- a user/browser-created exact `https://chatgpt.com/c/<id>` conversation;
- normalized conversation IDs;
- same-conversation leases;
- detached background workers;
- isolated per-job browser profiles;
- durable job state, response text, and artifacts;
- persisted same-thread follow-ups;
- best-effort Pi wake-up while preserving the result when wake-up is missed.

Source:
<https://github.com/fitchmultz/pi-oracle/blob/main/docs/ORACLE_DESIGN.md>

This directly invalidates old assumptions that a Pi extension cannot coordinate a
detached durable browser worker.

## `pi-gpt` 0.4.2 — direct/private candidate

Exposes ChatGPT/Codex account, model, chat, conversation, and message operations
inside Pi and can continue conversations.

Source: <https://pi.dev/packages/pi-gpt>

Because it relies on undocumented ChatGPT backend interfaces, keep it
capability-gated and replaceable.

## Consequence for `harness/extensions/cg-foreman/transport.ts`

The merged transport file is a types-only stub from an older candidate analysis.
Its comments must not become permanent architecture authority. It assumes a custom
durable sidecar/background arrangement before the current ecosystem was fully
accounted for.

**Disposition:** preserve the semantic requirements (exact thread, stale revision
rejection, ambiguous send is not blind retry) but test `pi-oracle`/`pi-gpt` first.
Do not build another standalone Chrome/CDP transport.

---

# 8. Memory and continual refinement — compose, do not rebuild

Prime itself now has continual-harness refinement over prompt notes, memories,
skills, and subagent specs.

Current observational-memory candidate:

```text
pi-observational-memory 3.0.4
```

It provides observation-centered memory, durable reflections, background memory
work, source-backed recall, and compaction integration.

Sources:

- Prime `skills/refine/SKILL.md` and refinement implementation;
- <https://pi.dev/packages/pi-observational-memory>
- <https://pi.dev/packages/pi-continual-harness> as a smaller upstream-Pi donor.

**Disposition:** no Governor memory engine. Choose/configure at most one owner for a
memory concern and evaluate downstream action quality using the requirements from
ADR-0007 / Stanford research.

---

# 9. Lifecycle seam — upstream Pi has `agent_settled`

Current upstream Pi exposes `agent_settled`: fully settled means no automatic
retry, compaction retry, or queued continuation remains.

Current Prime source search on 2026-09-02 found no `agent_settled` by that name;
Prime 0.8.1 extension docs expose `agent_end` once per prompt.

Pi source:
<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md>

**Disposition:** generic missing lifecycle primitives should be upstreamed/adapted
through Prime, not replaced by another Governor provider-specific detector.

---

# 10. Prime worker-loss journal defect — real defect, conditional workaround

The S1b work found a real Prime defect: a worker socket can close after an effect,
and the supervisor catch path can persist the ordinary failure response in the
command journal.

Current Prime `main` still contains:

- worker socket close -> `Daemon worker socket closed`;
- supervisor catch path that records ordinary caught failures into the command
  journal.

Relevant current source:

- `packages/coding-agent/src/modes/daemon/daemon-worker-client.ts`
- `packages/coding-agent/src/modes/daemon/daemon-supervisor.ts`

Prime also correctly refuses to replay a still-pending command under the same
identity using `command_result_uncertain`.

The crucial product question is **who consumes a finalized worker-close failure and
whether they automatically create a replacement mutation**.

PR #18's large D2 ledger/classifier exists because Command Governor chose to be an
external raw daemon client that interprets outcomes and may issue replacements.
If the final product is an in-process Prime package and does not implement that
external mutation/re-dispatch authority, the duplicate-effect problem may no longer
need a separate Governor mutation ledger.

Do not guess. Prove it with a small black-box package-path spike:

1. run the same real worker-loss reproducer through the intended package product;
2. observe whether stock Prime/selected package automatically issues a new mutation;
3. if there is no automatic duplicate, remove the external D2 control-plane path;
4. if a duplicate remains possible, retain only the smallest compatibility shim;
5. keep the real reproducer and define the shim's upstream-removal condition.

---

# 11. DeepSeek Harness — research donor, not permission to build another Governor runtime

The DeepSeek donor review found valuable patterns:

- capability/service seams;
- append-only typed event projections;
- explicit Session vs Activation identity;
- provider-neutral child capability negotiation;
- provenance/source relationships;
- workflows and PTC/tool composition;
- explicit approvals/credentials/policy seams;
- component model/token/cache metadata;
- ACP specialist-worker potential.

Those are legitimate research findings.

But draft PR #19 immediately turned many of those patterns into new custom
Governor code:

```text
governor/composition/events.ts
governor/composition/capabilities.ts
governor/composition/lifecycle.ts
governor/composition/child.ts
governor/composition/mailbox.ts
governor/composition/workflow.ts
governor/composition/sandbox.ts
governor/composition/component.ts
```

That is exactly the move this audit is designed to challenge.

Examples:

- Do not build a new child runtime before testing Prime RLM / `pi-subagents` /
  `pi-squad`.
- Do not build another workflow runtime before testing Prime autonomous/goals and
  current workflow packages.
- Do not build a new generic event spine just because DSH has one; Prime/Pi session
  state plus selected package state may already be enough.
- Do not make DSH sandbox patterns a mandatory product feature; trusted-local
  Command Governor does not require a sandbox.
- Do not build generic capability registries merely to re-express Prime package
  composition unless a concrete collision cannot be solved by the selected stack.

**PR #19 disposition now:**

```text
DeepSeek research document     KEEP / REFRESH AS NEEDED
ADR-0010                       DO NOT ACCEPT YET
DSH specialist ACP worker      OPTIONAL FUTURE BAKE-OFF
new governor/composition code  FREEZE; DE-DUP FIRST
sandbox contract               NOT CORE PRODUCT REQUIREMENT
```

DeepSeek Harness is changing rapidly; the reviewed alpha.4 source is already one
release behind alpha.5. Treat it as a donor/candidate, not another thing Command
Governor must mirror.

---

# 12. Capability disposition matrix

| Concern | Preferred owner/candidate | Disposition |
| --- | --- | --- |
| agent/model runtime | Prime | **USE EXISTING** |
| daemon supervisor / resident workers | Prime | **USE EXISTING** |
| session JSONL / resume / tree | Prime | **USE EXISTING** |
| worker/session leases | Prime | **USE EXISTING** |
| schedules / heartbeats | Prime | **USE EXISTING** |
| goals / autonomous continuation | Prime | **USE EXISTING** |
| RLM / recursive children | Prime | **USE EXISTING** |
| agent-to-agent messaging | Prime | **USE EXISTING** |
| continual-harness refinement | Prime | **USE EXISTING** |
| generic delegation | Prime RLM / `pi-subagents` | **BAKE FIRST; DO NOT BUILD** |
| durable work/evidence | `pi-tasks` | **BAKE FIRST** |
| independent candidate acceptance | `pi-squad` | **BAKE FIRST** |
| GitHub PR review | `pi-pr-review` | **BAKE FIRST** |
| multi-model governance | `pi-governance-pipeline` | **DONOR/CANDIDATE** |
| exact existing ChatGPT thread | `pi-oracle` | **FIRST CANDIDATE** |
| direct ChatGPT transport | `pi-gpt` | **SECOND/EXPERIMENTAL CANDIDATE** |
| observational memory | `pi-observational-memory` | **OPTIONAL BAKE** |
| generic settled lifecycle | upstream Pi `agent_settled` | **UPSTREAM/ADAPT** |
| user-owned decision gate | selected package or small policy extension | **PROVE GAP FIRST** |
| stale task/revision/foreman correlation | transport/package or small policy extension | **PROVE GAP FIRST** |
| raw Prime mutation classifier/ledger | only if raw external daemon path remains | **CONDITIONAL TEMP WORKAROUND** |
| DeepSeek event/child/workflow contracts | Prime/packages unless proven gap | **FREEZE PR #19 IMPLEMENTATION** |
| sandbox | none by default | **OPTIONAL PROFILE ONLY** |
| ACP | Prime stable ACP / optional other ACP workers | **INTEROP, NOT INTERNAL AUTHORITY** |

---

# 13. PR #18 salvage by file family

PR #18 was correctly reviewed for the architecture it implemented. The salvage
question is whether that architecture remains necessary after composition.

## Preserve as evidence until replacement behavior is proven

High-value real reproducers/specifications include:

- `conformance/runtime/d2-worker-loss-uncertain.test.ts`
- `conformance/runtime/d2-import-jsonl-post-effect.test.ts`
- `conformance/runtime/d1-resident-root-recovery.test.ts`
- `conformance/runtime/d8-explicit-session-path.test.ts`
- pin/protocol drift tests;
- crash/restart negative controls that encode a real product requirement.

As the implementation shrinks, convert surviving requirements to **black-box
package-level conformance**. Delete tests whose only purpose is proving machinery we
intentionally remove after equivalent product-level evidence exists.

## `governor/prime/*`

External raw daemon client, protocol types, launcher, client identity, environment.

**Preferred:** remove from normal product path if the package can operate through
Prime's normal extension/session APIs. Retain only pin/protocol conformance or a
truly necessary narrow adapter.

## `governor/session/*`

Custom registry/path/incarnation/recovery authority.

**Preferred:** re-run D1/D8 through the package path. If Prime high-level lifecycle
already satisfies the requirement, remove registry/reopen authority. If one
preflight policy remains useful, make it a small extension policy.

## `governor/mutation/*`

D2 classifier/digest/ledger/proof matrix.

**Preferred:** do not assume permanent ownership. First prove whether the package
product ever needs to interpret raw daemon failure and create replacements. If not,
remove the ledger/classifier from the normal path. If yes, retain the minimum shim
needed by the reproducer.

## `governor/fs/*` and `governor/process/*`

Exist mainly to support custom authoritative stores and fences. Remove if those
authorities disappear, unless the surviving minimal compatibility shim needs them.

## `harness/agents/*`

Compatible with a package-shaped product, but generic roles may be duplicated by
`pi-subagents`, `pi-squad`, `pi-pr-review`, etc. Keep only product-specific roles or
roles that win a measured bake-off.

## `harness/extensions/cg-foreman/transport.ts`

Keep only the semantic contract worth preserving. Test `pi-oracle`/`pi-gpt` before
building transport implementation or durable browser plumbing.

## `harness/authorities.json`

Keep one-owner-per-concern as a useful inventory. Update actual owners to Prime /
selected packages. Existing `governor/*` ownership is not permanent simply because
#18 assigned it first.

## pins/bootstrap/conformance

Keep a deliberate Prime pin and a **small distribution conformance suite**. Gate the
assembled Command Governor package, not every internal implementation detail of
Prime.

---

# 14. What may actually remain custom

The final Command Governor-specific code may be very small:

1. package manifest / curated dependency profile;
2. only genuinely missing policy extension(s);
3. a thin foreman adapter around an existing ChatGPT transport if necessary;
4. product-specific roles/skills/prompts;
5. temporary compatibility code for a reproduced Prime defect only while needed;
6. focused conformance tests for the assembled behavior.

Candidate unique semantics still requiring a **gap proof**:

- implementer cannot self-satisfy the exact independent acceptance requirement;
- stale task/revision foreman responses cannot close newer work;
- explicitly user-owned decisions route back to the user;
- the exact chosen ChatGPT conversation is used.

`pi-squad`, `pi-tasks`, `pi-pr-review`, and governance packages now cover parts of
these, so none is automatically custom code.

---

# 15. What should not be on the roadmap unless this audit is disproven

Do not build:

- another provider/model runtime;
- another generic daemon/supervisor;
- another session/transcript format;
- another generic subagent framework;
- another generic scheduler/goals system;
- another general memory/refinement engine;
- another Chrome/CDP ChatGPT automation stack;
- mandatory MCP merely to return a foreman disposition;
- mandatory sandboxing for trusted-local Command Governor;
- another generic event/child/workflow runtime merely because DeepSeek has good
  architectural patterns;
- a second durable authority simply because merged code already created one.

A sandbox may remain an optional hardened profile for users intentionally running
untrusted code. It is not a core prerequisite for the trusted-local product.

---

# 16. Next work — one small evidence-driven salvage sequence

Do **not** start another broad implementation phase from merged #18 or draft #19.

## Step 1 — smallest installable Prime package skeleton

Create a minimal `@commandgovernor/harness` package using Prime's documented package
format. No parallel Governor daemon. Almost no behavior. Prove the product shape.

## Step 2 — bake alternatives under Prime, separately

A package working on upstream Pi is not automatically proven on Prime.

Priority:

1. `pi-squad` — independent acceptance semantics;
2. `pi-tasks` — work/evidence contract;
3. `pi-pr-review` — GitHub review;
4. `pi-subagents` — generic delegation/reviewer roles;
5. `pi-oracle` — exact existing ChatGPT thread;
6. `pi-gpt` — direct ChatGPT alternative;
7. `pi-observational-memory` only if a measured gap remains;
8. `pi-governance-pipeline` / DeepSeek / other donors as comparisons, not default
   stack.

Do not install overlapping authorities simultaneously during the first bake-off.

## Step 3 — re-run #18 D1/D2/D8 through the package path

Especially determine whether the raw external D2 ledger is needed at all.

Key question:

> Does the intended Prime-package product path duplicate an external effect after
> the real worker-loss scenario without our external Governor ledger?

No -> delete unnecessary control-plane machinery.
Yes -> keep the smallest workaround proven by the reproducer.

## Step 4 — prove exact foreman loop using existing transport

Use `pi-oracle` first against a user-created exact `https://chatgpt.com/c/<id>`.
Prove:

```text
candidate/review
  -> correlated request
  -> exact ChatGPT conversation
  -> durable response retrieval
  -> stale/wrong correlation rejected
  -> intended current work affected exactly once
```

Only then decide whether a tiny Governor foreman policy extension is still needed.

## Step 5 — deletion/shrink PR

Success metric: **less custom production code**.

Every #18/#19 component must end in:

```text
USE EXISTING
PLUGIN
TEMP WORKAROUND
DELETE
```

Report before/after custom production LOC and which Prime/package owner replaces
each removed subsystem.

---

# 17. ADR consequences

Do not simply flip ADR-0009 Proposed -> Accepted without incorporating this audit.

Keep:

- Prime as selected runtime substrate;
- upstream Pi as upstream/fallback;
- Command Governor reliability semantics;
- ACP as useful interoperability where needed.

Clarify:

- Command Governor defaults to a Prime package/distribution;
- existing Prime/Pi packages are preferred over new Governor subsystems;
- workarounds are temporary and tied to a reproduced upstream defect on the actual
  product path;
- ChatGPT transport composes existing Pi-native implementations when they pass
  conformance;
- sandboxing is optional hardening, not a core trusted-local prerequisite;
- DeepSeek is a donor/specialist candidate, not permission to mirror its runtime;
- S2/S3-style future gates must not prevent a usable trusted-local product when
  those gates are irrelevant to that profile.

ADR-0008's **no parallel general-purpose runtime** rule remains controlling.

ADR-0010 from draft PR #19 should not be accepted until the DeepSeek donor findings
are reclassified under this composition audit.

---

# 18. Freshness rule — narrow, not bureaucratic

The ecosystem moves fast enough that a one-day-old survey missed material packages
and DeepSeek advanced a release line during the same day.

Before implementing a new **category** of production subsystem:

> Search current Prime, current Pi, and current packages for that category; test a
> credible existing implementation before writing ours.

This is not a request for perpetual research. It is the minimum check required
before committing to another custom subsystem.

---

# Final recommendation

The next milestone is **not** “finish the Governor control plane.”

It is:

> Make Command Governor a usable Prime package with the smallest possible custom
> policy surface, while deleting or demoting merged #18 and draft #19 machinery
> that Prime/current packages make unnecessary.

The #18 engineering remains valuable failure evidence and a compatibility oracle.
The #19 DeepSeek research remains valuable donor evidence. Neither receives
permanent product ownership merely because substantial implementation work already
exists.
