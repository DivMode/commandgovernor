# Command Governor composition and de-duplication audit — 2026-09-02

Status: **post-merge architecture correction and salvage research**

This document is the source of truth for one question:

> After adopting Prime Agent as the runtime substrate, which Command Governor
> capabilities should be provided by Prime/Pi as-is, which should be composed
> from existing packages, which genuinely require a small Command Governor
> extension, which are temporary compatibility workarounds, and which custom
> code should be removed?

This is deliberately not another general architecture proposal. It consolidates
and corrects the practical consequences of ADR-0008, ADR-0009, the Pi/Prime
research, and merged PR #18.

## Executive conclusion

Command Governor should be a **curated Prime Agent package/distribution**, not a
second orchestration runtime wrapped around Prime.

The target shape is:

```text
Prime Agent
  + selected existing Pi/Prime packages
  + @commandgovernor/harness
      - small policy extension(s)
      - roles / skills / prompts
      - compatibility shim only for a proven Prime defect that still matters
        on the chosen integration path
      - conformance tests
```

The correct default for a capability is therefore:

1. **USE PRIME/PI** when the substrate already provides it.
2. **USE AN EXISTING PACKAGE** when a reviewed package provides it.
3. **SMALL COMMAND GOVERNOR EXTENSION** only for product-specific policy that
   remains missing.
4. **TEMPORARY WORKAROUND** only for a proven upstream defect that is exercised
   by the chosen product path.
5. Otherwise **REMOVE / DO NOT BUILD**.

There is no sixth outcome called “build a parallel Governor subsystem because we
can make it safer ourselves.”

## Repository state this audit starts from

PR #18 (`Prime-native foundation: Gate S1b adaptation layer`) was merged to
`main` on 2026-09-02.

- reviewed PR head: `5801a029d3b2be784f641246d9f181f4c61ac953`
- merge commit on `main`: `d9e5ab04b2037b2d2c5ac7c104780a4f6fd4a6a2`
- PR size: 84 changed files, 18,396 additions, 15 deletions
- ADR-0009 remained `Proposed` after that merge.

The merged code is therefore real repository state. This audit does **not**
pretend #18 can simply be ignored. It decides what to retain, shrink, move into
an extension, upstream, or remove in a follow-up salvage PR.

## Prior Command Governor sources reviewed

The audit incorporates:

- `docs/adr/0008-adopt-pi-native-command-governor-harness.md`
- `docs/adr/0009-prime-agent-substrate-and-acp-boundary.md`
- `docs/research/2026-09-01-pi-native-command-governor-harness-review.md`
- `docs/research/2026-09-01-agent-harness-landscape-and-substrate-bakeoff.md`
- `docs/research/2026-09-01-rust-invariant-catalog.md`
- `docs/prime-native/adaptation-layer.md`
- `docs/upstream/2026-09-01-prime-worker-loss-journal.md`
- `harness/authorities.json`
- merged PR #18 and its review history.

ADR-0008 had the important strategic direction right: Command Governor should be
Pi-native, composition-first, and should not own another general provider /
session / subagent / memory / browser runtime. ADR-0009 correctly selected Prime
as the stronger runtime substrate, but some of its later gates and the S1b
implementation allowed a compatibility problem to grow into a substantial
external adaptation layer. This document corrects that implementation direction
without discarding the reliability requirements themselves.

## External snapshots used

Research was refreshed on 2026-09-02 rather than relying only on the 2026-09-01
survey.

### Prime Agent

Selected production pin from ADR-0009 / #18:

```text
Prime Agent v0.8.1
514633727bf26d74f39f3119c2b0e31a5ceb2a9d
```

Current upstream `main` checked during this audit:

```text
0ba0423c5c18805c72ad03d03aaf1d9e0cc622d0
```

Source: <https://github.com/PrimeIntellect-ai/prime-agent>

### Upstream Pi

Current upstream `main` checked during this audit:

```text
e266507b606b9552fa277252644054afd4384b11
```

Source: <https://github.com/earendil-works/pi>

The current snapshots are research inputs, not silent dependency updates. Any
actual re-pin remains a deliberate compatibility change.

## Finding 1 — Prime already has the runtime we were trying to govern externally

