# ADR 0009: Select Prime Agent as the initial substrate and ACP v1 as the public agent-client boundary

- **Status:** Accepted — 2026-09-04, on the evidence in [`../research/2026-09-04-zero-custom-code-proof.md`](../research/2026-09-04-zero-custom-code-proof.md); see "Acceptance record" at the end
- **Date:** 2026-09-01
- **Supersedes/refines:** ADR 0008's initial upstream-Pi substrate direction
- **Research:** [`../research/2026-09-01-agent-harness-landscape-and-substrate-bakeoff.md`](../research/2026-09-01-agent-harness-landscape-and-substrate-bakeoff.md)

## Context

ADR 0008 correctly changed Command Governor from a planned standalone Rust agent runtime into a Pi-native/compatible harness distribution. It made the implementation strategy composition-first and retained the old Rust code only as a migration/conformance oracle.

A broader follow-up review found that two Pi-derived runtimes have already internalized major pieces Command Governor would otherwise need to integrate itself:

- **Prime Agent** — detached supervisor/resident workers, process-safe session leases, durable mutation journals, uncertain-effect non-replay, generation-aware reconnect, RLM children, schedules, goals, heartbeats, continual harness, Agent Skills, and stable ACP mode;
- **Oh My Pi (OMP)** — highly optimized editing/tool surfaces, LSP/DAP, hashline edits, typed subagents, advisor/review, approval tiers, browser/Python, memory backends, and ACP.

The source-level bake-off compared upstream Pi, Prime Agent, and OMP using Command Governor-specific criteria. Prime Agent is the strongest fit for the hardest Governor requirement: **durable long-running worker/session behavior across client, supervisor, worker, and external-effect failures**.

Separately, Agent Client Protocol (ACP) has matured into a portable client-to-agent boundary. Prime Agent and OMP ship ACP servers, Goose increasingly uses ACP as a unifying client/agent protocol, and the official TypeScript SDK maintains stable ACP v1 while ACP v2 remains explicitly experimental.

The architecture should distinguish three layers:

1. **runtime durability** — internal Prime Agent worker/supervisor/session authority;
2. **public agent-client interoperability** — ACP v1;
3. **Command Governor semantics** — policies, review, ChatGPT foreman, curation, conformance, and security defaults layered above the substrate.

## Decision

### 1. Prime Agent is the initial Command Governor substrate

Subject to Gate S0/S1 below, Command Governor will use:

```text
Prime Agent v0.8.1
commit 514633727bf26d74f39f3119c2b0e31a5ceb2a9d
```

as the first pinned runtime substrate.

Command Governor remains the product name, `DivMode/commandgovernor` remains the canonical repository, and `commandgovernor.com` remains the public identity. This does not rename Command Governor to Prime Agent or turn the repository into an upstream mirror.

Prime Agent is a Pi-derived implementation substrate selected because it already contains durable runtime mechanisms Command Governor would otherwise have to own.

### 2. The selection is conditional on real-machine conformance

The current research environment could inspect source/releases but could not execute external release artifacts because direct external downloads/network resolution are blocked.

Prime Agent is therefore the **selected candidate**, but the production pin is not final until Gate S0 and the critical parts of S1 run on the real supported macOS machine in disposable state roots.

If Prime Agent fails materially, fallback order is:

1. upstream Pi `v0.84.4` plus the minimum durable helper packages required;
2. OMP only if runtime evidence proves suitable durability and its integrated-fork maintenance cost is acceptable;
3. re-open substrate selection rather than silently weakening Governor semantics.

### 3. Reuse Prime Agent's durable runtime instead of building a second Governor runtime

Command Governor should depend on or wrap Prime Agent's documented long-running mechanisms rather than recreate them as parallel authorities:

- detached supervisor;
- resident root workers;
- session-path leases;
- worker generations and replay cursors;
- coherent recovery snapshots;
- command IDs and mutation journals;
- explicit uncertain mutation state;
- uncertain-effect non-replay;
- per-session schedules and claim-before-delivery behavior;
- child-agent registry and recovery;
- persistent kernel/RLM state.

