# Agent harness landscape and Command Governor substrate bake-off — 2026-09-01

Status: **architecture research; source-level bake-off complete, real-machine runtime smoke still required**.

This review follows ADR 0008's decision to stop building Command Governor as a
parallel general-purpose Rust agent runtime and instead make it a curated,
high-powered Pi-family harness/distribution.

The immediate question is narrower and more consequential:

> **Which existing runtime should Command Governor actually use as its primary
> substrate, and which surrounding standards/projects should become first-class
> layers rather than Command-Governor-specific reinventions?**

The review compares three Pi-family substrate candidates in detail:

1. upstream **Pi**;
2. **Prime Agent**;
3. **Oh My Pi (OMP)**.

OpenCode and Goose are used as non-Pi reference baselines so the comparison does
not become circular. The review also evaluates Agent Client Protocol (ACP), Agent
Skills, sandbox/security tooling, RLM/context patterns, independent review, harness
linting, and code-intelligence/editing patterns that materially affect the final
Command Governor architecture.

## Executive conclusion

### Substrate

**Prime Agent v0.8.1 is the strongest initial substrate for Command Governor,
subject to one real-machine conformance gate before the choice is frozen.**

This is not because Prime Agent has the largest ecosystem or the most coding tools.
It wins because its **stable release already implements the failure semantics that
are hardest and most differentiated for Command Governor**:

- detached supervisor + resident worker processes;
- persisted sessions that continue after the UI disconnects;
- process-safe session leases;
- generation-aware replay cursors and recovery snapshots;
- durable command IDs and mutation journals;
- explicit `uncertain` mutation outcomes that are **not replayed**;
- crash recovery without replaying uncertain external effects;
- schedules claimed/advanced before prompt delivery;
- child-agent continuity;
- persistent Python/RLM state;
- goals, heartbeats, schedules, and bounded autonomous continuation;
- built-in stable ACP v1 mode;
- Agent Skills with progressive loading;
- a namespaced extension path for features that ACP itself does not model.

Those mechanisms strongly overlap the exact invariants Command Governor spent its
first architecture phase designing independently.

Upstream Pi remains the best **minimal/reference substrate** and the most important
ecosystem/upstream compatibility target. It has the cleanest extension model and
largest community, but choosing it directly would require Command Governor to
assemble or build the detached durability/control layer that Prime Agent already
ships.

Oh My Pi is the strongest **coding-tool/IDE research donor**: hashline edits,
LSP/DAP, typed subagent results, advisor review, approval tiers, browser/Python
surfaces, memory backends, and ACP are all highly relevant. It does not win the
primary substrate bake-off because the reviewed stable release does not expose the
same documented detached root-session/crash-recovery authority as Prime Agent, and
its much larger integrated fork creates a wider maintenance/authority surface.

### Protocol

**Use stable ACP v1 as Command Governor's public agent-client interoperability
boundary where it fits; do not make ACP the internal durability authority.**

Prime Agent's internal daemon protocol should continue to own worker/session
recovery. ACP is the portable client-facing surface for prompt/stream/cancel/tool
updates/permissions. Command-Governor-specific metadata can use the protocol's
namespaced `_meta` convention when a standard ACP client may safely ignore it.

ACP v2 is currently explicitly experimental/draft and is **not** a foundation
contract yet.

### Surrounding architecture

Command Governor should be treated as a layered distribution rather than merely
"Pi plus plugins":

```text
Command Governor
  ├── runtime substrate        Prime Agent (initial choice; Pi-derived)
  ├── client protocol          ACP v1 + richer internal/local APIs where needed
  ├── skill/workflow format    Agent Skills + progressive disclosure
  ├── security plane           sandbox + plugin/skill/MCP admission + hashes
  ├── context plane            RLM state + bounded/typed memory + lazy loading
  ├── coding intelligence      hashline/LSP/semantic-code techniques
  ├── review plane             independent reviewer / evidence-based QA
  ├── ChatGPT foreman          exact-thread closed-loop transport
  └── Governor Bench           correctness/durability/security/token evaluation
```

This is a refinement of ADR 0008, not a return to the old standalone Governor
runtime.

---

## Research method

The bake-off uses **released stable tags where possible**, not screenshots or
moving-main claims.