At the selected pin Prime already provides the important runtime mechanics:

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
- ACP mode;
- extensions and packages.

Prime's long-running-agent model explicitly makes the resident worker, not a
terminal client, the owner of the queue, schedules, session, kernel, and
children. These are not Command Governor implementation targets.

Primary docs:

- `packages/coding-agent/docs/long-running-agents.md`
- `packages/coding-agent/docs/daemon.md`
- `packages/coding-agent/docs/extensions.md`
- `packages/coding-agent/docs/packages.md`

## Finding 2 — the Governor-specific layer can be a Prime package

Prime packages bundle extensions, skills, prompts, and themes and can be
installed from npm, Git, or a local path. For compatibility with the inherited Pi
ecosystem they use the `pi` package manifest key.

At the selected 0.8.1 pin an extension can:

- subscribe to lifecycle events;
- intercept/block tool calls;
- inject or modify context;
- register tools and commands;
- persist custom session entries with `pi.appendEntry()`;
- perform asynchronous initialization;
- use normal Node dependencies and built-ins;
- perform external integrations.

Therefore the default product artifact should be a package like:

```text
@commandgovernor/harness
  extensions/
    policy.ts
    [only other genuinely necessary extensions]
  skills/
  prompts/
  agents-or-role-resources/
  package.json
```

A separate Governor daemon is not required merely to express review policy,
role policy, user-decision gates, or ChatGPT integration.

Primary sources:

- <https://github.com/PrimeIntellect-ai/prime-agent/blob/514633727bf26d74f39f3119c2b0e31a5ceb2a9d/packages/coding-agent/docs/extensions.md>
- <https://github.com/PrimeIntellect-ai/prime-agent/blob/514633727bf26d74f39f3119c2b0e31a5ceb2a9d/packages/coding-agent/docs/packages.md>

## Finding 3 — current package ecosystem duplicates more of Governor than the old research captured

The package ecosystem moved quickly enough that a one-day-old architecture review
is already incomplete. The following packages are current candidates, not automatic
adoptions.

### Work/evidence state: `pi-tasks`

Current package checked: `pi-tasks` 0.2.4.

It already provides:

- evidence-gated completion;
- acceptance criteria;
- decisions and blockers;
- ordered plans;
- scope-drift detection;
- branch-aware persistent events;
- compaction-safe resume;
- completion refusal while evidence/criteria/blockers are incomplete.

This overlaps directly with the old Governor “durable obligation + evidence”
concept. It does not provide subagent orchestration by itself, so it should be
bake-tested as a work/evidence component, not mistaken for the whole product.

Source: <https://pi.dev/packages/pi-tasks>

### Independent acceptance semantics: `pi-squad`

`pi-squad` is especially relevant because its agents cannot mark a squad
accepted. When candidates finish, status becomes `review`; main Pi must
independently inspect the original contract, actual source/diff, and verification,
then submit `squad_review`. Failed review remains review-blocked and rework occurs
inside the same authoritative squad; pending/failed review gates survive restart.

That is extremely close to the core Command Governor distinction:

```text
worker finished != work accepted
```

This package deserves a focused source/security/runtime bake-off before we write
our own review state machine.

Source: <https://pi.dev/packages/pi-squad>

### General delegation/review: `pi-subagents`

Current package checked: `pi-subagents` 0.57.0.

It ships worker, reviewer, oracle, scout, and researcher roles; foreground and
background runs; parallel reviewers; review loops; and implement-then-review
workflows. It is extremely popular relative to most packages and is a natural
candidate for generic delegation.

Its ordinary review policy is still prompt/config driven, so it must not be
confused with a hard acceptance authority by itself.

Source: <https://pi.dev/packages/pi-subagents>

### GitHub PR review: `pi-pr-review`

Current package checked: `pi-pr-review` 1.17.5 (published 2026-09-01).

It already supplies multi-lane parallel PR review, host-owned structured
findings, exact reviewed-head/staleness checks, optional verification, and gated
COMMENT/APPROVE publishing to GitHub. It is a much stronger candidate for the
GitHub review surface than building custom reviewer plumbing.

Source: <https://pi.dev/packages/pi-pr-review>

### Multi-model governance pipeline: `pi-governance-pipeline`