Command Governor may add product-specific durable records where Prime Agent lacks a required concept, but it must not recreate an entire competing worker/session control plane by default. A new authoritative store requires proof that the relevant state cannot be represented safely through Prime Agent plus a narrow Governor sidecar.

### 4. Stable ACP v1 is the public agent-client interoperability boundary

Command Governor will target **stable ACP v1** for portable client-to-agent interactions where ACP fits.

ACP is preferred for initialization/capability negotiation, client-driven sessions, prompt/stream interaction, cancellation, tool activity updates, permission requests, editor/client interoperability, and evaluation-harness driving.

Where Command Governor exposes an agent endpoint, an ACP-compatible path should be preferred over a proprietary duplicate unless a required capability cannot be expressed safely.

### 5. ACP is not the internal durability authority

ACP does not replace Prime Agent's daemon protocol or Command Governor's exact reliability semantics.

Resident worker adoption, session lease ownership, durable command journaling, uncertain mutation recovery, scheduler claims, worker-generation reconciliation, descendant/goal/heartbeat state, and crash-recovery bookkeeping remain internal runtime concerns unless standardized later.

```text
external client / editor / evaluation harness
                │
              ACP v1
                │
      Command Governor integration
                │
      Prime Agent runtime APIs
                │
 detached supervisor / resident workers
```

### 6. Namespaced ACP metadata is allowed for additive Governor state

Prime Agent's stable ACP implementation places non-standard features under a reverse-domain `_meta` namespace while preserving standard ACP objects. Command Governor may follow the same pattern for additive metadata generic clients may safely ignore.

Conceptually:

```text
_meta["com.commandgovernor"]
```

Rules:

- never add Governor-specific fields to standard ACP roots;
- version Governor metadata independently;
- generic clients must remain safe if they ignore it;
- correctness-critical state must not exist only as ephemeral ACP metadata.

### 7. ACP v2 remains experimental

The official ACP TypeScript SDK currently describes ACP v2 as a draft whose wire protocol/API may change incompatibly.

Therefore V1 targets stable ACP v1. V2 experiments must be capability-gated, v2 wire state is not an irreversible product contract, and no V1 acceptance gate requires v2-only features. Migration to stable v2 requires a future compatibility decision.

### 8. Upstream Pi remains an architectural upstream and fallback

Selecting Prime Agent does not erase upstream Pi. Command Governor should continue to track Pi for provider/agent-core evolution, extension and skill patterns, session/compaction improvements, telemetry, upstream fixes, and an exit/fallback path if Prime Agent diverges incompatibly.

Governor-specific packages should avoid unnecessary dependence on private Prime internals where a portable Pi/Agent-Skills/ACP surface suffices.

### 9. OMP is a tooling/UX research donor and optional interoperable worker

OMP's strongest capabilities should be evaluated individually rather than adopting its entire integrated fork.

High-value bake-off lanes include hashline/content-hash edits, LSP/semantic-code operations, DAP/debugging, typed subagent results, advisor/reviewer models, rule-on-violation injection, approval tiers, virtual resource namespaces, and optimized read/search/tool formats.

Prefer portable implementations on the Prime substrate when practical. OMP itself may later be usable as an ACP-compatible specialized worker.

### 10. Agent Skills is the preferred portable workflow/skill format

Command Governor will prefer the open Agent Skills format for reusable workflows and instruction packages where appropriate.

Progressive disclosure is a product requirement:

- startup receives small stable metadata;
- full instructions load only when selected;
- scripts/resources load only when needed;
- large installed catalogs are not dumped into every model request.

Executable skills are software dependencies and must pass the admission policy below.

### 11. Add Gate P0: component and harness security

Every default skill/extension/MCP/helper must record at minimum:

```text
name
source repository/package
exact version/revision
content/package hash where practical
license
runtime authority/capabilities
network/filesystem/process requirements
update policy
conformance tests
security scan/lint status
```

Before admission:

1. inspect source/license;
2. pin version/revision/hash;
3. run deterministic harness/config lint where applicable;
4. run agent-component/skill/MCP security scanning;
5. scan untrusted executable MCP configurations inside a sandbox;
6. declare component authority so overlapping lifecycle owners are rejected;
7. run sandboxed smoke/conformance tests;
8. require review for upgrades that change executable code/tool definitions.

Command Governor should produce a machine-readable **agent component manifest** analogous to an SBOM.

### 12. Sandboxing becomes a foundation requirement

Prime Agent, upstream Pi, and persistent RLM/Python execution normally run with the local user's OS permissions. Tool allowlists/approval prompts are not OS containment.

Command Governor must define execution profiles and use a real isolation mechanism for untrusted workloads. Initial candidates include approaches upstream Pi already recognizes such as Gondolin, Docker/container isolation, or OpenShell. The final macOS default is selected by Gate S3.

At minimum:

- untrusted repository -> sandbox required;
- downloaded/unreviewed executable skill -> sandbox required;
- untrusted MCP executable inspection -> sandbox required;
- credentials remain outside worker reach unless explicitly brokered;
- network policy is bounded for untrusted workloads where supported.

### 13. RLM is a measured context strategy

Prime Agent's persistent IPython/RLM model is a major reason to select it, but Command Governor will test it rather than assume every task benefits.

Governor Bench must compare ordinary and RLM-heavy workflows using correctness, fresh/cached/total input, output/reasoning tokens, latency, cost, tool/retry counts, and security exposure.

The persistent kernel is useful working state, not authority that may silently override exact lifecycle/policy/user facts.

### 14. Memory remains advisory and must win downstream-action tests

ADR 0007/0008 memory semantics remain. Selecting Prime Agent's continual harness/RLM does not automatically choose one memory system. Observational memory, continual-harness state, reflection/knowledge pages, and other strategies are evaluated by **correct later action**, not recall quality alone.

Exact control/safety/capability/user-owned facts remain deterministic and pinned outside lossy model memory.

### 15. Coding-tool improvements are experimental modules, not substrate reasons

Hashline editing, semantic code intelligence, advisor agents, and related techniques may materially improve coding quality. They should be admitted through Governor Bench lanes rather than by switching the entire runtime to OMP.

No tool becomes default solely on author-reported benchmark claims.

### 16. Independent review remains a product invariant

Prime Agent's subagent/RLM features supply execution mechanics; they do not weaken Governor's review separation.

```text
implementer
  -> durable result/evidence
  -> independent reviewer
  -> review evidence/verdict
  -> ChatGPT Web foreman
  -> ACK | REVISE | DELEGATE | ASK_USER
```

A worker cannot satisfy its own independent-review obligation by self-reporting success.

### 17. ChatGPT Web foreman remains a separate highest-risk gate

ACP selection does not solve consumer ChatGPT Web transport. ADR 0008's closed-loop requirement remains:

```text
worker result
  -> durable correlated foreman event
  -> exact ChatGPT Web /c/<id>
  -> foreman response
  -> correlated read-back
  -> durable disposition
```

Direct/private ChatGPT-Web transports remain replaceable/capability-gated, with browser-backed transport as a fallback candidate. The substrate must support this loop but does not define how ChatGPT is reached.

### 18. Governor Bench becomes a release/admission mechanism

Command Governor is expected to accumulate powerful optional components. Without measurement, plugin accumulation can make the harness slower, more expensive, less cacheable, or less correct.

Governor Bench compares the baseline substrate against candidate additions and records correctness/regressions, wall time, fresh/cache/total tokens, cost, tool calls/retries, crash recovery, and permission/security violations.

A component may remain optional even if impressive. Default status requires measured net value on representative workloads.

## Bake-off gates