Reviewed pins:

| Candidate | Version / revision | Role |
| --- | --- | --- |
| upstream Pi | `v0.84.4` / `b79e4cc834970cca69daebffab7df1da7d1e52c4` | substrate candidate |
| Prime Agent | `v0.8.1` / `514633727bf26d74f39f3119c2b0e31a5ceb2a9d` | substrate candidate |
| Oh My Pi | `v18.0.11` / `b8ce33a58911c26bed1d84f0db9a5e2e727c49a2` | substrate candidate |
| Goose | main observed at `4ad43df42d8e6f5c9dae962d4cf4cbad2aadf3de` | ACP/reference baseline |
| OpenCode | current `dev` documentation reviewed | product/reference baseline |

Primary repositories:

- <https://github.com/earendil-works/pi>
- <https://github.com/PrimeIntellect-ai/prime-agent>
- <https://github.com/can1357/oh-my-pi>
- <https://github.com/aaif-goose/goose>
- <https://github.com/anomalyco/opencode>
- <https://github.com/agentclientprotocol/typescript-sdk>

### Important limitation: no release-binary execution in this research environment

A release-artifact smoke test was attempted in an isolated temporary directory,
but this execution environment blocks direct external release downloads/network
resolution. Therefore this document **does not claim that the three released
binaries were executed here**.

The substrate decision is consequently based on source/release-tag architecture
and tests/documentation. A short real-machine **Gate S0** is mandatory before the
Prime Agent selection becomes the irreversible production pin.

This limitation is intentionally visible because a source-level architecture
review cannot prove packaging, process launch, macOS behavior, or local
installation compatibility.

---

## Command Governor requirements used for scoring

The candidates are not scored as generic coding assistants. They are scored for
**Command Governor's actual product**.

Weights:

| Criterion | Weight | Why it matters |
| --- | ---: | --- |
| durable lifecycle / crash recovery | 22 | the core Governor problem |
| extensibility / composability | 14 | Governor must remain a distribution, not an unmaintainable fork |
| provider + session foundation | 8 | baseline harness competence |
| subagents / multi-agent control | 10 | implementation/research/review workflows |
| context / RLM / memory architecture | 12 | long-running work and token efficiency |
| coding-tool quality / code intelligence | 10 | real engineering correctness |
| protocol / interoperability | 8 | avoid proprietary client lock-in |
| security / permissions / sandbox integration | 8 | third-party plugin + autonomous execution risk |
| maintenance / supply-chain posture | 5 | long-term operability |
| ecosystem / exit cost | 3 | ability to move or reuse components |

Scores below are **pre-runtime-bake-off architecture scores**, 0–5 per criterion.
They are not benchmark results.

| Candidate | Durability | Extensible | Sessions | Subagents | Context/RLM | Coding tools | Protocol | Security | Maintenance | Exit/ecosystem | Weighted |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| upstream Pi | 2.0 | 5.0 | 5.0 | 2.0 | 4.0 | 2.5 | 2.5 | 2.5 | 5.0 | 5.0 | **65.4** |
| Prime Agent | 5.0 | 4.0 | 4.5 | 5.0 | 5.0 | 3.0 | 5.0 | 2.0 | 4.0 | 3.5 | **85.7** |
| Oh My Pi | 2.5 | 4.0 | 5.0 | 4.5 | 4.0 | 5.0 | 5.0 | 3.5 | 3.0 | 3.0 | **77.2** |

The purpose of the numeric table is to expose weighting and disagreements. It is
not a claim of scientific precision. Gate S0/S1 can change the result.

---

# Candidate 1 — upstream Pi

## What Pi does exceptionally well

Upstream Pi is the cleanest general agent substrate reviewed.

The current project exposes separate packages for:

- unified multi-provider LLM API;
- agent core;
- coding-agent CLI;
- TUI;
- vendor-neutral telemetry.

Its coding-agent layer provides persistent JSONL sessions, resume/fork/tree,
branching, compaction, RPC/JSON modes, context files, skills, and a deep extension
surface. The surrounding ecosystem has rapidly produced subagents, memory,
supervisors, ChatGPT Web transports, MCP bridges, voice, browsers, and specialized
workflows without requiring changes to Pi core.

