# Pi-native Command Governor harness research review — 2026-09-01

Status: **architecture input for ADR 0008; research/composition decision, no third-party implementation source copied into this repository by this document**.

## Executive conclusion

Command Governor should stop developing a parallel general-purpose agent runtime and instead become a **custom Pi harness/distribution**: a curated, tested set of Pi packages, extensions, skills, agent roles, policies, prompts, and a small amount of Command-Governor-specific glue.

The product name and canonical repository remain **Command Governor** / `DivMode/commandgovernor`. Pi is the substrate, not the product name. The existing `DivMode/pi-config` fork remains a research/reference input rather than the final product repository.

This is a reversal of the implementation strategy recorded on 2026-08-31. The original architecture correctly identified hard reliability requirements—durable owed work, exact correlation, restart recovery, independent review, user-owned decisions, and the rule that lossy memory/compaction cannot become lifecycle truth—but it assumed those mechanisms should be independently reimplemented in a Rust daemon. The Pi ecosystem now demonstrates enough of the session, extension, subagent, memory, continual-harness, supervision, and ChatGPT-Web machinery that duplicating the entire stack would create more maintenance risk than it removes.

The new strategy is therefore:

1. **ADOPT Pi as the runtime/harness substrate.**
2. **COMPOSE existing Pi packages before building replacements.**
3. **EXTEND Pi only where Command Governor has a real unmet invariant.**
4. **CONTRIBUTE missing generic primitives upstream where practical.**
5. **KEEP the Command Governor reliability contract as product behavior, not as a requirement for a separate Rust control-plane process.**
6. **DO NOT require MCP as the architecture spine** if Pi can complete a closed-loop exchange with the exact ChatGPT Web foreman conversation directly.

## What changed since the 2026-08-31 architecture

ADR 0007 and its research note explicitly said that Pi was useful only as a source of patterns and that Command Governor should independently implement those patterns in Rust. That conclusion was reasonable before the broader Pi ecosystem was reviewed.

The additional evidence changes the build-vs-compose decision:

- Pi itself exposes a deep extension system with session, agent, turn, message, tool, retry, compaction, and settlement events.
- Pi sessions are already persistent, resumable, forkable, branch-aware, and compactable.
- mature/community packages already implement asynchronous and interactive subagents, session lineage, observer/consolidator memory, durable local tasks, process supervision, verification, hooks, reviewed memory, cross-session tooling, and continual harness refinement;
- `pi-oracle` already targets an explicitly supplied existing `https://chatgpt.com/c/<id>` thread through the real web app;
- `pi-gpt` exposes ChatGPT account/chat/conversation/message operations as Pi tools through an undocumented ChatGPT web backend;
- Prime Intellect's Prime Agent demonstrates a serious production/research harness built by evolving Pi into a persistent, self-improving long-running system rather than recreating every coding-agent primitive from scratch;
- the `Continual Harness` and ACE research directions are already being translated into Pi-native packages;
- the user's own `DivMode/pi-config` fork already captures the exact Pi configuration family previously identified as useful.

The strategic question is therefore no longer “can Command Governor copy these ideas?” It is “what remains uniquely Command Governor after adopting the best existing Pi mechanisms?”

## Sources reviewed

### Pi core and ecosystem