Current package checked: 1.0.14.

It already separates implementation from independent reviewers, routes roles to
different models/providers, severity-gates findings, enforces a minimum reviewer
panel, and keeps hard run budgets outside model context.

It is more opinionated than Command Governor may want. Treat it as a candidate /
donor and bake it against our workflow; do not install it merely because its name
sounds aligned.

Source: <https://pi.dev/packages/pi-governance-pipeline>

### Other review/workflow donors

Additional packages found in the refreshed search include:

- `@agwab/pi-workflow` — deep review, spec review, impact review with audited
  final reports;
- `gentle-pi` — adversarial review/finalization and release receipts;
- `@misunders2d/pi-goal` — autonomous goal execution with independent completion
  audit;
- `pi-team`, `pi-agentteam`, `pi-agents-team`, `pi-teams` — overlapping multi-agent
  coordination models.

These reinforce the composition direction; they are not a recommendation to load
multiple overlapping authorities at once.

## Finding 4 — do not build a Command Governor ChatGPT browser runtime

Two existing Pi-native transports remain the leading candidates.

### `pi-oracle` — preferred first exact-thread bake-off

`pi-oracle` already supports:

- an explicit user/browser-created `https://chatgpt.com/c/<id>` conversation;
- normalized exact conversation IDs;
- same-conversation leases;
- detached background workers;
- isolated per-job browser profiles;
- durable job state, response text, and artifacts;
- persisted same-thread follow-ups;
- best-effort Pi wake-up while preserving the result if the wake is missed.

This is materially more complete than the browser transport Command Governor was
planning to build.

Source:
<https://github.com/fitchmultz/pi-oracle/blob/main/docs/ORACLE_DESIGN.md>

### `pi-gpt` — direct/private transport candidate

Current package checked: `pi-gpt` 0.4.2.

It exposes ChatGPT/Codex account, model, chat, conversation, and message operations
inside Pi and can continue conversations. It uses undocumented ChatGPT backend
interfaces, so it should remain capability-gated and replaceable rather than the
only supported path.

Source: <https://pi.dev/packages/pi-gpt>

### Consequence for the merged `cg-foreman` transport stub

`harness/extensions/cg-foreman/transport.ts` should not be treated as settled
architecture. Its comments were written from an older candidate analysis and
contain assumptions that no longer survive the refreshed evidence. In particular,
`pi-oracle` demonstrates that a Pi extension can coordinate detached background
workers and durable external job state.

The next transport work should therefore be **adapter/bake-off work around an
existing package**, not implementation of another Chrome/CDP stack.

## Finding 5 — memory/refinement should not become a Governor subsystem

Prime itself now has a continual-harness refinement system covering prompt notes,
memories, skills, and subagent specs. That already implements the major direction
we researched from Continual Harness / ACE.

For observational memory, `pi-observational-memory` 3.0.4 is a current mature
candidate with observation-centered memory, durable reflections, background
memory work, source-backed recall, and compaction integration.

Sources:

- Prime `skills/refine/SKILL.md` and refinement implementation;
- <https://pi.dev/packages/pi-observational-memory>
- <https://pi.dev/packages/pi-continual-harness> as a smaller upstream-Pi donor.

Command Governor should choose/configure at most one owner for each memory concern
and evaluate downstream action quality. It should not implement another memory
engine.

## Finding 6 — upstream Pi has a stronger settled lifecycle event than Prime

Current upstream Pi exposes `agent_settled`, defined as the point where a run is
fully settled and no automatic retry, compaction retry, or queued continuation
remains.

Current Prime source search on 2026-09-02 found no `agent_settled` equivalent by
that name; the selected Prime 0.8.1 extension docs expose `agent_end` once per
prompt.

This is the kind of generic gap that should be proposed upstream to Prime (or
adapted through an existing public Prime lifecycle surface), not replaced with a
new provider-specific Governor lifecycle detector.

Pi source:
<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md>

## Finding 7 — the Prime worker-loss journal defect is real, but its product relevance depends on architecture

The S1b research found a real defect: a worker socket can close after an effect,
and the supervisor catch path can persist the ordinary failure response in the
command journal. Current Prime `main` still contains the worker-socket-close path
and the supervisor path that records ordinary caught failures into the command
journal, so this is not invented work.

