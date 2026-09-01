# ADR 0008: Adopt Pi as the Command Governor harness substrate

- **Status:** Accepted
- **Date:** 2026-09-01
- **Research:** [`../research/2026-09-01-pi-native-command-governor-harness-review.md`](../research/2026-09-01-pi-native-command-governor-harness-review.md)

## Context

Command Governor was originally designed as an independent Rust-first durable control plane around external worker runtimes. ADRs 0001–0007 established a one-daemon authority, SQLite event/obligation state, worker/runtime adapters, ChatGPT Web delivery, a foreman MCP ABI, provider-specific lifecycle interpretation, session lineage/loadouts, observational memory, and analytics.

That design correctly identified important **behavioral invariants**, but it also committed Command Governor to independently owning an increasingly broad agent-harness surface:

- provider/model integration;
- session persistence/resume/fork/branching;
- compaction and context restoration;
- Claude/Codex/future worker adapters;
- subagents and recursive delegation;
- worker input/steering;
- memory/observer/consolidator systems;
- process/runtime supervision;
- verification hooks;
- analytics;
- ChatGPT Web integration.

A broader review on 2026-09-01 found that Pi and its ecosystem already implement or expose strong primitives for nearly all of those concerns. Pi core provides persistent sessions, RPC/SDK embedding, a deep extension event surface, provider abstraction, compaction, branching, and the stronger `agent_settled` lifecycle event. Public Pi packages provide asynchronous/interactive subagents, observational memory, continual-harness/ACE-style refinement, durable local tasks, crash-reconciled supervision, hooks/verification, memory, and ChatGPT Web transports.

The user's retained `DivMode/pi-config` fork is itself a fork of `amosblomqvist/pi-config`, the configuration family previously studied for interactive subagents and observational memory. Prime Intellect's Prime Agent provides further evidence that a serious long-running, persistent, self-improving harness can evolve from the Pi substrate rather than reimplementing the entire coding-agent stack independently.

The cost of duplication has therefore become larger than the benefit of owning every layer.

## Decision

### 1. Command Governor becomes a Pi-native harness/distribution

**Command Governor remains the product name, repository, and public identity. Pi becomes its runtime/harness substrate.**

The canonical product remains:

- repository: `DivMode/commandgovernor`;
- name: **Command Governor**;
- domain: `commandgovernor.com`.

Command Governor will be delivered as a curated, tested Pi-native harness composed from:

- pinned Pi runtime/packages;
- selected third-party Pi extensions/packages;
- Command-Governor-specific extensions where genuine gaps remain;
- agent-role definitions;
- skills;
- prompts/policies;
- memory/continual-harness configuration;
- verification and analytics configuration;
- ChatGPT Web foreman integration;
- compatibility and conformance tests.

`DivMode/pi-config` remains a research/reference fork. It does **not** replace `DivMode/commandgovernor` as the product repository.

### 2. Do not build a parallel general-purpose Command Governor agent runtime

Command Governor will no longer independently implement a second provider/session/subagent/memory runtime when Pi core or reviewed Pi packages can satisfy the required behavior.

The following are no longer default implementation targets for bespoke Command Governor infrastructure:

- provider-specific agent loops;
- `governor-worker-claude` / `governor-worker-codex` style general adapters;
- a competing session/transcript format;
- a separate generic subagent framework;
- a separate generic compaction engine;
- a separate generic observational-memory engine;
- a separate generic provider model registry;
- Herdr-specific lifecycle inference as a core product dependency.

A new Command Governor implementation component must justify why Pi's public core/SDK/RPC/extension surfaces plus reviewed packages cannot safely satisfy the requirement.

### 3. Composition-first policy

For every capability, use this order:

1. **ADOPT** a Pi core capability when it meets the contract.
2. **COMPOSE** a reviewed Pi package/extension when it meets the contract.
3. **CONTRIBUTE UPSTREAM** a generic missing primitive where practical.
4. **EXTEND** with a Command Governor Pi package only for a real product-specific gap.
5. **FORK Pi core** only as a last resort when a required primitive cannot be implemented safely through supported extension/SDK/RPC surfaces and an upstream solution is unavailable on the required timeline.

Any core fork must document the exact delta, pin, conformance tests, and upstream/exit plan.

### 4. The Command Governor reliability contract survives the implementation pivot

The following invariants remain product requirements and must be tested independent of which Pi package implements them:

1. Delegated work does not disappear because a Pi session, subagent, terminal, helper process, browser, or ChatGPT turn restarts.
2. Worker completion is distinct from required foreman processing/review.
3. Foreman events and replies are correlated to exact task/revision identities; stale replies cannot close newer work.
4. Ambiguous external delivery is reconciled before retry; it is never blindly replayed merely because the local process restarted.
5. Lossy memory, generated summaries, and provider compaction are not authority for exact lifecycle, capabilities, safety rules, or user-owned decisions.
6. Worker/subagent roles and resumed loadouts are explicit and least-authority; resume cannot silently broaden an old worker under new defaults.
7. High-risk/destructive/credential-sensitive/materially broader decisions remain user-owned unless explicitly delegated.
8. Independent review cannot be satisfied by an implementer self-certifying its own work.
9. GitHub remains the engineering source of truth for GitHub-backed issues, commits, PRs, and reviews.

These are **Command Governor semantics**, not a mandate for one Rust daemon or one database technology.

### 5. Command Governor durability becomes Pi-native

“No standalone Governor control plane” does not mean “no durable state” or “no helper daemon.”

Command Governor may use:

- Pi's session persistence;
- extension-owned durable sidecars/artifacts;
- Pi-native task/event stores;
- reviewed Pi supervisor/task daemons;
- browser worker processes;
- local indexes/memory stores;

when needed for correctness or performance.

The rejected architecture is a second **general-purpose orchestration authority** that duplicates Pi's runtime. Purpose-built Pi-native helpers are allowed and may outlive the interactive Pi process when crash survival requires it.

### 6. ChatGPT Web foreman integration moves into the Pi harness

The preferred architecture is a closed loop owned by Pi:

```text
Pi worker/subagent finishes
  -> Command Governor Pi extension creates durable foreman event
  -> Pi ChatGPT-Web transport sends event to exact bound conversation
  -> ChatGPT Web foreman reviews/reasons
  -> Pi reads the correlated foreman response
  -> Command Governor validates + durably records disposition
  -> ACK | REVISE | DELEGATE | ASK_USER
```

This removes the architectural requirement that ChatGPT must call a separate Command Governor MCP server merely to return a disposition.

### 7. MCP becomes optional interoperability, not the mandatory spine

ADR 0004's exact-binding/correlation concerns remain valid, but its mandatory foreman MCP shape is superseded.

If a direct Pi ↔ ChatGPT Web closed loop can send and read the exact foreman conversation safely, MCP is optional. Command Governor may still expose/use MCP where it adds interoperability or where a target ChatGPT capability requires it, but the product must not depend on MCP solely because the old topology did.

ADR 0006's empirical capability-testing principle remains useful for any MCP adapter that is shipped, but MCP mutation capability is no longer a universal Command Governor gate.

### 8. ChatGPT transport is capability-gated and replaceable

Two Pi-native transport families are currently relevant:

- **direct candidate:** `pi-gpt`, which exposes ChatGPT chat/conversation/message operations from Pi through an undocumented ChatGPT web backend;
- **web-app candidate/fallback:** `pi-oracle`, which can explicitly target an existing `https://chatgpt.com/c/<id>` conversation in an isolated authenticated browser runtime and durably stores job results.

The architecture does not permanently bless either adapter. The preferred transport is the simplest one that passes the Command Governor foreman conformance suite.

A direct/private interface must be treated as undocumented, capability-gated, replaceable, and subject to compatibility/terms risk. Command Governor does not define bypassing provider security controls as a product requirement.

### 9. Existing Pi memory and continual-harness work is the default starting point

ADR 0007's memory and compaction principles are retained, but its “independently implement the mechanisms in Rust” strategy is superseded.

Command Governor should first evaluate and compose:

- `pi-observational-memory` or stronger alternatives for tiered observer/consolidator memory;
- `pi-continual-harness` / related Pi-native implementations for ACE/Continual-Harness-style structured refinement;
- reviewed Pi memory/task/supervision packages where their contracts fit.

The Stanford/MemoryArena requirements remain acceptance criteria: memory must improve correct downstream action, not merely recall; exact control/safety/capability facts remain deterministic and non-lossy.

### 10. Existing Rust Phase-1 code is frozen pending Pi-native parity

The current Rust core/store/artifact/testkit/daemon scaffold is not immediately deleted by this ADR, because it contains reviewed invariants and tests that are useful as migration oracles.

However:

- no new general-purpose live worker/runtime/browser/MCP architecture should be built on the old Rust topology;
- the Rust implementation is **frozen for feature expansion** while the Pi-native parity spike executes;
- useful behavioral tests/specifications should be translated into Pi-native conformance tests;
- after Pi-native parity is proven, redundant Rust runtime crates should be archived or removed in a dedicated migration PR rather than kept as a second production path.

The goal is one product architecture, not permanent dual runtimes.

## Supersession map

### ADR 0001 — durable orchestration control plane

**Partially superseded.** The mission and durable-work/review invariants remain. The one-authoritative-custom-daemon implementation topology is superseded by the Pi-native harness architecture.