| Project | Reviewed revision/version | Relevance |
| --- | --- | --- |
| `earendil-works/pi` | `3fc3ef532b966b28b764af070d62302c0acab0d5` (main, 2026-09-01) | core runtime, sessions, RPC, extensions, events, compaction, provider abstraction |
| `DivMode/pi-config` | fork of `amosblomqvist/pi-config`; upstream head `f82da563ab05d66729492d64c7ed4e96db3663f3` | our retained research fork; configuration/extension map |
| `amosblomqvist/pi-interactive-subagents` | `c3e8b53c0754ae5ccc19fdab5a7481ec039bc2f7` | async interactive children, resume/addressability, input routing, loadout/session patterns |
| `amosblomqvist/pi-subagents` | current public main reviewed 2026-09-01 | minimal isolated subagents, role/tool allowlists, bounded recursive delegation |
| `amosblomqvist/pi-observational-memory` | `78a1efcfdd46332253fb289724f05b26dfc7769e` | observer/consolidator memory, branch-local ledger, deterministic compaction, durable topic memory |
| `pungggi/pi-continual-harness` | `e697c8e01624b0a3d35b3d322319266f205e044b` | evidence-backed structured CRUD refinement over prompt/memory/skills/subagent specs; ACE-inspired bounded injection |
| `geminixiang/pi-stuff` | `dbfb5946997303c2cda2efbb36230b61a09f677e` | Pi-native task protocol, crash-reconciled supervisor, verification, hooks, memory, agent-team work |
| `fitchmultz/pi-oracle` | `b26f56106fb362b849fdb55dc14d9bc6fb9b28d1` | existing ChatGPT Web thread targeting, isolated browser auth/runtime, durable job results |
| `pi-gpt` | npm/package gallery `0.4.2`, published 2026-07-27 | direct ChatGPT account/chat tools, continuation by conversation id, conversation/message reads |
| `PrimeIntellect-ai/prime-agent` | `173d845a56f654fac8d82a6e47aec7a644207114` | evidence that Pi-derived architecture can support persistent goals, subagents, daemon continuity, continual harness and long-running work |
| `HazAT/pi-config` | current public main reviewed 2026-09-01 | ecosystem signal: migrated away from homegrown orchestration to Solo-native subagents/scratchpads/todos |

Primary URLs:

- <https://github.com/earendil-works/pi>
- <https://github.com/DivMode/pi-config>
- <https://github.com/amosblomqvist/pi-config>
- <https://github.com/amosblomqvist/pi-interactive-subagents>
- <https://github.com/amosblomqvist/pi-subagents>
- <https://github.com/amosblomqvist/pi-observational-memory>
- <https://github.com/pungggi/pi-continual-harness>
- <https://github.com/geminixiang/pi-stuff>
- <https://github.com/fitchmultz/pi-oracle>
- <https://pi.dev/packages/pi-gpt>
- <https://github.com/PrimeIntellect-ai/prime-agent>
- <https://github.com/HazAT/pi-config>

### Research

1. **Agentic Context Engineering: Evolving Contexts for Self-Improving Language Models** — Qizheng Zhang et al., arXiv:2510.04618. ACE identifies brevity bias and context collapse in repeated monolithic rewrites and instead treats context as an evolving playbook updated through structured incremental generation/reflection/curation.
   <https://arxiv.org/abs/2510.04618>
2. **Continual Harness: Online Adaptation for Self-Improving Foundation Agents** — Seth Karten et al., arXiv:2605.09998. It formalizes reset-free online refinement of prompt, subagents, skills, and memory from ongoing trajectories.
   <https://arxiv.org/abs/2605.09998>
3. **Agent Memory: Characterization and System Implications of Stateful Long-Horizon Workloads** — Yasmine Omri et al., arXiv:2606.06448. Stanford-affiliated systems work characterizing ten agent-memory systems and separating construction/retrieval/generation cost.
   <https://arxiv.org/abs/2606.06448>
4. **MemoryArena: Benchmarking Agent Memory in Interdependent Multi-Session Agentic Tasks** — Zexue He et al., arXiv:2602.16313 / Stanford Digital Economy Lab. It shows that recall benchmarks do not adequately measure whether remembered information drives correct later action.
   <https://arxiv.org/abs/2602.16313>
   <https://digitaleconomy.stanford.edu/publication/memoryarena-benchmarking-agent-memory-in-interdependent-multi-session-agentic-tasks/>

These papers are architecture/evaluation evidence, not product specifications.

## Pi core is already the layer we were about to build

Pi's `AgentSession` is shared across interactive, print, and RPC modes and already owns state access, event subscription with automatic session persistence, model/thinking configuration, compaction, bash execution, session switching, and branching.

Its extension API can subscribe to and influence the exact lifecycle seams Command Governor needs, including:

- `session_start`, `session_before_switch`, `session_before_fork`, `session_shutdown`;
- `session_before_compact`, `session_compact`, tree/branch events;
- `before_agent_start`, `agent_start`, `agent_end`, and the stronger `agent_settled`;
- turn/message/tool start-update-end events;
- retries and compaction retries through RPC/event streams;
- extension-defined tools, commands, shortcuts, flags, UI, and resource paths.