However, the huge architectural question is **who consumes that response and what
they do next**.

The #18 D2 adaptation is required when Command Governor is an external raw daemon
client that interprets outcomes and may create replacement commands. If the
product becomes an in-process Prime package that does not implement an external
mutation/re-dispatch authority, the duplicate-effect risk may no longer require a
separate Governor mutation ledger at all.

That must be proved with a small black-box spike:

1. run the same real worker-loss reproducer through the intended Prime-package
   product path;
2. prove whether stock Prime or any selected package automatically issues a new
   mutation after the uncertain worker loss;
3. if no automatic replacement exists, remove the external D2 control-plane path;
4. if a real duplicate remains possible on the product path, keep only the
   minimum compatibility shim and retain the reproducer;
5. upstream the generic defect where practical and define the shim's removal
   condition.

Current Prime source checked:

- `packages/coding-agent/src/modes/daemon/daemon-worker-client.ts`
- `packages/coding-agent/src/modes/daemon/daemon-supervisor.ts`

The durable command journal correctly refuses replay of a still-pending command
under the same identity via `command_result_uncertain`; the residual problem is
how a worker-close failure gets finalized and how a higher-level caller reacts.

## Capability disposition matrix

| Concern | Preferred owner now | Disposition |
| --- | --- | --- |
| agent/model runtime | Prime | **USE PRIME** |
| daemon supervisor / resident workers | Prime | **USE PRIME** |
| session JSONL / resume / tree | Prime | **USE PRIME** |
| worker/session leases | Prime | **USE PRIME** |
| schedules / heartbeats | Prime | **USE PRIME** |
| goals / autonomous continuation | Prime | **USE PRIME** |
| RLM / recursive children | Prime | **USE PRIME** |
| agent-to-agent messaging | Prime | **USE PRIME** |
| continual-harness refinement | Prime | **USE PRIME** |
| generic delegation | Prime RLM and/or `pi-subagents` | **BAKE-OFF; DO NOT BUILD** |
| durable work/evidence contract | `pi-tasks` candidate | **BAKE-OFF; DO NOT BUILD YET** |
| independent candidate acceptance | `pi-squad` candidate | **BAKE-OFF FIRST** |
| GitHub PR review | `pi-pr-review` candidate | **BAKE-OFF FIRST** |
| multi-model governance | `pi-governance-pipeline` donor/candidate | **BAKE-OFF; MAY BE TOO OPINIONATED** |
| exact ChatGPT existing-thread transport | `pi-oracle` | **FIRST CANDIDATE** |
| direct ChatGPT transport | `pi-gpt` | **SECOND / EXPERIMENTAL CANDIDATE** |
| observational memory | `pi-observational-memory` candidate | **OPTIONAL BAKE-OFF** |
| generic settled lifecycle | upstream Pi has `agent_settled` | **UPSTREAM/ADAPT, DO NOT REBUILD** |
| user-owned high-risk decisions | small policy extension if packages do not cover it | **POSSIBLY GOVERNOR-SPECIFIC** |
| stale task/revision/foreman correlation | small policy extension if needed after transport bake-off | **POSSIBLY GOVERNOR-SPECIFIC** |
| raw Prime mutation classifier/ledger | only if raw external daemon-client path remains | **CONDITIONAL TEMP WORKAROUND** |
| sandbox | none by default | **OPTIONAL PROFILE ONLY; NOT A PRODUCT PREREQUISITE** |
| ACP | Prime stable ACP | **INTEROP SURFACE, NOT INTERNAL AUTHORITY** |

## PR #18 salvage disposition

PR #18 was correctly reviewed for the architecture it implemented. The salvage
question is different: **which of those components should remain once the product
stops acting like an external competing control plane?**

### Preserve as evidence until the replacement path is proven

Do not immediately delete the valuable real reproducers and specifications:

- `conformance/runtime/d2-worker-loss-uncertain.test.ts`
- `conformance/runtime/d2-import-jsonl-post-effect.test.ts`
- `conformance/runtime/d1-resident-root-recovery.test.ts`
- `conformance/runtime/d8-explicit-session-path.test.ts`
- pin/protocol drift tests;
- crash/restart negative controls that describe a real required product behavior.