### Gate S0 — real-machine substrate smoke

On the supported macOS machine, using disposable state/config/worktrees, verify the exact released pins:

- Pi `v0.84.4` — `b79e4cc834970cca69daebffab7df1da7d1e52c4`;
- Prime Agent `v0.8.1` — `514633727bf26d74f39f3119c2b0e31a5ceb2a9d`;
- OMP `v18.0.11` — `b8ce33a58911c26bed1d84f0db9a5e2e727c49a2`.

Verify release integrity/install, version/help, fresh session, resume, package/skill loading, state-root behavior, and clean shutdown. Record observed filesystem/process side effects.

**Acceptance:** Prime Agent has no blocking installation/runtime defect and its required stable features are present.

### Gate S1 — Prime durability conformance

Failure-inject client detach during work, supervisor death/replacement, worker death/recovery, concurrent session open, reconnect cursor/generation change, mutation admitted then connection lost before durable result, completed command retry, scheduled prompt crash window, and descendant/child recovery.

**Acceptance:** no uncertain external effect is blindly replayed; session authority is single-writer/fenced; recoverable state resumes coherently.

### Gate S2 — ACP v1 conformance

Drive Prime Agent using the official ACP SDK and verify initialize, session creation, prompt streaming, tool updates, cancel, permissions, close, and safe handling of unknown namespaced metadata.

**Acceptance:** a generic ACP client works without Prime/Governor-specific logic; Governor-aware metadata is additive.

### Gate S3 — sandbox/security baseline

Select and prove at least one supported isolation profile plus component admission.

**Acceptance:** an untrusted test workload cannot escape declared filesystem/network/credential policy under the selected sandbox; rejected tool/capability paths fail closed; component manifest is reproducible.

### Gate S4 — context/tool bake-off

Measure baseline Prime against RLM-heavy operation, progressive skill loading, hashline edits, semantic code intelligence, and selected memory lanes.

**Acceptance:** defaults are chosen from correctness + resource evidence, not cache percentage alone.

### Gate S5 — independent review

Prove implementer -> independent reviewer -> foreman disposition with evidence and restart-safe result identity.

### Gate S6 — ChatGPT Web foreman

Prove exact consumer-thread correlated round trip and ambiguity/restart recovery as required by ADR 0008.

## Relationship to ADR 0008

ADR 0008 remains accepted in its major decision: **Command Governor is a curated Pi-family harness/distribution rather than a parallel standalone Rust agent runtime.**

This ADR refines:

- initial `base harness = upstream Pi` -> **Prime Agent v0.8.1**, pending S0/S1;
- ACP v1 becomes the preferred public agent-client interoperability boundary;
- P0 component/sandbox security is added before plugin composition;
- OMP becomes a tooling donor/reference rather than a competing default substrate;
- progressive Agent Skills and Governor Bench become explicit architecture layers.

ADR 0008's product identity, Rust freeze/migration-oracle policy, durable-work/review/ambiguity/user-decision invariants, optional-MCP direction, and ChatGPT Web closed-loop requirement remain unchanged.

## Consequences

### Positive

- avoids rebuilding Governor's hardest durable-session mechanics on vanilla Pi;
- retains Pi lineage while gaining a production-oriented long-running runtime;
- provides a standard ACP boundary immediately;
- RLM, recursive agents, goals, schedules, and progressive Agent Skills are available without bespoke Governor implementations;
- preserves an exit path through ACP/Agent Skills/upstream Pi;
- separates runtime durability from coding-tool experimentation;
- adds a missing supply-chain/sandbox layer before the plugin ecosystem grows;
- makes performance/context claims measurable through Governor Bench.

### Costs / risks

- strategic dependency moves to a smaller, more opinionated Prime Agent project;
- persistent Python expands the code-execution attack surface;
- the substrate is not a sandbox and requires another execution-policy layer;
- some Pi community extensions may need adaptation;
- Prime-specific ACP metadata is non-standard even when namespaced;
- maintaining upstream Pi compatibility/exit paths requires discipline;
- source-level bake-off still needs real macOS crash/runtime proof;
- OMP may continue to outpace Prime on editing/IDE capabilities, requiring modular integration.