`agent_settled` is particularly important: it is emitted only after a full session-level run is settled and no automatic retry, compaction retry, or queued continuation remains. That is a better generic harness seam than maintaining one provider-specific interpretation per worker backend.

Pi's built-in session commands (`/resume`, `/fork`, `/clone`, `/tree`, `/compact`, `/export`, `/import`) and automatic JSONL persistence mean Command Governor should not invent a competing session file format unless an exact missing requirement is first demonstrated.

## The user's Pi fork already captures the right configuration family

`DivMode/pi-config` is a real fork of `amosblomqvist/pi-config`. Its upstream README explicitly says it is not intended to be installed as one monolithic package; users should copy the pieces they need. It points to:

- `pi-interactive-subagents`;
- `pi-observational-memory`;
- `pi-dictate`;
- the `learn` specialist-agent system;
- session-analysis utilities;
- ask-user, bash-guard, browser, prompt-snippet, web-fetch and web-search extensions.

That repo should remain a source/research fork. Command Governor should productize selected capabilities under its own tested package/distribution rather than becoming a lightly renamed personal configuration dump.

## Existing subagent work substantially replaces Governor worker/runtime adapters

The ecosystem already provides multiple subagent models with different tradeoffs.

`amosblomqvist/pi-subagents` demonstrates a minimal model: isolated Pi subprocesses, tool allowlists, role definitions, bounded concurrent fan-out, and explicit recursive delegation allowlists. `pi-interactive-subagents` adds long-lived interactive workers, persistent addressing/resume patterns, child input routing, status/watchdog behavior, and result steering back to the parent.

Other Pi packages add more durable process/task behavior. The important architecture conclusion is not that one package is automatically the final choice; it is that Command Governor no longer needs bespoke `governor-worker-claude`, `governor-worker-codex`, Herdr lifecycle interpretation, provider hook parsers, and a parallel worker-session abstraction merely to get these capabilities.

Command Governor should define conformance requirements and select/compose the best Pi-native implementation behind those requirements.

## Existing memory work substantially replaces a custom Governor memory subsystem

`pi-observational-memory` already implements the strongest mechanisms identified in ADR 0007:

```text
raw chunks
  -> parallel observer Pi subprocesses
  -> atomic observations + coverage watermark
  -> branch-local ledger
  -> deterministic/model-free compaction rendering
  -> serialized consolidator
  -> durable per-session topic memory + journey
```

This matches several recommendations from the Stanford memory characterization: background construction, bounded concurrency, explicit construction cost, freshness/coverage, and bounded consolidation.

Command Governor should compose or fork this package only when a specific missing invariant is demonstrated. It should not independently rebuild the same observer/consolidator architecture in Rust.

## ACE and Continual Harness already have Pi-native implementations

The ACE paper's core warning—brevity bias and context collapse under repeated prose rewrites—is directly addressed by `pi-continual-harness`. The package stores prompt notes, memory, skill descriptions, and subagent specs as structured state and applies evidence-backed item-level create/update/delete deltas rather than replacing one giant prompt. Its current 0.8.x line also performs bounded, importance-ordered injection instead of dumping all stored state into every turn.

Prime Agent independently validates the architectural direction at larger scale: it began as a Pi-derived system and now combines persistent sessions, recursive subagents, durable harness state, persistent goals, direct agent-to-agent communication, heartbeats/schedules, daemon-backed continuity, and continual refinement.

Command Governor should treat these as evidence to **compose and specialize**, not as a reason to create a third incompatible self-improvement/memory implementation.

## Durable tasks and helper daemons can remain Pi-native

Moving completely to Pi does **not** mean every byte of durability must exist inside one interactive Pi process.

For example, `@geminixiang/pi-task-protocol` defines versioned task lifecycle states, sequenced events, output cursors, and acknowledgements; `@geminixiang/pi-supervisor` implements a crash-reconciled local task daemon with atomic snapshots, append-only event spools, process-identity validation, bounded output reads, and orphan handling.

The architectural distinction is:

- **Rejected:** a separate Command Governor general-purpose orchestration runtime that duplicates Pi's agent/session/provider ecosystem.
- **Allowed:** Pi-native extensions that use purpose-built helper processes/daemons when required for crash survival, supervision, indexing, or browser work.

