# Pi package selection matrix

Every package evaluated for the Command Governor distribution, at the exact
revision it was read, with the concern it would own and the verdict.

Source: [`../research/2026-09-01-pi-package-evaluation.md`](../research/2026-09-01-pi-package-evaluation.md),
investigated 2026-09-01 against the `@earendil-works/pi-coding-agent@0.84.4`
pin. Every row was read from a clone or an unpacked tarball at the revision
named, not from a README.

**Nothing in this matrix is installed yet.** `pins/pins.json` `packages[]` is
empty and `harness/authorities.json` records every third-party concern as
unassigned. This is the reviewed shortlist, not the shipped set.

Verdicts: **ADOPT** — take it as the owner of a concern. **COMPOSE** — take a
part, or take the idea. **REJECT** — do not depend on it. **DEFER** — the
decision is not ready to be made.

---

## Correction to ADR 0008's package table

ADR 0008's "Initial stack direction" names `pi-subagents` and
`pi-observational-memory` in a context implying the `amosblomqvist` repositories
of those names. The npm names resolve elsewhere:

| name in ADR 0008 | what the ADR reviewed | what `pi install npm:<name>` actually gets |
| --- | --- | --- |
| `pi-subagents` | `amosblomqvist/pi-subagents` — no LICENSE, no `package.json` | `pi-subagents@0.62.0` → `nicobailon/pi-subagents`, MIT |
| `pi-observational-memory` | `amosblomqvist/pi-observational-memory` — not on npm | `pi-observational-memory@3.0.4` → `elpapi42/pi-observational-memory`, MIT |

Verified with `npm view <name> repository.url`. An erratum is recorded at the
bottom of ADR 0008. The package you actually get is, in both cases, the stronger
of the two — but an installer following the ADR literally would install
something the ADR did not review.

Second correction: the `@mariozechner/*` npm scope was renamed to
`@earendil-works/*`, and `@mariozechner/pi-coding-agent` is frozen at 0.73.1.
Any package still importing that scope cannot load against the 0.84.4 pin
without a port. That disqualifies both `amosblomqvist` packages on technical
grounds independent of licensing.

---

## Subagents and orchestration

| Package | Revision read | License | Concern it would own | Limitations | Verdict |
| --- | --- | --- | --- | --- | --- |
| **`pi-subagents`** (nicobailon) | npm `0.62.0` (2026-08-31); repo `59d920f935239fc8952709d0891202f16d40c821` | MIT | Subagent process lifecycle and child run state | Upstream CI tests against a shim (`@earendil-works/pi-coding-agent@0.0.0-pi-subagents-test-shim`) and its one real-session e2e test skips when the shim is present, so upstream green is not evidence for us. Completion-wake ownership is a per-process random UUID, so a restarted parent does not re-own old runs. Result files are consumed and deleted on delivery. No npm provenance attestation on 0.62.0. 46 of 50 recent commits by one author. | **ADOPT** (Phase C) |
| `@tintinweb/pi-subagents` | npm `0.19.0`; repo `4f572eaa04c09d3dbc16e4a5f13a16b295e84e14` | MIT | same concern — cannot coexist | Children are in-process `createAgentSession()` calls, not detached processes: if the parent dies, every child dies. That fails Gate P2's "parent dies, child lives". Its peer range `>=0.84.0` is an explicit match for the pin, and it voluntarily stands down when another subagent tool is present. | **REJECT** as primary; documented fallback |
| `amosblomqvist/pi-subagents` | `1f541897588b995144f0bb8e71a335d1c85b1e62` (2026-05-22) | **none** — no LICENSE file | — | No license grant means all rights reserved. Imports the frozen `@mariozechner` scope. 992 LOC, zero tests. | **REJECT** |
| `amosblomqvist/pi-interactive-subagents` | `c3e8b53c0754ae5ccc19fdab5a7481ec039bc2f7`, `3.7.2`, not on npm | MIT | — | Frozen `@mariozechner` scope. Self-described tmux-only fork, so process supervision is delegated to a terminal multiplexer — and Gate P2 requires recovery without screen state. | **REJECT** as a dependency; **COMPOSE the idea**: it writes a `<sessionFile>.loadout.json` at spawn and refuses resume when the snapshot is missing, which is ADR 0007's immutable-loadout fence |
| `@geminixiang/pi-agent-team` | npm `0.3.0`; repo `dbfb594` | MIT (no LICENSE at repo root) | would contend for child identity | Peer deps `>=0.82.1 <0.83.0` exclude the pin. State is bounded in-memory `Map`s — its own notes say retained-team control is not durable persistence. | **REJECT** (version-incompatible) |
| `PrimeIntellect-ai/prime-agent` | `74c8d39ee16a94cc85fe6f388c2976e8d2593616`, app `0.8.1`, not on npm | MIT | not a dependency | A hard fork of Pi v0.74.0 with no merge-back. Republishes `pi-agent-core`/`-ai`/`-tui` under names it does not own, pinned to CDN URLs — composing it with our pin nests two divergent Pi cores. Hard Python requirement, default-on telemetry, unvouched PRs auto-closed. | **REJECT** as a dependency; **COMPOSE as pattern evidence** — see below |
| `@earendil-works/pi-server`, `-protocol`, `-client`, `-session-backend-sqlite-node` | npm `0.84.4`, published 2026-08-28 | MIT | would be the session-serving authority | Self-described experimental. **Not evaluated.** Same namespace and version as our pin. | **DEFER** — but assess before anyone proposes a bespoke helper daemon |