During salvage, convert surviving requirements into black-box package-level
conformance tests wherever possible. Tests of code that is intentionally removed
should be removed with that code after equivalent product-level evidence exists;
we do not keep thousands of internal tests merely to preserve sunk cost.

### `governor/prime/*`

Current role: external raw daemon protocol client, substrate launcher, client
identity, protocol types, environment handling.

Preferred disposition:

- **REMOVE from the product path** if the Prime package can run entirely through
  Prime's normal extension/session APIs;
- retain only pin/protocol conformance or a truly necessary narrow adapter;
- do not maintain a second generic Prime client just because it exists now.

### `governor/session/*`

Current role: custom session registry, canonical path policy, incarnation/recovery
logic.

Preferred disposition:

- re-run D1/D8 through the intended package path;
- if Prime's normal high-level lifecycle already supplies the needed behavior,
  **REMOVE registry/reopen authority**;
- if one product-specific preflight policy is still useful (for example refusing
  an unsafe path shape), implement it as a small extension policy rather than a
  parallel session authority.

### `governor/mutation/*`

Current role: D2 classification, digest, durable ledger, proof matrix.

Preferred disposition:

- **do not assume it remains**;
- first prove whether the package product path ever needs to interpret raw daemon
  failure and create replacements;
- if not, remove the mutation ledger/classifier from the normal product path and
  keep the Prime defect as an upstream/conformance concern;
- if yes, retain the smallest compatibility module that closes the reproduced
  duplicate-effect path and nothing more.

### `governor/fs/*` and `governor/process/*`

These exist primarily to make the custom authoritative registries/ledgers
process-safe. If those authorities disappear, these helpers should disappear too
unless a surviving compatibility shim demonstrably needs them.

### `harness/agents/*`

Role definitions are compatible with a package-shaped product, but do not assume
we need our own generic worker/reviewer definitions when selected packages already
ship good ones. Keep only roles that encode genuinely Command Governor-specific
policy or outperform package defaults in a measured bake-off.

### `harness/extensions/cg-foreman/transport.ts`

Keep the high-level semantic requirements (exact thread, stale revision rejection,
ambiguous send is not blind retry). Rework or replace the transport abstraction
after testing `pi-oracle` and `pi-gpt`; do not implement the old standalone
browser plan.

### `harness/authorities.json`

Keep the concept of one owner per concern, but update the actual ownership map so
Prime and selected packages own their real concerns. It must not make existing
`governor/*` files permanent merely because #18 assigned them first.

### `pins/*`, bootstrap, and conformance runner

Keep a deliberate Prime pin and a small compatibility/conformance suite. The
suite should gate **our distribution and selected packages**, not attempt to
retest every Prime internal implementation detail.

## The minimum Command Governor-specific product that may remain

After composition, custom Command Governor behavior may be as small as:

1. a package manifest / curated dependency profile;
2. a policy extension that enforces only genuinely missing Command Governor
   rules;
3. a foreman adapter around an existing ChatGPT transport if no package already
   exposes the exact structured disposition we need;
4. role/skill/prompt resources that are actually product-specific;
5. compatibility code for a proven Prime defect only while it remains necessary;
6. focused conformance tests proving the assembled product behavior.

Candidate product-specific rules that still require proof of a gap:

- an implementer cannot satisfy the exact independent-acceptance requirement by
  self-certification;
- stale task/revision foreman responses cannot close newer work;
- explicitly user-owned decisions are routed back to the user;
- the exact chosen ChatGPT foreman conversation is used.

Even these are **not automatically custom code**. `pi-squad`, `pi-tasks`,
`pi-pr-review`, and governance packages now cover parts of them and must be tested
first.

## What should NOT be on the roadmap unless this audit is disproven

Do not build:

- another provider/model runtime;
- another generic daemon/supervisor;
- another session/transcript format;
- another generic subagent framework;
- another generic scheduler/goals system;
- another general memory/refinement engine;
- another Chrome/CDP ChatGPT automation stack;
- mandatory MCP merely to return a foreman disposition;
- mandatory sandboxing for trusted local Command Governor use;
- a second durable authority simply because the previous architecture already
  wrote one.