Those helpers are implementation details of the Command Governor Pi harness, not a competing orchestration authority.

## ChatGPT Web changes the foreman architecture

The original design assumed Command Governor had to wake ChatGPT Web and then expose MCP mutation tools back to ChatGPT so the foreman could claim, resume, ACK, and answer worker input.

The Pi ecosystem opens a simpler closed loop.

### Candidate A — `pi-gpt`: direct ChatGPT web-backend transport

`pi-gpt` 0.4.2 exposes Pi tools for account status, model listing, starting/continuing chats, listing chats, reading a conversation, and reading a message. Its documentation says it reuses the user's ChatGPT/Codex login and can continue a conversation by `conversation_id`.

Potential advantages for Command Governor:

- no DOM selector dependency for the ordinary path;
- direct conversation/message identities;
- streaming/reconciliation opportunities;
- lower browser/process overhead;
- Pi can both send the foreman event and read the resulting foreman response.

Open questions that require live conformance proof:

- can it adopt the exact pre-existing browser-created ChatGPT foreman conversation, or only reliably continue conversations it initiated/discovered itself?;
- are message/conversation identities strong enough to reconcile an interrupted send without blind replay?;
- does the resulting turn preserve every foreman capability Command Governor actually needs?;
- how stable is the undocumented interface across ChatGPT product changes?;
- what compatibility/terms risk must be documented for an open-source product?

No product contract should depend on bypassing provider security controls. An undocumented transport may be supported only as an empirically gated adapter with clear compatibility/risk labeling.

### Candidate B — `pi-oracle`: real ChatGPT Web thread transport

`pi-oracle` already supports explicitly supplying a raw ChatGPT conversation id or `https://chatgpt.com/c/<id>` URL. It normalizes the thread, acquires a same-conversation lease, opens it in an isolated authenticated browser runtime, performs the web submission, and durably stores job state/results. Its documentation intentionally treats wake-up back into Pi as best-effort while preserving the result on disk.

This is strong fallback/reference evidence for exact existing-thread support.

### Consequence — MCP is optional, not architectural

If Pi can:

1. send a correlated foreman event to the exact ChatGPT Web conversation;
2. wait/read the resulting ChatGPT response;
3. validate task/revision/action structure;
4. durably record the disposition before routing it to workers;

then ChatGPT does **not** need to call a Command Governor MCP server merely to return an ACK or revision instruction.

The conceptual loop becomes:

```text
Pi worker/subagent finishes
  -> durable Command Governor foreman event
  -> Pi ChatGPT-Web transport sends event to exact /c/<id>
  -> ChatGPT Web foreman reviews/reasons
  -> Pi transport reads correlated response
  -> durable validated disposition
  -> ACK closes work | REVISE resumes worker | DELEGATE spawns role | ASK_USER waits
```

MCP remains useful as an optional interoperability surface, but it is no longer the mandatory spine of Command Governor.

## What Command Governor becomes

Command Governor remains the product name and canonical repository, but its product definition changes to:

> **Command Governor is a curated Pi-native software-engineering harness for durable, multi-agent, foreman-led work. It combines selected Pi packages with Command-Governor-specific extensions, roles, policies, prompts, memory, verification, analytics, and ChatGPT Web foreman integration.**

A likely repository shape is:

```text
commandgovernor/
  package.json / Pi package manifest
  extensions/
    foreman/              # exact ChatGPT Web closed loop + correlation/recovery
    obligations/          # only if existing Pi task packages do not satisfy semantics
    lifecycle/            # normalize Pi events into Governor task/attention states
    policy/               # high-risk/user-owned action gates
    analytics/            # session/cost/cache/subagent/memory visibility
  agents/
    planner/
    scout/
    researcher/
    implementer/
    reviewer/
  skills/
    github/
    verification/
    architecture-review/
    release-review/
  prompts/
  config/
  docs/
```

This is illustrative, not a commitment to build every listed extension. Existing packages are preferred wherever they satisfy the acceptance contract.

## Build-vs-compose matrix