### Patterns adopted from Prime Agent without adopting Prime Agent

It is the strongest available prior art and load-bearing rather than
aspirational: 450 test files, 6,137 cases, a test-to-source LOC ratio of 1.11:1,
and process tests that SIGKILL real processes. Worth copying, in order:

1. **Non-message durable state re-injected each turn.** Goals persist as a
   `custom` JSONL entry that is never converted to a message, so it cannot be
   compacted away, and is re-serialized fresh each turn. This is the structural
   answer to "exact facts are never summarized away": the fact never enters the
   context window to be summarized. Cost is those tokens on every turn.
2. **Admission-durable topology in an append-only ledger.** A spawn is not
   admitted until its parent/child edge is durably recorded; a failed append
   fails admission. Topology is read only from the ledger, with writer-claimed
   headers actively stripped.
3. **`(pid, startId)` process identity behind one oracle** returning
   `current | replaced | gone | unknown`.
4. **Leak-over-kill as explicit policy** — never signal a live worker whose
   identity cannot be verified.
5. **Journal-before-dispatch with no replay** — a command whose result is
   uncertain is reported uncertain, never re-executed.

Do not copy its per-child loadout model (children inherit wholesale from the
parent, strictly weaker than capability ceilings), its unauthenticated daemon
socket, or the refinement layer as a memory guarantee. Its self-improvement
claim does not survive the source: the harness state has exactly one consumer,
two lines in the prompt builder, and there is no evals directory — refinement is
measured for persistence, never for effect.

---

## Process and task supervision

| Package | Revision read | License | Concern | Limitations | Verdict |
| --- | --- | --- | --- | --- | --- |
| `@geminixiang/pi-task-protocol` | `dbfb594`, `0.1.0`, **not on npm** | MIT per `package.json` | reference schema only | 304 lines, no Pi coupling, clean transition matrix. Lacks an idempotency key, revision numbers and stale-reply fencing — the three things this product needs. | **COMPOSE**: vendor and extend |
| `@geminixiang/pi-supervisor` | `dbfb594`, `0.1.0`, not on npm | MIT per `package.json` | would contend for run lifecycle | **It reconciles by killing.** On daemon restart it terminalises every non-terminal task — including ones whose process identity verifies, which it SIGTERMs, SIGKILLs and marks failed. That branch has zero test coverage, and across all 34 test files `process.kill` never appears outside production code. One commit ever. | **REJECT** |
| `pi-background-tasks` | npm `2.4.2`; repo `37fdcf0` | ISC | would contend for background children | Peer range includes the pin and it has 26,575 weekly downloads, but its own source states it plainly: no detached or restart reattachment, children are killed on session shutdown or reload. | **REJECT** |
| `pi-goal-x` | npm `0.30.5`; repo `59826ec818aa8883329a74c62000d18aa1e1dbfe` | MIT | collides with the obligation authority | The only candidate with an explicit upper peer bound (`>=0.83.0 <0.85.0`) and real fault-injection and checkpoint-recovery tests. But it writes auto-continue checkpoints **into the Pi session file** and ships a recovery tool for sessions its own earlier versions bloated — and it runs an independent completion-review agent, which is exactly the foreman disposition authority this product must own. | **REJECT** for the foundation; mine its fault-injection tests for conformance ideas |

---

## Memory (Phase E — evaluation only)

