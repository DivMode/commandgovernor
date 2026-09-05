# Third-Party Notices and Provenance

Command Governor is licensed under MIT. This notice records architecture
inspiration/research separately from the compiled or packaged dependency graph.

The repository contains no vendored or copied third-party implementation source
code. Under ADRs 0008–0010 Command Governor is **composition-first** and ships no
custom production code: the product is the pinned Prime Agent release plus the
pinned packages below, installed by their own package managers at bootstrap or
by Prime at startup. Research citation alone does not make a project a
dependency, and any source copy/adaptation requires file-level provenance plus
the applicable license/NOTICE obligations before distribution.

## Pinned product dependencies

Exact versions, hashes, repositories, licenses and owned concerns are in
`pins/pins.json`; this list is the notice.

| Component | Version | License | Source |
| --- | --- | --- | --- |
| Prime Agent (`prime-agent`, `@earendil-works/pi-agent-core`, `pi-ai`, `pi-tui` at Prime's line) | 0.9.1 | MIT (Mario Zechner 2025, Prime Intellect 2026) | <https://github.com/PrimeIntellect-ai/prime-agent> |
| `pi-tasks` | 0.2.5 | MIT | <https://github.com/nczz/pi-tasks> |
| `@gotgenes/pi-subagents` | 21.4.0 | MIT | <https://github.com/gotgenes/pi-packages> |
| `pi-pr-review` | 1.17.10 | MIT | <https://github.com/10ego/pi-pr-review> |

## Patch offered upstream, not shipped

`docs/upstream/2026-09-04-pi-oracle-prime-compat.patch` is a proposed change to
`fitchmultz/pi-oracle` (MIT) at tag `v0.7.20`, written for upstream submission.
It is not applied, vendored or distributed by Command Governor; it is recorded
so the provenance of the idea and the exact target revision are reviewable.

## Architecture / protocol references

### Tandem

- Project: `Maxmedawar/tandem` and `DivMode/tandem`
- License: MIT
- Upstream revision re-verified: `a98bcafd2c40ae5473b85fe41183e4f391933799`
- DivMode main re-verified: `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee`
- DivMode PR #6 head re-verified: `af568233e1aae2d4cc343b38ca0e2a1a248e7857`
- URL: <https://github.com/Maxmedawar/tandem>
- URL: <https://github.com/DivMode/tandem>

Concepts studied include runtime/Herdr adaptation, ownership/provenance, MCP
orchestration, native Claude lifecycle, stale-client behavior, completion barriers,
and turn/session fencing.

No Tandem author or maintainer is implied to endorse Command Governor.

### codex-chatgpt-web

- Project: `miuuyy/codex-chatgpt-web`
- License: MIT
- Initial architecture review revision: `d7675fc7767a8f19b908f3e5d0e357699d1d9fdf`
- Current main re-verified at review completion:
  `06637f97a68faaa636986dad7514c7e2b3449347`
- Architecture document blob at current main:
  `4367828fae8ad0a53e4adb0af19c1589640cb37c`
- Release reviewed/re-verified: `v4.0.7`
- URL: <https://github.com/miuuyy/codex-chatgpt-web>

Concepts studied include exact browser-surface ownership, retained ChatGPT
conversation lifecycles, connector identity/schema compatibility, send/settlement
boundaries, reconnect without replay, and compaction handoff.

No codex-chatgpt-web author or maintainer is implied to endorse Command Governor.

### CCCC

- Project: `ChesterRa/CCCC`
- License: Apache-2.0
- Revision re-verified: `5f0b83242d09c88b1e2267d1056fc5bf64feb626`
- URL: <https://github.com/ChesterRa/CCCC>

Command Governor studied CCCC primarily as a protocol/semantics reference,
including append-only daemon authority and documented delivery states such as
`claimed`, `accepted`, `failed`, and `ambiguous`. The pre-ADR-0008 Rust design
planned an independent implementation; after the Pi-family pivot, the semantics
remain useful conformance requirements but no CCCC source is copied.

If Apache-2.0 source is copied or adapted later, the repository must preserve all
required license/NOTICE attribution and record exact files, revisions, and changes
here before distribution.

No CCCC author or maintainer is implied to endorse Command Governor.

## Durable-orchestration implementation references — no code copied

### Salvor

- Project: `joseym/salvor`
- License: Apache-2.0
- Revision reviewed: `dd9eb49f6bf854dc1c96b1b1ad7accbc509807b0`
- URL: <https://github.com/joseym/salvor>

Concepts studied include pure event replay, explicit external-effect classes,
write-ahead tool intent, dangling-write reconciliation, idempotency-key handling,
and deterministic kill/failpoint tests.

### Prime Agent

- Project: `PrimeIntellect-ai/prime-agent`
- License: MIT
- Earlier durable-orchestration review revision:
  `9f5edc192cfe3d4737205a2f551d2b6b6e34fe09`
- Substrate bake-off stable release: `v0.8.1`
- Stable release commit: `514633727bf26d74f39f3119c2b0e31a5ceb2a9d`
- URL: <https://github.com/PrimeIntellect-ai/prime-agent>

Concepts studied include mutating-command identity, write-ahead command journaling,
completed-result replay, uncertain-result no-replay, generation-aware cursors,
supervisor/worker recovery, process-safe session leases, RLM/persistent kernel
state, recursive subagents, schedules/goals/heartbeats, Agent Skills, and ACP.

Proposed ADR 0009 selects this stable release as the **initial substrate candidate**
subject to real-machine conformance. Selection as a dependency must be reflected in
the actual dependency/component manifest when the implementation lands.

### Agent Orchestrator

- Project: `ralphkrauss/agent-orchestrator`
- License: MIT
- Revision reviewed: `8b2f3b967e90877c3abac07061dbb2b1e67d2035`
- URL: <https://github.com/ralphkrauss/agent-orchestrator>

Concepts studied include daemon-owned orchestration truth, short-lived structured
orchestrator/reviewer turns, durable notification reconciliation, explicit
notification ACK, request-ID idempotency, and thin MCP/IPC/CLI transports.

No author or maintainer of these projects is implied to endorse Command Governor.
No implementation source from them is currently vendored/copied.

## Pi-family session / memory / substrate references — no code copied yet

These projects were first reviewed under ADR 0007. ADR 0008 superseded the earlier
strategy of independently recreating their mechanisms in Rust: Command Governor now
prefers pinned composition when a package meets the required contract.

### upstream Pi

- Project: `earendil-works/pi`
- License: MIT
- Substrate bake-off stable release: `v0.84.4`
- Stable release commit: `b79e4cc834970cca69daebffab7df1da7d1e52c4`
- URL: <https://github.com/earendil-works/pi>

Concepts/capabilities studied include provider abstraction, agent core, persistent
sessions, branching/tree/fork, compaction, RPC/JSON modes, extension loading,
Agent Skills, telemetry, supply-chain hardening, and external sandbox integration.

### pi-config

- Project: `amosblomqvist/pi-config`
- Revision reviewed: `f82da563ab05d66729492d64c7ed4e96db3663f3`
- URL: <https://github.com/amosblomqvist/pi-config>

Concepts studied include session analytics, role-specific extensions/agents,
prompt-pattern mining, interactive subagents, and observational memory.

### pi-interactive-subagents

- Project: `amosblomqvist/pi-interactive-subagents`
- License: MIT
- Revision reviewed: `c3e8b53c0754ae5ccc19fdab5a7481ec039bc2f7`
- URL: <https://github.com/amosblomqvist/pi-interactive-subagents>

Concepts studied include persistent logical child addressability, immutable
resolved loadout snapshots across resume, lineage-only/fork relationships,
recursive delegation allowlists, result steering, parked input, and activity/stall
observations.

### pi-observational-memory

- Project: `amosblomqvist/pi-observational-memory`
- License: MIT
- Revision reviewed: `78a1efcfdd46332253fb289724f05b26dfc7769e`
- URL: <https://github.com/amosblomqvist/pi-observational-memory>

Concepts studied include fixed source chunks, observer workers, coverage
watermarks, deterministic compaction rendering, bounded active memory, serialized
consolidation, fork seeding, and observer/consolidator cost accounting.

### pi-dictate and learn

- `amosblomqvist/pi-dictate`, revision
  `3208b563e3adfd070ac7b256a09ba9fc7b869f50`, MIT — operator UX research.
- `amosblomqvist/learn`, revision
  `7cfd8942f82ab9476e63572387e1fe9bcea5082c` — specialist
  researcher/visual-agent composition research.

### Oh My Pi

- Project: `can1357/oh-my-pi`
- License: MIT
- Substrate bake-off stable release: `v18.0.11`
- Stable release commit: `b8ce33a58911c26bed1d84f0db9a5e2e727c49a2`
- URL: <https://github.com/can1357/oh-my-pi>

Concepts studied include hashline/content-hash editing, LSP/DAP integration,
persistent Python/Bun execution, typed subagents, Agent Hub steering, advisor and
review roles, approval tiers, rule-on-violation injection, memory backends, virtual
resource namespaces, and ACP. Proposed ADR 0009 treats OMP as a tooling/UX research
donor and possible interoperable worker rather than the primary substrate.

No Pi-family implementation source is currently vendored/copied into Command
Governor by the architecture documents alone. Any package actually composed into
the product must be added to the machine-readable component/dependency manifest
with its exact pin and license.

## Agent protocol / portable-skill references — no code copied

### Agent Client Protocol TypeScript SDK

- Project: `agentclientprotocol/typescript-sdk`
- License: Apache-2.0
- URL: <https://github.com/agentclientprotocol/typescript-sdk>

The official SDK is reviewed for stable ACP v1 client/agent interoperability,
permission requests, session updates, and protocol extension behavior. Its current
README explicitly labels ACP v2 experimental/draft. Proposed ADR 0009 therefore
targets stable v1 first.

### Goose ACP reference

- Project: `aaif-goose/goose`
- Main revision observed during bake-off:
  `4ad43df42d8e6f5c9dae962d4cf4cbad2aadf3de`
- URL: <https://github.com/aaif-goose/goose>

Goose was reviewed as independent evidence that ACP can serve as a unifying client
boundary and as an agent-provider bridge.

### Agent Skills

- Project: `agentskills/agentskills`
- URL: <https://github.com/agentskills/agentskills>

Reviewed for the portable skill format and progressive-disclosure model. Any
executable skill remains a software dependency subject to Command Governor's
admission policy.

## Harness / agent-component security references — no code copied

### Harness Eval

- Project: `redhat-community-ai-tools/harness-eval`
- License: Apache-2.0
- URL: <https://github.com/redhat-community-ai-tools/harness-eval>

Reviewed for deterministic harness/config linting, cross-component graphs,
credential/confused-deputy analysis, skill verification, MCP/hook/agent checks, and
CI gating patterns.

### Snyk Agent Scan

- Project: `snyk/agent-scan` (successor/current repository for earlier MCP-scan
  lineage)
- URL: <https://github.com/snyk/agent-scan>

Reviewed for component discovery and skill/MCP/agent risk scanning. Its own
security warning notes that inspecting configured stdio MCP servers may execute
their commands; Command Governor therefore treats scanning untrusted executable
MCP configuration as a sandboxed action.

## Additional research references — no code copied

These projects were studied to understand current ChatGPT Web browser/private-API
tradeoffs. They are not currently implementation dependencies or copied sources:

- OpenWeb (`imoonkey/openweb`), revision
  `a387b50c829d871839a613732e1b97bfa1946124`
- `Octo-Lex/ChatGPT-Web2API`, revision
  `497527dceabfa3f95961e23c291e618c5570f1ac`
- `stufently/gpt-web-gateway`, revision
  `efb01a32e9e4c7fbebb8acff204c8c2a448c476c`

Rust browser alternatives examined but not copied/depended on yet:

- `mattsse/chromiumoxide` main
  `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`
- `rust-headless-chrome/rust-headless-chrome` main/release
  `0a5c307a85debc450378a1f19e4dac1838d7b22d` (`1.0.22`)
- `tauri-apps/wry` dev
  `bb69d628a905d65042c71a95e85f6921ec9b3264`
- `tauri-apps/cef-rs` dev
  `a2e15ae659c4b3957883e34de879bd8b38360ce5`

See the research documents under `docs/research/` for the architecture evidence and
exact adoption/rejection reasoning.

## Existing Rust dependencies

The frozen Phase-1 Rust scaffold still has resolved Rust dependencies. Their exact
versions are recorded in committed `Cargo.lock`; every one is a published crates.io
release and none is vendored/copied into this repository.

They remain vetted by this repository's `cargo-deny` policy in `deny.toml`, which
CI runs as `cargo deny --all-features check`:

- **Licenses** must appear on an explicit permissive allowlist (MIT, MIT-0,
  Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC,
  Zlib, CC0-1.0, Unicode-3.0, Unlicense).
- **Sources** are restricted to crates.io; unknown registries and unknown git
  sources are denied.
- Known-malicious versions from the August 2026 crates.io compromise are banned by
  exact version — `arrayref@0.3.10`, `internment@0.8.7`, and
  `append-only-vec@0.1.9`.
- RUSTSEC advisories are enforced with an empty ignore list; yanked crates are
  denied and wildcard version requirements are denied except for path
  dependencies.

The official Rust MCP SDK was re-verified at main
`ad9832ec212baf526e1a69d73ee04cd8305ae331`, workspace version `3.1.4`; that is
historical/research context, not a current commitment to the old mandatory-MCP
architecture.

A dependency manifest or generated license report does not replace this provenance
record when source/patterns are materially copied or adapted.