| Capability | New default | Command Governor action |
| --- | --- | --- |
| base agent loop | Pi | ADOPT |
| provider/model abstraction | Pi | ADOPT |
| session persistence/resume/fork/tree | Pi | ADOPT |
| compaction hooks | Pi | ADOPT; customize only where evidence requires |
| subagents | existing Pi packages | COMPOSE/SELECT; do not rebuild first |
| interactive child questions | existing Pi subagent/intercom patterns | COMPOSE |
| observational memory | `pi-observational-memory` / alternatives | COMPOSE, benchmark |
| ACE/continual refinement | `pi-continual-harness`, Prime Agent patterns | COMPOSE, benchmark |
| process supervision | Pi-native supervisor/task packages | COMPOSE |
| verification hooks | existing Pi verification/hooks packages | COMPOSE |
| session/cost/cache analytics | Pi usage/session data + extensions | COMPOSE/EXTEND |
| ChatGPT Web exact-thread browser fallback | `pi-oracle` | ADOPT/ADAPT candidate |
| ChatGPT Web direct transport | `pi-gpt` | PRIMARY EXPERIMENT, capability-gated |
| MCP | Pi MCP packages or small adapter | OPTIONAL INTEROP ONLY |
| exact foreman event/reply correlation | not yet proven complete | BUILD only missing glue |
| Governor-specific owed-work semantics | reuse task packages if possible | EXTEND only if gap remains |
| user-owned high-risk decision policy | harness policy extension | BUILD/COMPOSE |
| GitHub durable engineering truth | GitHub tools/skills | COMPOSE |

## Reliability invariants that survive the move

Moving to Pi changes implementation, not the product's reliability goals.

Command Governor should preserve these invariants as executable conformance tests:

1. **Delegated work does not disappear because a Pi session, terminal, subagent, helper, browser, or ChatGPT turn restarts.**
2. **Worker completion is not the same fact as foreman processing.** If the product promises foreman review, the task remains open until a correlated foreman disposition is durably recorded.
3. **Every foreman event and reply carries exact task/revision correlation.** A stale reply cannot close newer work.
4. **An ambiguous external send is never blindly replayed.** Reconcile first; if proof is insufficient, expose attention rather than duplicate an external effect.
5. **Lossy memory and model-generated compaction never become authority for exact lifecycle, capability, safety, or user-owned decisions.**
6. **Subagent roles/loadouts are explicit and least-authority.** Resume may not silently broaden an old worker because today's defaults changed.
7. **High-risk/destructive/credential-sensitive/materially broader decisions remain user-owned unless the user has explicitly delegated that authority.**
8. **Independent review remains independent.** An implementer cannot self-certify work merely because the same harness can run both roles.
9. **GitHub remains the durable engineering source of truth for issues/commits/PRs/reviews where the workflow is GitHub-backed.**

The implementation should use Pi-native state and adopted packages to satisfy these tests; it should not assume a Rust daemon is the only way to satisfy them.

## Why cache efficiency matters but is not the primary decision

Pi surfaces input/output/cache-read/cache-write usage and its architecture is intentionally small and extensible. Stable prefixes and a compact harness can improve provider prompt-cache reuse and reduce fresh-input traffic.

However, “cache hit rate” alone is not a sufficient architecture metric. A harness can report a higher percentage while still sending far more total tokens. Command Governor should measure:

- task correctness;
- fresh input tokens;
- cached input tokens;
- total input/output/reasoning tokens;
- cache-write traffic where exposed;
- wall-clock latency;
- actual cost;
- context growth/compaction frequency;
- subagent and memory-worker share.

The Pi move is justified primarily by ecosystem reuse, extension depth, provider portability, and maintenance leverage. Cache efficiency is an additional benefit to benchmark, not a religious requirement.

## Why a Pi-native distribution is preferable to an immediate hard fork

Command Governor should initially depend on/pin upstream Pi and package-level extensions rather than hard-forking Pi core.

Benefits:

- inherit rapid upstream fixes and provider support;
- reduce merge debt;
- make third-party Pi packages easier to consume;
- keep Command Governor's differentiated code small and auditable;
- allow generic missing hooks to be contributed upstream;
- permit independent pin/upgrade/conformance testing.

A downstream Pi core fork is allowed only when a required primitive cannot be implemented safely through the public extension/SDK/RPC surfaces and an upstream fix is unavailable on the required timeline. Any such fork must document the exact delta and an exit/upstream plan.