## Alternatives considered

### Keep upstream Pi as the direct base

Rejected as the initial choice after this bake-off. Pi is the cleanest/minimal substrate but would make Command Governor assemble or own the detached durable worker/session/ambiguity layer Prime Agent already ships. This remains the fallback if Prime fails S0/S1 or becomes an unacceptable dependency.

### Select Oh My Pi

Rejected as primary runtime. OMP is strongest in coding-tool/IDE integration and has ACP/permissions/subagents, but the reviewed stable source did not establish Prime-equivalent detached root-worker durability/journaling as its core contract. Adopt its strongest patterns modularly where benchmarks justify them.

### Use ACP as the whole Governor control plane

Rejected. ACP is a client-agent protocol, not a replacement for durable session leases, mutation journals, uncertain-effect reconciliation, or resident-worker recovery.

### Hard-fork Prime Agent immediately

Rejected. Start from a pinned upstream Prime release and use documented extension, skill, ACP, RPC, and host surfaces. If a core patch is unavoidable, attempt upstream contribution first and maintain a minimal documented delta/exit plan.

### Keep the old Rust Governor daemon beside Prime Agent

Rejected as a default architecture. The old code remains a conformance oracle until parity is proven. A narrow sidecar/helper is allowed only for a concrete missing product-specific invariant.

## Acceptance / transition

Move this ADR from **Proposed** to **Accepted** only after:

1. Gate S0 passes on the actual supported Mac;
2. critical ambiguity/session-owner portions of Gate S1 pass;
3. no license/distribution issue blocks shipping Prime Agent as the substrate;
4. the implementation foundation PR records the exact pin and a reproducible component manifest.

After acceptance, the next implementation PR should establish the pinned Prime Agent Command Governor distribution and ACP conformance harness. It should **not** start by porting OMP features, memory systems, or ChatGPT transport all at once.
## Acceptance record (2026-09-04)

Accepted on the real supported Mac with the conditions of the transition
section met as follows:

1. **Gate S0** passed for Prime `v0.9.1` (`81ae3cb34d27d38ee37f9e205a1e73694993b344`),
   the current release; the pin moved from 0.8.1 to 0.9.1 with verified
   assets and a regenerated lockfile (`pins/`).
2. **Gate S1 (critical portions)** passed through stock Prime clients:
   worker loss never duplicates an external effect on any stock surface,
   session authority converges on one worker per path, a dead resident root
   is reopened on the same `sessionId`, and a dead supervisor is replaced by
   a live worker. No Command Governor code was needed for any of it; the
   §3 rule ("do not recreate a competing worker/session control plane") is
   satisfied literally.
3. **License:** MIT; no distribution restriction.
4. **Foundation:** `pins/pins.json` is the reproducible component manifest
   (substrate, packages, concerns).

Refinements recorded at acceptance:

- §4–§7 (ACP): interoperability only; no shipped path needs it yet, and
  Prime has no ACP client, so driving another ACP agent is an upstream
  contribution, not a Command Governor layer.
- §11 (P0 admission): a package is admitted only after it is observed to
  register on the pinned Prime; extension load failures are silent in
  headless modes.
- §12 (sandboxing): refined by ADR 0010 §19 to optional hardening, with one
  sharpening from the proof: Prime has no permission system and its kernel
  executes shell commands below every extension hook, so OS containment of
  the kernel process is the only available control for destructive work.
- Gates S2–S6 are not prerequisites for the trusted-local product; S5
  (independent review) is satisfied by GitHub review as the acceptance
  record plus `pi-pr-review` and `--autonomous-gate`; S6 (ChatGPT foreman)
  waits on an upstream compatibility fix and user-side steps.