### ADR 0002 — Rust daemon + `rusqlite`

**Superseded as the V1 product runtime decision.** Rust/SQLite may still be used by a specific Pi-native helper if justified, but Command Governor is no longer defined as a Rust daemon + CLI product.

### ADR 0003 — ChatGPT browser-backed hybrid

**Superseded as the mandatory/default transport.** Browser-backed exact-thread transport remains a valid fallback/candidate through Pi-native packages such as `pi-oracle`; direct Pi transport is tested first when it offers stronger/simpler semantics.

### ADR 0004 — foreman MCP + binding

**Partially superseded.** Exact conversation binding, generation/revision correlation, and explicit disposition remain. MCP is no longer mandatory if Pi can read the foreman response directly.

### ADR 0005 — structured Claude lifecycle + result durability

**Implementation superseded; semantics retained.** Pi lifecycle events, Pi-native subagent/session mechanisms, and selected packages replace a bespoke Claude-first worker-host architecture. Durable final-result/recovery tests remain required.

### ADR 0006 — empirical ChatGPT MCP capability gate

**Superseded as a universal gate.** Capability testing remains required for any shipped MCP path, but Command Governor may operate without foreman MCP.

### ADR 0007 — session lineage, memory, compaction, analytics

**Semantic requirements retained; implementation strategy superseded.** Lineage/loadouts, advisory memory, analytics, type-aware exact-state preservation, and downstream-action testing remain. Existing Pi-native implementations are now preferred over independent Rust reimplementation.

## Initial Command Governor Pi-native stack direction

The exact package set remains subject to source/security/license/conformance review, but the default investigation order is:

| Concern | First candidates |
| --- | --- |
| base harness | upstream Pi |
| sessions/branching/compaction | Pi native |
| subagents | `pi-subagents`, `pi-interactive-subagents`, durable alternatives |
| process/task supervision | `@geminixiang/pi-task-protocol`, `@geminixiang/pi-supervisor`, alternatives |
| observational memory | `pi-observational-memory`, alternatives |
| continual refinement | `pi-continual-harness`, Prime Agent patterns |
| verification/hooks | Pi-native verification/hooks packages |
| ChatGPT direct transport | `pi-gpt` capability spike |
| ChatGPT existing-thread web fallback | `pi-oracle` |
| MCP interoperability | Pi MCP packages if/when needed |
| Command-Governor-specific foreman protocol | build the smallest missing Pi extension |

Popularity alone is not acceptance. Every dependency must be pinned, reviewed, licensed, and exercised by Command Governor conformance tests.

## Foreman protocol direction

The Pi-native foreman exchange should be structured and exact. A conceptual envelope is:

```text
FOREMAN_EVENT
  task_id
  task_revision
  delivery_id
  event_kind
  bounded result/reference payload

FOREMAN_ACTION
  task_id
  task_revision
  delivery_id
  action = ACK | REVISE | DELEGATE | ASK_USER
  instructions / delegation / question
```

The exact serialization is not frozen by this ADR. The required semantics are:

- stale revision rejection;
- duplicate reply idempotence;
- durable disposition before worker side effects;
- restart recovery;
- ambiguous-send reconciliation;
- user-owned decision routing.

## Acceptance gates before removing the old runtime

### Gate P1 — Pi substrate pin and package loading

A pinned Pi release/revision must load the Command Governor distribution reproducibly on the supported local platform, with project/global resource precedence characterized and version drift detected.

### Gate P2 — durable subagent lifecycle

Prove spawn, parallel children, role/tool restrictions, child input wait, answer/resume, completion, parent restart, orphan handling, and result recovery without relying on screen state.

### Gate P3 — memory and compaction

Prove repeated compaction does not erase exact policy/control constraints and run dependent-session tests where earlier experience must change a later action. Memory worker failure must not corrupt task truth.

### Gate P4 — ChatGPT Web foreman closed loop

Against the exact target consumer ChatGPT Web conversation:

1. bind/identify the exact conversation;
2. send a unique task/revision/delivery event;
3. receive/read the resulting foreman action;
4. validate correlation;
5. durably record disposition;
6. execute ACK/revise/delegate/wait exactly once.

Inject failures before/during/after send and before/after local disposition. Blind duplicate submission is not acceptable.

### Gate P5 — independent review workflow

Prove an implementer can produce work, a separate reviewer role can inspect source/GitHub evidence independently, and the foreman can disposition the result without an implementer self-approving.

### Gate P6 — observability and cache/cost accounting

Expose session/role/provider/model cost and input/output/cache metrics where providers report them. Optimize for correctness and fresh/total token efficiency rather than a cache-hit percentage alone.

## Consequences

### Positive