A sandbox can remain an optional hardened profile for users who intentionally run
untrusted code. It is not a prerequisite for the core product described here.

## Next work — one small evidence-driven salvage sequence

Do **not** start another broad implementation phase from the merged #18 state.
Run this sequence instead.

### 1. Create the smallest installable Command Governor Prime package skeleton

Use Prime's documented package format. It should load under the selected Prime pin
without a parallel Governor daemon.

The first version should contain almost no behavior; its purpose is to prove the
product shape.

### 2. Bake existing packages under Prime, not only upstream Pi

A Pi package being good on upstream Pi does not automatically prove it is safe on
Prime. For each candidate, test install/load, lifecycle compatibility, state
ownership, restart behavior, and clean removal.

Priority order:

1. `pi-squad` — independent acceptance semantics;
2. `pi-tasks` — durable work/evidence contract;
3. `pi-pr-review` — GitHub review;
4. `pi-subagents` — generic delegation/reviewer roles;
5. `pi-oracle` — exact existing ChatGPT thread;
6. `pi-gpt` — direct ChatGPT alternative;
7. `pi-observational-memory` only if Prime's own refinement/memory behavior leaves
   a measured gap;
8. `pi-governance-pipeline` as a comparison/donor rather than a default stack.

Do not install overlapping task/review authorities together during the first
bake-off. Test alternatives separately, then select one owner per concern.

### 3. Re-run the #18 failure scenarios through the package path

Especially D1, D2, and D8.

The key D2 question is no longer “is the external ledger correct?” It is:

> Does the intended Prime package product path create a duplicate external effect
> after the real worker-loss scenario without that external ledger?

If no, delete the unnecessary ledger architecture. If yes, keep the smallest
workaround required by the reproducer.

### 4. Prove the foreman loop with existing transport

Target a user-created exact `https://chatgpt.com/c/<id>` using `pi-oracle` first.
Prove:

```text
candidate work/review
  -> correlated request
  -> exact existing ChatGPT thread
  -> response is durably retrievable
  -> stale/wrong correlation is rejected
  -> disposition affects the intended current work exactly once
```

Only after that test should we decide whether a tiny Governor foreman-policy
extension is required.

### 5. Produce a deletion/shrink PR

The success metric for the next implementation PR is **less custom production
code**, not more.

Every merged #18 component should end in one of four final buckets:

```text
USE EXISTING
PLUGIN (small and product-specific)
TEMP WORKAROUND
DELETE
```

The PR should report before/after custom production LOC and exactly which Prime or
package owner replaced each removed subsystem.

## ADR consequences

Do not simply flip ADR-0009 from Proposed to Accepted without incorporating this
audit.

A focused ADR correction should preserve:

- Prime as the selected runtime substrate;
- upstream Pi as architectural upstream/fallback;
- stable ACP as optional/public interoperability where useful;
- the Command Governor reliability semantics.

It should clarify:

- Command Governor's default implementation is a Prime package/distribution;
- existing Prime/Pi packages are preferred over new Governor subsystems;
- compatibility workarounds are temporary and tied to a reproduced upstream
  defect/product path;
- ChatGPT transport is composed from existing Pi-native transports when they pass
  conformance;
- sandboxing is optional hardening for intentionally untrusted workloads, not a
  core prerequisite;
- S2/S3-style gates must not block getting a usable trusted-local product when
  they are not required by that product profile.

ADR-0008's no-parallel-runtime decision remains the controlling principle.

## Freshness rule for future architecture work

The package ecosystem is changing fast enough that yesterday's survey was already
missing relevant packages. Before implementing a new **category** of production
capability, refresh current Prime/Pi/package evidence for that category.

This is not a request for perpetual broad research or another bureaucracy. It is
a narrow rule:

> If we are about to write a new subsystem, first search whether Prime/Pi/current
> packages already ship that subsystem.

If an existing implementation is found, test it before writing ours.

## Final recommendation

The next milestone is **not** “finish the Governor control plane.”

It is:

> Make Command Governor a usable Prime package with the smallest possible custom
> policy surface, while deleting or demoting merged #18 machinery that the
> package architecture makes unnecessary.

The #18 engineering remains useful as failure evidence and a compatibility oracle.
It does not get permanent ownership merely because it passed review and merged.