Pi also has a strong supply-chain posture in its own repository: exact external
dependency pins, lock/shrinkwrap controls, ignored lifecycle scripts where
possible, release smoke tests, and dependency/audit checks.

The ecosystem/community size is a major advantage. The GitHub repository was
observed on 2026-09-01 at roughly 100k stars and more than 12k forks. Popularity is
not a correctness criterion, but it reduces abandonment and integration risk.

## The gap that matters for Command Governor

Pi explicitly does **not** provide a built-in filesystem/process/network/credential
permission sandbox. It recommends external/container isolation such as Gondolin,
Docker, or OpenShell.

More importantly for Governor, the reviewed upstream stable architecture does not
provide a first-class detached supervisor/resident-worker system with the same
published semantics Prime Agent already has for:

- process-safe active-session leases;
- supervisor replacement/adoption;
- durable mutation journals;
- generation-aware reconnect cursors;
- uncertain-effect quarantine;
- retained detached root workers.

Those can be built from Pi packages/extensions/helper daemons, but **that would make
Command Governor the integrator/owner of the hardest lifecycle layer again**.
That is exactly the duplication ADR 0008 was intended to avoid.

## Verdict

**KEEP AS UPSTREAM REFERENCE / FALLBACK / ECOSYSTEM SOURCE.**

Do not choose vanilla Pi as the primary production substrate unless Prime Agent
fails Gate S0/S1 or creates unacceptable fork/maintenance constraints.

---

# Candidate 2 — Prime Agent

## Pi lineage without rebuilding long-running durability

Prime Agent explicitly acknowledges Pi as its agent/TUI foundation, but builds a
long-running RLM harness around it.

Its stable `v0.8.1` architecture is unusually well aligned with Command Governor.

### Detached process topology

The stable release documents:

```text
clients
  <-> detached supervisor
       -> catalog process
       -> resident worker A -> root session + scheduler + kernel + descendants
       -> resident worker B -> root session + scheduler + kernel + descendants
```

Closing the TUI detaches rather than killing a resident worker. Workers can adopt a
replacement supervisor. A worker crash is scoped to one root tree.

This directly addresses the failure mode that originally motivated Command
Governor: UI/client lifetime is not worker lifetime.

### Process-safe session ownership

Every persisted session is protected by a process-safe lease keyed to its canonical
JSONL path. A worker must acquire the target before opening it, and concurrent
opens return the owning active-session identity rather than allowing two writers.

This is much stronger than treating a session name, pane, or process as authority.

### Durable command identity + ambiguity semantics

This is the most consequential finding in the bake-off.

Prime Agent's stable daemon protocol uses stable client/command IDs and journals
mutating commands **before dispatch**. The documented behavior is:

- repeat of a completed command -> return stored result;
- command received without durable result -> report **uncertain**;
- uncertain mutation -> **do not replay** merely because the connection/process
  restarted;
- reconnect retains the same command ID;
- completed journal records can be acknowledged/compacted.

This is essentially the same external-effect ambiguity class Command Governor's
old Rust architecture independently designed.

### Generation-aware event recovery

Clients use `{ generation, sequence }` cursors. A new worker generation invalidates
old sequence comparisons. Missing replay can recover from a coherent snapshot,
and stale/duplicate generation events are ignored.

Again, this maps directly to Governor's binding/incarnation/replay concerns.

### Schedules use claim-before-delivery semantics

Per-session schedules are persisted. A due tick is claimed and advanced before
prompt delivery. A crash therefore does not replay an uncertain scheduled prompt.

That is the right pattern for Governor's future reminders/heartbeats/autonomous
continuation as well.

### RLM/context model

Prime Agent's stable RLM architecture uses a persistent IPython kernel as the
model-facing control environment. Python variables/imports/functions/task handles
survive tool calls and compaction, while authoritative provider/session/scheduling
operations remain in the TypeScript host via typed host requests.

This creates a useful separation:

```text
LLM context = reasoning/navigation
persistent Python = working state / programmatic composition
host runtime = authoritative lifecycle/external effects
```

Native `rlm(...)` calls create real child AgentSessions, and child registry state
survives compaction, kernel restart, and parent restoration.

### Progressive skills