## Role of `DivMode/pi-config`

`DivMode/pi-config` should be kept as a research/reference fork and potentially as a test fixture/source of cherry-pickable configuration ideas. It should not become the canonical Command Governor product repository.

Reasons:

- upstream explicitly frames it as a personal config to browse/copy from;
- Command Governor needs its own versioned compatibility matrix, acceptance tests, provenance, upgrade policy, and product defaults;
- we should be able to replace one third-party extension without inheriting the entire personal config;
- the Command Governor name/domain/repository remain the durable public identity.

## Migration direction

The existing Rust Phase-1 scaffold should be **frozen, not expanded**, while the Pi-native parity spike runs.

Recommended sequence:

1. Pin a known Pi release/revision and create the initial Command Governor Pi package/distribution skeleton.
2. Import only reviewed configuration/agent/skill concepts; do not bulk-copy the personal `pi-config` tree.
3. Select one subagent implementation and prove spawn, concurrent fan-out, input wait, resume, parent restart, child result recovery, and role/loadout restrictions.
4. Select memory/continual-harness packages and run MemoryArena-style dependent-session tests plus repeated-compaction constraint-preservation tests.
5. Build the minimal `foreman` extension around a structured event/reply envelope.
6. Try the direct `pi-gpt` closed loop first; use `pi-oracle` as exact-thread browser fallback/reference.
7. Crash-test every boundary: before send, during send, after provider acceptance, after response arrival, before durable disposition, and before worker resume/closure.
8. Only after Pi-native conformance passes, archive/remove the now-redundant Rust runtime crates and update README/roadmap/install docs.

## Required foreman spike

A move to Pi should not be declared end-to-end complete until this exact experiment passes:

```text
existing user-created ChatGPT Web /c/<id>
  <- Pi sends EVENT(task_id, revision, unique_delivery_id)
  -> ChatGPT produces structured FOREMAN_ACTION
  <- Pi reads the exact response
  -> Pi validates task/revision/action
  -> Pi durably records disposition
  -> Pi resumes/closes/delegates/waits exactly once
```

Required failure injections:

- Pi process exits before send;
- transport disconnects during send;
- provider accepts but local acknowledgement is lost;
- Pi exits after ChatGPT response but before local application;
- duplicate/stale ChatGPT reply arrives;
- same task gets a newer revision before an old reply is processed;
- parent/subagent/helper restarts;
- memory/observer state is stale;
- user-owned decision is requested.

A browser or direct-backend transport is an adapter detail. The tests define the product behavior.

## Risks and mitigations

### Fast-moving upstream

Pi is moving quickly. Pin versions/commits, maintain a tested compatibility matrix, use lockfiles, and run an upgrade conformance suite before bumping Pi or critical extensions.

### Extension quality varies

Popularity is not proof. Every adopted package needs source/license/security review plus behavioral acceptance tests. Prefer small, inspectable, actively maintained packages and upstream contributions over opaque glue.

### Too many overlapping plugins

Do not install multiple packages that own the same authority/state unless the integration is explicitly designed. Command Governor should expose one curated default stack and treat alternatives as tested profiles.

### Private ChatGPT Web transport drift

Treat direct/private ChatGPT interfaces as capability-gated and replaceable. Do not place the rest of the harness behind transport-specific assumptions. Keep a supported/fallback path where practical and fail closed on ambiguous delivery.

### Memory becomes accidental authority

Keep exact task/revision/capability/user-decision records in deterministic structured state. Observational and continual memory may advise but not silently rewrite those facts.

### “Pi-native” becomes a disguised second framework

Set a strict rule: before creating a new Command Governor extension, document why Pi core plus reviewed ecosystem packages cannot satisfy the requirement. The burden of proof is on new infrastructure.

## Recommendation

Adopt the Pi-native architecture now, while Command Governor's live Rust service adapters have not yet hardened.

The Command Governor differentiation should be **curation + integration + durable foreman-led software-engineering behavior + strong conformance tests**, not ownership of another provider/session/subagent/memory/runtime stack.

The most important next implementation work is therefore not another Rust adapter. It is a Pi-native Command Governor skeleton and the end-to-end ChatGPT Web foreman spike.