| Package | Revision read | License | Hooks | Verdict |
| --- | --- | --- | --- | --- |
| `pi-observational-memory` (elpapi42) | npm `3.0.4` = commit `e07d2b2`; HEAD `ce9fc982b3a219a7839f07c9f4a3e054e81a2b21` | MIT | `session_before_compact`, `agent_settled`, `agent_start`, `turn_end` | **DEFER.** The published 3.0.4 destroys pre-cut context when memory is empty: it hands Pi an empty compaction summary on every fresh session before the first observer run. The fix exists at HEAD and was never released. Best maintenance of the three: 104 commits, 8 contributors, CI on every push. Vendor `ce9fc98` if needed; never install 3.0.4. |
| `observational-memory` (amosblomqvist) | `78a1efcfdd46332253fb289724f05b26dfc7769e`, `0.1.0`, not on npm | MIT | `session_before_compact` and four more | **REJECT.** Never declines compaction ownership, and an observer-crash watermark that never rolls back means a crashed observer permanently skips its span — silent, unrecoverable memory loss. Harvest three ideas: cut alignment to observation-chunk boundaries, per-role USD cost accounting (the only cost accounting in any candidate), and inert-data fencing of the observer prompt. |
| `pi-continual-harness` | npm `0.8.0`; repo `e697c8e01624b0a3d35b3d322319266f205e044b` | MIT | `before_agent_start`, `turn_end`, `session_start` | **REJECT** for memory — it is an ACE-style prompt optimizer, not observational memory, and its `harness_mutate` is an ungated model-facing tool whose output lands in the system prompt next turn, with `skill` and `subagent` item kinds: a closed self-modification loop into the control path. **COMPOSE its storage model**, which is the best idea across all memory candidates: a structured CRUD delta on discrete items, stored and rendered verbatim, never summarized. |
| `pi-hermes-memory` | npm `0.9.7`, 402 stars, peer `>=0.80.6` | MIT | `session_before_compact` | **DEFER.** Ships zero tests in its tarball despite claiming 732. The one alternative that plausibly outranks elpapi42 on maintenance; needs a full evaluation before Phase E is decided. |
| `pi-blackhole` | npm `0.4.10`, peer `>=0.81.1 <1.0.0` | MIT | `session_before_compact`, `session_compact`, `context` | **REJECT.** Vendors elpapi42's ledger modules renamed under its own tree, inheriting that lineage while claiming three compaction-related hooks — the widest compaction authority grab of any candidate. |
| `pi-memory` (jayzeng), `pi-active-memory`, `pi-memsearch` | npm `0.4.2` / `1.9.0` / `1.2.1` | MIT | varies | Checked for identity, licence, peer range and compaction collision only. Not evaluated against ADR 0007. |

**The finding that should drive Phase E: not one of the six memory packages
satisfies ADR 0007's repeated-compaction constraint-survival requirement or its
dependent-session requirement.** No test anywhere in this ecosystem proves a
constraint survives N compactions, and no MemoryArena-style dependent-session
eval exists — confirmed independently across the candidates and Prime Agent.
Adoption is therefore not the decision in front of us; building the two tests
is, because that is what would let a candidate be evaluated rather than trusted.

---

## ChatGPT foreman transport (Gate P4)

| Package | Revision read | License | Verdict |
| --- | --- | --- | --- |
| `pi-gpt` | npm `0.4.3`, read from the tarball | see below | **PROBE ONLY.** Its declared source repository returns 404 from both the web and the GitHub API, which fails the dependency-curation bar for a shipped component. Its send path depends on provider security-control modules, which ADR 0008 §8 keeps out of the product. It has the better correlation material — real message ids, parent chains, a tested ancestry guard — and no durability at all. Worth a ~30-minute capability probe answering one question: can a Codex-issued token read a conversation the user created in their browser? Its **read** leg is valuable independently of which transport sends. |
| `pi-oracle` | npm `0.7.20`; repo `fitchmultz/pi-oracle`, MIT, 39 stars | MIT | **PRESUMED TRANSPORT.** The only candidate that already satisfies exact-thread binding and restart survival, which are the two requirements that cannot be glued on from outside: a detached worker outliving the Pi turn, a durable job ledger with lifecycle events, and a per-conversation lease. Correlation is positional and weak, and the reply is an accessibility-snapshot scrape rather than the source message. Validates against `@earendil-works/pi-*` `^0.80.9` while the pin is 0.84.4 — characterize that drift before relying on it. Point `PI_ORACLE_JOBS_DIR` at a durable directory; `/tmp` is the default. |

Neither delivers Gate P4 alone, and the gap is the same in both cases and is
this product's to fill: a delivery-id-in-body protocol plus an owned event
ledger. `harness/extensions/cg-foreman/transport.ts` is that interface.

---

## Summary of what would be installed, and when

| Concern | Owner | Phase |
| --- | --- | --- |
| Base agent loop, sessions, compaction mechanics, `agent_settled` | Pi 0.84.4 (ADOPT) | now |
| Subagent process lifecycle | `pi-subagents@0.62.0` | C |
| Least-authority loadouts | `pi-subagents` capability ceilings, registered by a Command Governor policy extension | C |
| Durable obligations, foreman correlation, delivery idempotence | Command Governor extension (BUILD — the genuinely missing piece) | B |
| Task/obligation schema | vendored `pi-task-protocol`, extended | B |
| Process supervision daemon | **none** — do not build or adopt one; assess `@earendil-works/pi-server` first | — |
| Compaction summary | exactly one package, not yet chosen | E |
| Exact lifecycle/capability/safety facts | Command Governor's own deterministic store, never a memory package | B |

---

## Standing constraints on any future addition

- **Popularity is not acceptance.** Every dependency must be pinned to an exact
  version or commit SHA, licensed, reviewed at that revision, and exercised by
  this repository's conformance suite. Upstream green is not evidence for us.
- **Composing a third-party Pi package is composing arbitrary code.** Pi has no
  sandbox; extensions run with the full permissions of the launching user. The
  pin is what makes a review meaningful.
- **Check the authority collision map before adding anything.** Pi detects none
  of these at runtime. The concerns most likely to collide are the compaction
  summary, subagent process lifecycle, durable owed work, independent completion
  review, and tool gating.