Prime Agent supports the Agent Skills format. Only skill metadata is included at
startup; full instructions are loaded on match. Python-backed skills can expose
programmatic APIs without forcing every capability schema/instruction into every
model request.

### ACP in stable release

`prime-agent --mode acp` exposes stable ACP over NDJSON/stdin/stdout. It supports
prompting, streaming, cancellation, tool activity, and client-supplied session MCP
servers.

Prime-specific features that ACP does not model are carried under a namespaced
`_meta` object rather than altering ACP root schemas. This is exactly the extension
pattern Command Governor should prefer.

## Important weaknesses

Prime Agent is not a sandbox. Its own documentation says the worker/kernel execute
model-generated Python and project commands with the user's operating-system
permissions.

The persistent Python control surface is both a major capability and an enlarged
attack surface. Command Governor cannot ship it as a trusted-by-default execution
boundary for untrusted repositories/skills.

Prime Agent also has a smaller ecosystem than upstream Pi and is necessarily more
opinionated. Choosing it increases dependency on Prime Intellect's fork decisions.
The escape hatch must remain tested: Command Governor should keep portable skills,
ACP compatibility, and a small product-specific extension layer rather than
forking Prime Agent immediately.

## Verdict

**SELECT AS INITIAL PRIMARY SUBSTRATE, CONDITIONAL ON GATE S0/S1.**

Pin `v0.8.1` / `514633727bf26d74f39f3119c2b0e31a5ceb2a9d` for the first reproducible
Command Governor spike. Do not follow moving `main` silently.

---

# Candidate 3 — Oh My Pi

## The strongest engineering tool surface

OMP is the most aggressively integrated coding-agent runtime reviewed.

Its stable `v18.0.11` ships or documents:

- 60+ model providers;
- native/in-process search/shell utilities;
- LSP code intelligence;
- DAP debugging;
- persistent Python and Bun execution;
- real browser tooling;
- first-class subagents in isolated worktrees;
- typed/schema-validated child results;
- live Agent Hub steering/revive/kill controls;
- advisor/reviewer model running beside the doer;
- parallel `/review` workflows;
- hashline/content-hash editing with stale-source rejection;
- memory backends;
- GitHub/resource virtual filesystems;
- rule injection triggered on violations rather than loading every rule every
  turn;
- ACP server mode.

Several of these should influence Command Governor regardless of substrate.

### Hashline edits are especially relevant

A content-hash editing protocol is naturally multi-agent-safe: an edit anchored to
stale source is rejected rather than silently applying to changed content. That is
a useful optimistic-concurrency property for Governor's parallel workers.

### Approval model

OMP's stable release has explicit `read | write | exec` tool tiers plus
`allow | deny | prompt` policies, argument-sensitive tool decisions, and ACP client
permission routing. Unknown/malformed custom tool approvals default to `exec`,
which is a sensible fail-conservative classification.

However, OMP's ordinary configured default is `yolo`, so Command Governor would
need to ship stricter policy rather than inherit defaults blindly.

### ACP

OMP provides a built-in ACP server and its own behavior-compatible implementation
of the ACP SDK surface. That proves the architecture is practical, although using a
local reimplementation rather than the official SDK adds protocol-maintenance
surface.

## Why OMP does not win the substrate bake-off

The source/tag review did not locate a documented stable detached root-session
supervisor with Prime Agent's combination of:

- resident root workers surviving client disconnect;
- process-safe session leases;
- generation-aware replay cursors;
- command journals with explicit uncertain non-replay;
- supervisor replacement/adoption.

OMP has daemons for individual facilities and strong interactive/subagent tooling,
but its strongest differentiation is **tool quality and IDE semantics**, not the
long-running durable session authority that Governor most needs.

OMP is also a much larger integrated fork. The repository has an extensive native
Rust/TypeScript surface and a high open-issue count. Selecting it would reduce the
amount of plugin composition we need, but it would simultaneously increase how
much fork-specific behavior Governor inherits.

## Verdict

**DO NOT USE AS PRIMARY SUBSTRATE TODAY. USE AS A TOOLING/UX RESEARCH DONOR AND
OPTIONAL ACP-COMPATIBLE WORKER.**

Specifically evaluate/adapt independently:

- hashline editing;
- LSP/semantic code intelligence;
- advisor/reviewer pattern;
- typed child results;
- rule-on-violation injection;
- approval tiers;
- virtual resource namespaces.

Do not fork/copy large OMP internals into Command Governor merely to obtain these
patterns.

---

# Non-Pi controls: OpenCode and Goose

These projects were reviewed to ensure the recommendation was not predetermined by
Pi lineage.

## OpenCode

OpenCode is highly productized, multi-provider, cross-platform, and already has
separate plan/build agents plus a general subagent. It is an important usability
and distribution benchmark.

For Command Governor, its main weakness is fit: the product is broader/more
application-like than the minimal programmable harness layer we want to curate,
and the reviewed public overview does not present Prime Agent's durable detached
root/control semantics as its central abstraction.

Use OpenCode as a **UX/product/cache reference baseline**, not the initial Governor
runtime.

## Goose

Goose is important primarily because of its protocol architecture. It supports ACP
as a client/server boundary and can use ACP agents as providers. Current Goose work
is explicitly converging clients around ACP so terminal/desktop/IDE integrations do
not each need a proprietary agent bridge.

This independently supports the decision to give Command Governor an ACP-compatible
public boundary even when its internal runtime is Prime Agent.

---

# Protocol bake-off — ACP vs Pi/Prime-native control

## What ACP should own

Stable ACP v1 is a standardized code-editor/client-to-agent protocol. The official
TypeScript SDK defines agent and client roles around initialization, sessions,
prompting, updates, and permission requests. Prime Agent and OMP both already
implement ACP server modes; Goose uses ACP extensively.

Command Governor should use ACP for portable external operations such as:

- initialize/capability discovery;
- create a client-driven session;
- prompt/stream;
- cancel;
- surface tool activity;
- permission interaction;
- editor/client interoperability.

## What ACP should **not** own

ACP is not a substitute for Prime Agent's durable worker authority.

Do not force these through standard ACP when the runtime already has a richer,
durable internal mechanism:

- supervisor replacement;
- resident-worker adoption;
- process-safe session leases;
- operation journals;
- uncertain mutation quarantine;
- scheduled-job claims;
- internal child registry;
- exact crash-recovery projections.

Use the runtime's local/daemon API for these, and bridge relevant public state to
ACP.

## ACP extensions

Prime Agent's stable pattern is appropriate: standard clients receive standard ACP;
additional subagent/gate/goal state travels in a reverse-domain `_meta` namespace.
A client that does not understand Command Governor metadata should remain safe and
functional.

A possible namespace is conceptually:

```text
_meta["com.commandgovernor"]
```

The exact schema is future work and must be versioned if introduced.

## ACP v2

The official TypeScript SDK currently states ACP v2 is experimental/draft and may
change incompatibly. Command Governor therefore:

- targets stable ACP v1 first;
- may prototype v2 behind a capability flag;
- does not persist v2 wire objects as irreversible product state;
- does not make v2-only behavior a V1 acceptance requirement.

---

# The surrounding ecosystem we were missing

The substrate bake-off uncovered several **layers**, not merely more agent
runtimes.

## 1. Portable Agent Skills and progressive disclosure

The Agent Skills project provides a lightweight open format for reusable agent
capabilities. The important systems property is progressive loading: discovery can
carry only metadata, with full instructions/resources loaded when needed.

Prime Agent already follows this model.

### Command Governor direction

- Prefer Agent Skills for portable instruction/workflow packages.
- Keep startup prompt content small and stable.
- Load full skill content only when selected.
- Treat executable skill code as an installable software dependency, not trusted
  prose.
- Keep Command-Governor-specific policy outside a skill when it must remain exact
  lifecycle authority.

This should become a first-class cache/context rule.

## 2. Harness configuration security

`redhat-community-ai-tools/harness-eval` is a linter for the **agent setup**, not
source code. It currently advertises more than one hundred deterministic rules and
cross-component analysis for credential-exfiltration chains, confused-deputy
flows, skill/hook conflicts, MCP configuration, and token-budget problems.

This is directly relevant to a curated distribution with third-party plugins.

### Command Governor direction

Add a **Gate P0 / component-admission phase** before any extension/skill becomes a
default:

```text
candidate component
  -> exact source/version/hash/license
  -> harness static lint
  -> security scan
  -> declared authority/capabilities
  -> sandbox smoke
  -> Governor conformance tests
  -> admission manifest
```

## 3. Agent/MCP/skill security scanners

The project formerly known around MCP scanning has evolved into Snyk Agent Scan.
It discovers agent components, skills, and MCP servers and checks prompt injection,
tool poisoning/shadowing, destructive capabilities, suspicious code, credentials,
and other agent-specific risks.

Important caution from its own documentation: scanning an untrusted stdio MCP
configuration can itself execute the configured command. Therefore **the scanner
must also run in a sandbox for untrusted inputs**.

Command Governor should not depend on one vendor scanner, but it should adopt the
security model:

- component inventory;
- exact hashes;
- tool-definition fingerprints where available;
- rescan after upgrades;
- quarantine on definition drift;
- no silent MCP/skill update into a trusted profile.

## 4. Sandboxing is a foundation, not a plugin nicety

Upstream Pi, Prime Agent, and the Prime RLM kernel all execute with user
permissions unless placed in an external isolation boundary. OMP has a richer
approval system but approval is not OS containment.

Pi itself recommends external isolation patterns including Gondolin, Docker, and
OpenShell.

### Command Governor direction

The supported distribution should make sandbox policy explicit by workload class:

- trusted local repository: owner-approved local mode may be allowed;
- unknown/untrusted repository or downloaded skill: sandbox required;
- scanner evaluating an untrusted MCP executable: sandbox required;
- high-risk browser/network workflows: network allowlist/policy required where
  practical.

Do not claim a same-user worker process is sandboxed merely because it has a tool
allowlist.

## 5. RLMs as context architecture

Prime Agent turns RLM from an add-on into its core programming model. Independent
Pi RLM implementations show the idea is broader than one project.

The relevant architectural question is not "does Python make the agent smarter?"
It is:

> Can a persistent external working environment keep bulky state, handles, parsed
> data, and recursive work outside the conversation so the model receives less
> repeated context?

That should be measured in Governor Bench against ordinary tool-loop Pi/Prime
operation.

Metrics must include fresh input, cached input, total input, output/reasoning,
wall-time, and correctness. A high cache-hit percentage alone is not the objective.

## 6. Code intelligence can be decoupled from the substrate

OMP's LSP/DAP integration is strong, but choosing OMP merely to obtain semantic
code intelligence would be an architectural mistake.

Framework-agnostic projects such as Serena expose symbol/reference-aware code
operations separately. Command Governor can therefore evaluate:

- native OMP LSP behavior as a benchmark;
- Serena/another semantic-code service with Prime Agent;
- simpler grep/read baseline.

Choose by measured task correctness/token cost.

## 7. Hash-anchored editing

OMP's hashline approach and independent Pi implementations demonstrate a useful
multi-agent invariant:

> an edit must fail when the source anchors no longer match the content the agent
> actually read.

That is effectively optimistic concurrency for source edits. It deserves a
Governor Bench lane even if OMP is not the runtime substrate.

## 8. Independent QA/reviewer workflows

Projects such as Harnessed implement an isolated QA stage where reviewers do not
inherit the implementer's self-assessment and require concrete evidence.

This aligns with an existing Command Governor invariant:

> implementer completion is not independent review.

The design should be adapted as a workflow/role contract, not necessarily adopted
as a runtime dependency.

## 9. Spec-driven engineering workflows

GitHub Spec Kit and portable software-development skill collections provide
structured specify/clarify/plan/tasks/implement/review workflows across multiple
agents.

Command Governor should prefer these portable process definitions where good
rather than creating a proprietary command for every software-engineering method.

## 10. Harness engineering needs experiments

The ecosystem is beginning to treat tool schemas, middleware, memory, permissions,
and prompt/context layout as experimental variables rather than folklore.

Command Governor should formalize this with **Governor Bench**:

```text
baseline substrate
vs + hashline
vs + semantic code intelligence
vs + memory strategy
vs + RLM mode
vs + lazy tool/skill discovery
vs + advisor/reviewer
```

Measure:

- task correctness;
- regression rate;
- fresh input tokens;
- cached input tokens;
- total input tokens;
- output/reasoning tokens;
- cost;
- wall-clock time;
- tool calls/retries;
- crash recovery;
- unsafe/unauthorized actions.

No plugin should become a default because it sounds powerful.

---

# Recommended Command Governor stack after source-level bake-off

```text
Command Governor
│
├── substrate
│   └── Prime Agent v0.8.1 (initial pin after S0)
│       └── Pi-derived agent/provider/session lineage
│
├── runtime durability
│   └── Prime Agent detached supervisor / worker / journal / lease model
│
├── public client protocol
│   └── ACP v1
│       └── namespaced Governor metadata only where necessary
│
├── internal control
│   └── Prime Agent daemon/RPC/host APIs
│
├── context
│   ├── RLM persistent working state
│   ├── Agent Skills progressive loading
│   ├── bounded exact control facts
│   └── separately evaluated advisory memory
│
├── engineering tools (bake-off lanes)
│   ├── hashline editing
│   ├── semantic/LSP code intelligence
│   ├── browser/web tools
│   └── GitHub evidence tooling
│
├── security
│   ├── sandbox profiles
│   ├── permission/capability policy
│   ├── component manifest + hashes
│   ├── harness-eval style lint
│   └── skill/MCP security scan + drift detection
│
├── workflow
│   ├── implementer
│   ├── independent reviewer
│   ├── researcher/scout
│   └── ChatGPT Web foreman
│
└── Governor Bench
    └── correctness + durability + security + context/cost evaluation
```

This is a **distribution architecture**. It does not require Command Governor to
rename itself Prime Agent or Pi.

---

# Required bake-off gates

## Gate S0 — real-machine substrate smoke

Run on the actual supported macOS development machine in disposable state roots.
Pin and verify:

- Pi `v0.84.4` / `b79e4cc834970cca69daebffab7df1da7d1e52c4`;
- Prime Agent `v0.8.1` / `514633727bf26d74f39f3119c2b0e31a5ceb2a9d`;
- OMP `v18.0.11` / `b8ce33a58911c26bed1d84f0db9a5e2e727c49a2`.

At minimum verify install/checksum path, `--version`, help/mode discovery, fresh
session creation, resume, extension/skill loading, and no writes outside the
disposable test root except documented state/config locations.

Prime Agent is selected only if this gate passes.

## Gate S1 — durable lifecycle / ambiguity

For Prime Agent:

- detach client while worker runs;
- replace/kill supervisor and verify worker adoption;
- crash one worker and verify scoped recovery;
- attempt concurrent session open and verify lease rejection;
- interrupt a mutating daemon command after admission and verify it becomes
  uncertain rather than being replayed;
- retry completed command ID and verify stored result/idempotence;
- exercise generation/sequence reconnect;
- verify scheduled prompt claim-before-delivery behavior.

Translate relevant old Rust Governor failure-injection tests rather than trusting
documentation alone.

## Gate S2 — ACP v1 conformance

Using the official ACP SDK/client:

- initialize;
- session/new;
- prompt + streamed assistant/tool updates;
- cancel;
- permission request;
- session close;
- MCP session configuration if used;
- unknown `_meta` ignored by generic client;
- Governor-aware `_meta` parsed by our conformance client.

Do not require experimental ACP v2.

## Gate S3 — security baseline

Before model-driven mutation is considered supported:

- define trusted vs untrusted execution profiles;
- integrate at least one real sandbox route;
- pin component versions/hashes;
- inventory skills/extensions/MCPs;
- run harness static lint;
- run agent-component security scanning in a sandbox where the scan may execute an
  MCP server;
- verify denied tool/capability paths fail closed.

## Gate S4 — context/tool efficiency

Compare at least:

1. Prime Agent baseline;
2. RLM-heavy workflow;
3. hashline edit lane;
4. semantic-code lane;
5. progressive/lazy skill/tool loading;
6. selected memory strategy.

Correctness is primary. Report fresh/cache/total tokens separately.

## Gate S5 — independent review

Prove:

```text
implementer -> immutable/evidence result -> separate reviewer -> foreman disposition
```

Reviewer must not rely on implementer self-certification.

## Gate S6 — ChatGPT Web foreman

ADR 0008's highest-risk closed loop remains required:

```text
Prime/Pi worker result
  -> durable correlated foreman event
  -> exact consumer ChatGPT Web /c/<id>
  -> foreman action
  -> correlated read-back
  -> durable disposition
  -> ACK | REVISE | DELEGATE | ASK_USER
```

Transport remains capability-gated (`pi-gpt`-style direct candidate, browser-backed
fallback, or another proven mechanism). ACP does not by itself solve ChatGPT Web.

---

# Migration consequences for the current repository

1. **Do not resume feature work on the old Rust daemon.** ADR 0008's freeze remains.
2. **Do not delete the Rust tests/specs yet.** They are valuable failure oracles for
   Gate S1/S6.
3. **Do not build a vanilla-Pi foundation PR until Gate S0 runs.** The substrate
   candidate changed after this broader landscape review.
4. After S0/S1 confirm Prime Agent, create one focused foundation PR that pins the
   substrate and establishes Command Governor packaging/config/compatibility tests.
5. Port old Governor invariants into substrate-neutral conformance tests before
   deleting redundant Rust crates.
6. Keep the product repository `DivMode/commandgovernor`; do not turn it into a
   mirror/fork named `prime-agent`.
7. If Prime Agent later diverges incompatibly, ACP + Agent Skills + portable
   Governor policies/workflows provide an exit path back to upstream Pi or another
   ACP-capable harness.

---

# Open questions

The source-level decision does **not** yet answer:

- exact Prime Agent extension/package compatibility with the Pi ecosystem we want;
- whether Prime Agent's stable daemon behaves identically on the target macOS
  machine under repeated forced crashes;
- which sandbox gives the best macOS developer UX and containment;
- whether RLM execution improves our actual coding tasks enough to justify the
  added Python execution surface;
- which memory system wins downstream-action tests;
- whether ACP should expose any Governor-specific methods in the future or only
  metadata;
- whether the best ChatGPT Web transport can safely/reliably resume the exact
  consumer thread;
- whether OMP's hashline/LSP implementations are reusable as dependencies or only
  patterns to independently implement/integrate.

These questions should be answered by experiments, not by expanding the ADR with
assumptions.

---

# Source / provenance index

## Substrates

- Pi: <https://github.com/earendil-works/pi>
- Pi stable tag: <https://github.com/earendil-works/pi/tree/v0.84.4>
- Prime Agent: <https://github.com/PrimeIntellect-ai/prime-agent>
- Prime Agent stable daemon architecture:
  <https://github.com/PrimeIntellect-ai/prime-agent/blob/v0.8.1/packages/coding-agent/docs/daemon.md>
- Prime Agent stable ACP:
  <https://github.com/PrimeIntellect-ai/prime-agent/blob/v0.8.1/packages/coding-agent/docs/acp.md>
- Prime Agent stable RLM:
  <https://github.com/PrimeIntellect-ai/prime-agent/blob/v0.8.1/packages/coding-agent/docs/rlm.md>
- Oh My Pi: <https://github.com/can1357/oh-my-pi>
- OMP approval model:
  <https://github.com/can1357/oh-my-pi/blob/v18.0.11/docs/approval-mode.md>
- Goose: <https://github.com/aaif-goose/goose>
- OpenCode: <https://github.com/anomalyco/opencode>

## Protocol / skills

- ACP TypeScript SDK: <https://github.com/agentclientprotocol/typescript-sdk>
- ACP registry: <https://github.com/agentclientprotocol/registry>
- Agent Skills: <https://github.com/agentskills/agentskills>

## Harness / component security

- Harness Eval: <https://github.com/redhat-community-ai-tools/harness-eval>
- Snyk Agent Scan: <https://github.com/snyk/agent-scan>

## Related patterns to evaluate in implementation research

- Pi config / observational-memory lineage already reviewed in ADR 0007/0008
- Serena semantic code tools: <https://github.com/oraios/serena>
- GitHub Spec Kit: <https://github.com/github/spec-kit>
- Hindsight memory: <https://github.com/vectorize-io/hindsight>
- OpenShell: <https://github.com/NVIDIA/OpenShell>

No third-party implementation code is copied by this document. Any future vendoring,
reuse, or derived implementation requires an explicit license/provenance review.