- dramatically less bespoke infrastructure to build and maintain;
- immediate access to Pi's multi-provider runtime and rapidly evolving ecosystem;
- sessions, compaction, branching, subagents, memory, and continual refinement become composable rather than separately invented;
- new model/provider support can often arrive through upstream Pi without a Command Governor adapter project;
- Command Governor code can focus on differentiated foreman orchestration, policy, review, reliability, and curated defaults;
- smaller stable harnesses can improve prompt/cache efficiency and reduce redundant context traffic;
- community packages can be improved upstream instead of copied into a private architecture;
- the existing Command Governor name/product can become a recognizable opinionated Pi power harness rather than another isolated agent framework.

### Costs

- Command Governor now depends strategically on Pi's evolution and extension contracts;
- third-party extension quality/security/maintenance varies and requires curation;
- overlapping packages can create conflicting authorities if installed carelessly;
- upgrades require a compatibility/conformance matrix, not casual “latest” updates;
- some reliability gaps may still require durable sidecars/helper daemons;
- undocumented ChatGPT Web transports can break and must remain replaceable;
- the existing Rust scaffold becomes migration/legacy work rather than the product foundation.

## Alternatives considered

### Continue the Rust-first standalone Governor architecture

Rejected. It would duplicate too much of Pi's now-proven runtime/session/extension ecosystem and make every provider, subagent, memory, compaction, and lifecycle evolution Command Governor's maintenance burden.

### Replace Command Governor with an off-the-shelf Pi config and drop the product

Rejected. Command Governor still has a differentiated product goal: a curated, durable, foreman-led engineering harness with strong review/recovery/policy defaults. The name, repository, conformance suite, and integration layer remain valuable.

### Hard-fork Pi immediately

Rejected as the default. A hard fork creates permanent merge debt and makes the surrounding Pi package ecosystem harder to consume. Start with pinned upstream Pi plus extensions; fork only when a proven missing primitive requires it.

### Keep a tiny Rust Governor daemon plus Pi

Not the default. A purpose-built helper daemon may still be useful for a concrete Pi-native feature, but preserving a separate authoritative Governor process merely because the previous architecture had one is rejected. The need must be demonstrated by a failure that Pi-native persistent state/helper packages cannot solve.

### Keep mandatory ChatGPT MCP ACK even if Pi can read replies

Rejected. If Pi owns both outbound and inbound ChatGPT Web transport, a correlated durable foreman response can carry the disposition directly. MCP should exist only when it provides an actual capability/interoperability benefit.

## Migration rule

Until the Pi-native gates pass, old Rust behavior/specs remain useful reference material. After parity passes, there must be no ambiguous “two Command Governors” state: one production Pi-native architecture becomes authoritative and redundant old runtime code is removed/archived deliberately.

## Erratum — 2026-09-01

The decision above stands unchanged. Two factual corrections to the “Initial Command Governor Pi-native stack direction” table, established while pinning the substrate for Gate P1 and recorded here rather than by rewriting an accepted ADR.

**1. The npm names do not resolve to the repositories this ADR reviewed.**

| Name as used above | Repository this ADR reviewed | What `pi install npm:<name>` actually installs |
| --- | --- | --- |
| `pi-subagents` | `amosblomqvist/pi-subagents` — no LICENSE file, no `package.json` | `pi-subagents@0.62.0` → `nicobailon/pi-subagents`, MIT |
| `pi-observational-memory` | `amosblomqvist/pi-observational-memory` — MIT, not published to npm | `pi-observational-memory@3.0.4` → `elpapi42/pi-observational-memory`, MIT |

Verified with `npm view <name> repository.url`. An installer following this ADR literally would install a different, larger project than the one reviewed — in both cases the stronger of the two, but not the one the review covered. Separately, the `@mariozechner/*` scope was renamed to `@earendil-works/*` and `@mariozechner/pi-coding-agent` is frozen at 0.73.1, so any package still importing that scope cannot load against the pinned 0.84.4 runtime without a port.

**2. “Overlapping packages can create conflicting authorities if installed carelessly” is listed above as a cost. It is an undetectable failure mode.**

Pi 0.84.4 resolves competing extension handlers silently by load order: for a `session_before` event every handler’s result overwrites the previous one, so two extensions that both answer `session_before_compact` do not conflict — the last one loaded wins, with no error and no warning. Pi also exposes no runtime API for enumerating loaded extensions, so nothing can detect the collision from inside a session. Single authority per concern must therefore be an install-time assertion over the distribution’s own pinned manifest, not a documented convention. `harness/authorities.json` and the P1-AUTH conformance tests are that assertion.

See [`../pi-native/dependency-matrix.md`](../pi-native/dependency-matrix.md) for the full evaluation and [`../pi-distribution.md`](../pi-distribution.md) for the pinned foundation.
