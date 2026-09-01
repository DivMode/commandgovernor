# Command Governor — Pi ecosystem dependency / package selection matrix

- **Date of investigation:** 2026-09-01
- **Substrate pin under evaluation:** `@earendil-works/pi-coding-agent` **0.84.4** (npm `latest` as of 2026-09-01; repo `earendil-works/pi`, tag `v0.84.4`)
- **Method:** live npm registry queries, GitHub API, and full source clones read at the exact revisions recorded below. Nothing here is answered from model memory.
- **Status:** research input. No repository file was modified; all work happened in a scratchpad.

---

## 0. Headline correction to ADR 0008 / the 2026-09-01 research review

**The npm package names cited in ADR 0008 do not resolve to the GitHub repositories cited in ADR 0008.** This is not a nuance; it changes the candidate set.

| Name as used in ADR 0008 | What ADR 0008 points at | What `npm install` / `pi install npm:<name>` actually gets |
| --- | --- | --- |
| `pi-subagents` | `github.com/amosblomqvist/pi-subagents` — 202 stars, **no LICENSE**, no `package.json`, last commit `1f54189` 2026-05-22 | npm `pi-subagents@0.62.0` → `github.com/nicobailon/pi-subagents` — MIT, 3,412 stars, 122,432 downloads/week, last commit `59d920f` 2026-09-01 |
| `pi-observational-memory` | `github.com/amosblomqvist/pi-observational-memory` — 30 stars, MIT, `package.json` name is `observational-memory@0.1.0`, **not published to npm** | npm `pi-observational-memory@3.0.4` → `github.com/elpapi42/pi-observational-memory` — MIT, 541 stars, 1,179 downloads/week |

Verification: `npm view pi-subagents repository.url` → `git+https://github.com/nicobailon/pi-subagents.git`, maintainer `nicopreme <nico.bailon@gmail.com>`; `npm view pi-observational-memory repository.url` → `git+https://github.com/elpapi42/pi-observational-memory.git`, maintainer `elpapi42`. `gh api repos/amosblomqvist/pi-subagents/license` → HTTP 404 (no license). `gh api repos/geminixiang/pi-stuff/license` → HTTP 404 (no license).

Consequence: an installer following ADR 0008's instruction `pi install npm:pi-subagents` installs Nico Bailon's package, which is a different, larger, better-maintained project than the one the ADR reviewed. **The ADR's package table must be corrected before Gate P1.** The good news is that the package you actually get is the stronger of the two.

Second correction: the `@mariozechner/*` npm scope was renamed to `@earendil-works/*`. `@mariozechner/pi-coding-agent` is frozen at **0.73.1** (last modified 2026-05-07). Any package still importing `@mariozechner/*` cannot load against the 0.84.4 pin without a port. That disqualifies both amosblomqvist subagent packages on technical grounds independent of licensing.

---

## 1. Selection matrix

Legend for **Authority owned** — the single lifecycle concern the package would be the sole writer for under the one-authority-per-concern rule.

### A. Subagents / orchestration

| Package | Exact source / revision / version | License | Role it would own in Command Governor | Authority owned | Important limitations | Recommendation |
| --- | --- | --- | --- | --- | --- | --- |
| **`pi-subagents`** (Nico Bailon) | npm `pi-subagents@0.62.0` (published 2026-08-31); `github.com/nicobailon/pi-subagents` @ `59d920f935239fc8952709d0891202f16d40c821` (2026-09-01) | **MIT** (LICENSE file present) | Child-agent execution: spawn, fan-out, detached background runs, steering, resume, capability ceilings, run artifacts, missions ledger | **Subagent process lifecycle and child run state.** Sole writer of `~/.pi/agent/…/async/<runId>/status.json`, `events.jsonl`, result files, mission records | Upstream CI tests against a **shim** (`@earendil-works/pi-coding-agent@0.0.0-pi-subagents-test-shim`), so no upstream proof against real 0.84.4 — the one real-session e2e test skips when the shim is installed. Completion-wake ownership is a per-process random UUID, so a restarted parent does not automatically re-own old runs. Result files are consumed and deleted on delivery; replay records are explicitly "best-effort temporary state, not a permanent run ledger". No npm provenance attestation on 0.62.0. Bus factor 1 (46 of 50 recent commits by one author). | **ADOPT** |
| **`@tintinweb/pi-subagents`** | npm `0.19.0` (2026-08-27); `github.com/tintinweb/pi-subagents` @ `4f572eaa04c09d3dbc16e4a5f13a16b295e84e14` | MIT | Would be the fallback subagent runtime | Same concern as above — **cannot coexist** with `pi-subagents` | Children run as **in-process `createAgentSession()` calls** (`src/agent-runner.ts:13`), not detached OS processes. No `detached:` anywhere in `src/`. If the parent Pi process dies, every child dies. That fails Gate P2's "parent dies, child lives". Upside: peer deps are `>=0.84.0`, an explicit match for the pin, and it voluntarily stands down when another `Workflow`/`SubagentWorkflow` tool is present. | **REJECT** as primary; keep as documented fallback if `pi-subagents` becomes unmaintained |
| **`amosblomqvist/pi-subagents`** | `github.com/amosblomqvist/pi-subagents` @ `1f541897588b995144f0bb8e71a335d1c85b1e62` (2026-05-22). No version — not packaged | **NONE.** No LICENSE file, no `package.json`, `gh api …/license` → 404 | — | — | Legally unusable: no license grant means all rights reserved. Technically unusable: imports `@mariozechner/pi-coding-agent` (`index.ts:11-13`), frozen at 0.73.1. 992 LOC, **zero tests** (no `test/` or `tests/` directory). Stale 3+ months. | **REJECT** |
| **`amosblomqvist/pi-interactive-subagents`** | `github.com/amosblomqvist/pi-interactive-subagents` @ `c3e8b53c0754ae5ccc19fdab5a7481ec039bc2f7` (2026-08-24), `package.json` `pi-interactive-subagents@3.7.2`. **Not on npm** (E404) | MIT | — | — | Imports `@mariozechner/pi-coding-agent`, `@mariozechner/pi-tui`, `@sinclair/typebox` only — will not load against 0.84.4 without a port. Self-described "**tmux-only fork**"; children live in tmux panes, so process supervision is delegated to a terminal multiplexer. Gate P2 requires recovery "without relying on screen state". | **REJECT** as a dependency; **ADOPT its loadout-sidecar idea as a design pattern** — `pi-extension/subagents/session.ts:87-137` writes `<sessionFile>.loadout.json` at spawn and refuses resume when the snapshot is missing, which is exactly ADR 0007's immutable-loadout fence, and `session.ts:147-196` keeps a name→session registry that survives a Pi restart |
| **`@geminixiang/pi-agent-team`** | npm `0.3.0` (2026-08-12); `geminixiang/pi-stuff` @ `dbfb594` | MIT (per `package.json`; repo root has no LICENSE) | — | Would contend with `pi-subagents` for child identity/lineage | Peer deps `@earendil-works/pi-coding-agent >=0.82.1 <0.83.0` — **excludes 0.84.4**. 22 downloads/week. See §2 for the delegated review. | **REJECT** (version-incompatible with the pin) |
| **`PrimeIntellect-ai/prime-agent`** | `github.com/PrimeIntellect-ai/prime-agent` @ `74c8d39ee16a94cc85fe6f388c2976e8d2593616` (2026-09-01); app version `0.8.1`; not on npm (E404) | MIT (dual © Zechner + Prime Intellect) | Not a dependency | — | Hard fork of Pi **v0.74.0** with no merge-back. Republishes `@earendil-works/pi-agent-core`/`-ai`/`-tui` under names it does not own, pinned to R2 URLs — composing it with our 0.84.4 pin nests **two divergent Pi cores**. Hard Python requirement, default-on telemetry, unvouched PRs auto-closed. See §2.4. | **REJECT as dependency; ADOPT as pattern evidence** |
| **`@earendil-works/pi-server`** + `pi-protocol`, `pi-client`, `pi-session-backend-sqlite-node` | npm `0.84.4`, all published 2026-08-28 | MIT | Possible upstream-native substrate for durable session serving | Would be the session-serving authority | **Not evaluated in this review.** Self-described "experimental". Same namespace and version as our pin. Consumer supplies the `PiServerService`. Highest-value unexamined lead — see §2.4. | **DEFER — evaluate before anyone proposes a bespoke helper daemon** |

### B. Process / task supervision

| Package | Exact source / revision / version | License | Role | Authority owned | Limitations | Recommendation |
| --- | --- | --- | --- | --- | --- | --- |
| **`@geminixiang/pi-task-protocol`** | `geminixiang/pi-stuff` @ `dbfb594`, `packages/pi-task-protocol`, `0.1.0`. **Not published to npm** (E404) | MIT per `package.json`; repo has no LICENSE file | Reference contract for Command Governor's own task/obligation schema | — (pure schema, no runtime state) | 304 lines, no Pi peer dep, clean transition matrix. Lacks an idempotency key, revision/generation numbers, and stale-reply fencing — exactly the three things Command Governor needs. Unpublished, so adopting means vendoring ~300 lines. | **ADOPT as vendored reference**, then extend |
| **`@geminixiang/pi-supervisor`** | `geminixiang/pi-stuff` @ `dbfb594`, `packages/pi-supervisor`, `0.1.0`. **Not published to npm** (E404) | MIT per `package.json`; repo has no LICENSE file | — | Would contend with `pi-subagents` for run/process lifecycle, as a machine-global singleton | **It reconciles by killing.** `src/runner.ts:77-96` terminalises every non-terminal task on daemon restart — including ones whose process identity verifies, which it SIGTERM/SIGKILLs and marks `failed`. Zero test coverage of that branch. No directory fsync. One commit ever. See §2.2. | **REJECT** |
| **`pi-background-tasks`** | npm `2.4.2` (2026-08-14); `ismailsaleekh/pi-background-tasks` @ `37fdcf0` | ISC | — | Would contend with `pi-subagents` for background child processes | Peer range includes `^0.84.0`; 26,575 downloads/week; state in durable `.pi/tasks`. But `src/extension.ts:56-59` states outright: *"No detached/restart reattachment: live child processes belong to this Pi extension runtime and are killed on session shutdown/reload."* Delegated work does not survive a restart. | **REJECT** |
| **`pi-goal-x`** | npm `0.30.5` (2026-08-25); `github.com/tmonk/pi-goal-x` @ `59826ec818aa8883329a74c62000d18aa1e1dbfe` | MIT | Durable long-running objectives | **Collides with Command Governor's own obligation authority and with `pi-subagents` missions** | Peer deps `>=0.83.0 <0.85.0` — compatible with the pin, and the only candidate with an explicit upper bound. Has real fault-injection and checkpoint-recovery tests (`tests/fault-injection.test.ts`, `tests/goal-ledger-checkpoint.test.ts`, `npm run test:checkpoint-recovery`). But it writes auto-continue checkpoints **into the Pi session file** and ships `pi-goal-x-recover` to repair sessions bloated by its own earlier versions (README §"Session checkpoint recovery"). It also runs its own **independent completion review** agent, which is precisely the foreman disposition authority Command Governor must own. | **REJECT** for the foundation — direct authority conflict. Re-examine its ledger/fault-injection tests as a source of conformance-test ideas |

### C. Memory (evaluation only — not in the first PR)

| Package | Exact source / revision / version | License | Role | Authority | Limitations | Recommendation |
| --- | --- | --- | --- | --- | --- | --- |
| **`pi-observational-memory`** (elpapi42) | npm `3.0.4` (= commit `e07d2b2`); `github.com/elpapi42/pi-observational-memory` @ `ce9fc982b3a219a7839f07c9f4a3e054e81a2b21` (2026-08-21) | MIT | Observer/consolidator memory | `session_before_compact` — the compaction summary | **The published 3.0.4 destroys pre-cut context** when memory is empty; the fix (`37986b6`) exists at HEAD but was never released. Dropper may delete `critical` observations on prompt discretion alone. No cost accounting. No waiting for observers before folding. Fails ADR 0007 (c), (d), (f). Best maintenance of the three: 104 commits, 8 contributors, CI on every push/PR. | **DEFER** (vendor `ce9fc98` if needed now; never install 3.0.4) |
| **`observational-memory`** (amosblomqvist) | `github.com/amosblomqvist/pi-observational-memory` @ `78a1efcfdd46332253fb289724f05b26dfc7769e`, `0.1.0`, **not on npm** | MIT | — | Would fight elpapi42 for `session_before_compact` | Unpublished, no CI directory, 15 commits, 1 author, 1 commit in 30 days. Two live defects: never declines compaction ownership, and an observer-crash watermark that never rolls back (`observer-trigger.ts:96,182-192`) — permanent silent memory loss. No provenance, no recall. | **REJECT**; harvest `snapCutoff`, per-role cost accounting, and observer prompt-fencing |
| **`pi-continual-harness`** | npm `0.8.0` (2026-08-10); `github.com/pungggi/pi-continual-harness` @ `e697c8e01624b0a3d35b3d322319266f205e044b` | MIT | Not memory — an ACE-style system-prompt optimizer | `before_agent_start` — system-prompt injection | 44 downloads/week, 2 stars, 1 author, dormant 3 weeks, CI only on tags. `harness_mutate` is an **ungated model-facing tool** writing into the system prompt, with `skill`/`subagent` item kinds. Writes machine-global `~/.pi/agent/harness-state.md`. | **REJECT** for memory; **steal the delta-over-items verbatim storage model** |
| **`pi-hermes-memory`** | npm `0.9.7` (2026-08-29), 402 stars, peer `>=0.80.6` | MIT | Candidate observer/memory | `session_before_compact` | Not yet evaluated against ADR 0007. Ships zero tests in its tarball despite claiming 732. **The one alternative that plausibly outranks elpapi42 on maintenance.** | **DEFER — evaluate before Phase E is decided** |
| **`pi-blackhole`** | npm `0.4.10` (2026-08-29), 115 stars, peer `>=0.81.1 <1.0.0` | MIT | — | `session_before_compact` + `session_compact` + `context` | Vendors elpapi42's `session-ledger` module set renamed under `src/om/ledger/` (verified from the shipped sourcemap), so it inherits that lineage while claiming three compaction-related hooks. | **REJECT** (widest compaction authority grab of any candidate) |

---

## 2. Detailed findings

### 2.1 `pi-subagents` (Nico Bailon) — what it does and does not own

Verified against the clone at `59d920f` and an actual install against 0.84.4.

**Satisfies, with source evidence:**

- **Restart behavior — parent dies, child lives.** Async runners are spawned `detached: platform !== "win32"` (`src/runs/shared/background-process-options.ts:1-9`) and `proc.unref()` (`src/runs/background/async-execution.ts:679`). The docs state the consequence precisely: *"Detached children do not stop when the session does… the run keeps going, completes, and notifies nobody. **What is lost is the notification, not the work.**"* (`docs/extension-api.md`, §Host session lifetime and completion wakes).
- **Orphan reconciliation.** `src/runs/background/stale-run-reconciler.ts` models `PidLiveness = "alive" | "dead" | "unknown"` and repairs status via atomic JSON writes, with `missingStatusGraceMs` and `staleAlivePidMs` thresholds.
- **Ambiguity is fail-closed.** *"A proof is `observed` only after the live parent observes the exact detached runner's `close` event… If the observer is unavailable, the proof is `unknown`; do not infer process exit from `endedAt`, result-file existence, PID disappearance, or lease-directory absence."* (`docs/observability.md`, §Process-terminal proof). Capacity slots are retained on unknown proof; policy reclaim after 20 min is explicitly labelled `processProof: unknown`.
- **Loadout / least authority, and resume cannot broaden.** Capability ceilings intersect `allowedTools`/`allowedAgents` and OR `denyExtensions` (`src/runs/shared/capability-ceiling.ts:141-158`), propagate to children through `PI_SUBAGENT_CAPABILITY_CEILING_V1`, and on resume are **intersected again** across status, step and result records (`src/runs/background/async-resume.ts:485,517,558`). This is ADR 0008 invariant 6, implemented.
- **Recursive delegation.** `PI_SUBAGENT_MAX_DEPTH` with monotone tightening — per-agent `maxSubagentDepth` "can tighten the limit… but cannot relax an inherited stricter limit" (`docs/configuration.md:357`). Cumulative spawn budget claims are "never released or refunded" (`:282`).
- **Parent→child steering.** Correlated `requestId`, states `delivered|queued|missed|failed`, bounded FIFO of 20, persisted `steering` ledger (`docs/tool-reference.md:350-358`).
- **Child→parent questions.** Native `contact_supervisor` / `subagent_supervisor({action:"reply"})` — no `pi-intercom` dependency (`docs/workflows.md:403`). File-based request/reply channel with atomic writes and `reason: need_decision | interview_request | progress_update` (`src/intercom/native-supervisor-channel.ts:19-46`).
- **Uses `agent_settled` correctly.** Completion "honors `agent_end.willRetry` and prefers `agent_settled`" (`CHANGELOG.md:1063`); tested in `test/unit/child-protocol.test.ts` and `compaction-resume.test.ts`.
- **A clean seam for Command Governor.** `registerBackgroundWorkProvider` (`docs/extension-api.md`, §Background-work provider API) and `registerSubagentCapabilityCeiling` (§Capability ceilings) let a Command Governor extension own obligations and policy *without* owning subagent process lifecycle. Providers meet through `Symbol.for("pi-subagents.background-work.v1")`, so independently loaded extensions compose in one Pi process.
- **Native Herdr integration** that correctly declares "the Pi integration remains the lifecycle authority" (`docs/extension-api.md`, §Herdr integration) — relevant to this machine.

**Gaps Command Governor must own:**

1. **Run state is not reboot-durable by default.** `RESULTS_DIR`, `ASYNC_DIR`, `CHAIN_RUNS_DIR` and `TEMP_ARTIFACTS_DIR` all hang off `TEMP_ROOT_DIR`, which defaults to `path.join(os.tmpdir(), 'pi-subagents-<scope>')` (`src/shared/types.ts:2678-2686`). The only override is the environment variable `PI_SUBAGENTS_TEMP_ROOT`, which appears **nowhere** in `docs/`, `README.md`, or `CHANGELOG.md`. Missions (`~/.pi/agent/missions/…`) are durable; run state is not. **Gate P1 must set this to a durable path.**
2. **Completion wake ownership is per-process and non-durable.** `currentCompletionOwnerId()` is a `randomUUID()` held in a global symbol, documented as "Stable for one parent Pi process across extension reloads" (`src/shared/completion-owner.ts:9-14`), and `owns()` requires an exact match (`src/runs/background/result-delivery-ownership.ts:22-25`). After a parent restart the new process does not re-own old runs; recovery is via `mission.show` / `status` / `bg_wait`, not an automatic wake.
3. **Result files are consumed and deleted on delivery.** Replay records are "best-effort temporary state, **not a permanent run ledger**" and expire with the dedupe window (`docs/observability.md`).
4. **No foreman correlation protocol.** Mission receipts are limited to `pull_request | ci | deployment | release` (`src/missions/store.ts:36`); decisions are only `open|resolved` (`src/missions/types.ts:42-44`). There is **no delivery id, no task revision, no stale-reply rejection, no duplicate-reply idempotence**. This is exactly the glue ADR 0008 predicted Command Governor would still have to build.
5. **Upstream CI does not test against real Pi.** `devDependencies` resolve `@earendil-works/pi-coding-agent` to `file:./test/fixtures/pi-coding-agent-shim` (version `0.0.0-pi-subagents-test-shim`), and the single real-session test skips itself when the shim is present (`test/e2e/real-session-subagent.test.ts:20-23`). Command Governor's conformance suite must therefore run against the real 0.84.4 runtime; upstream green is not evidence for us.

**Measured compatibility (2026-09-01):** `npm install @earendil-works/pi-coding-agent@0.84.4 pi-subagents@0.62.0` resolves cleanly, no peer conflicts, `pi-coding-agent` deduped at 0.84.4. `CHANGELOG.md:566` records explicit work to keep behavior correct "across the Pi 0.81 and 0.84 APIs". No install/postinstall scripts; ships readable TypeScript, no bundle. **No npm provenance attestation on 0.62.0** (`dist.attestations` is `null` in the registry document) despite the release workflow using `npm publish --provenance` — pin by version and lockfile integrity hash.

### 2.2 `geminixiang/pi-stuff` — the supervisor does not do what its README says

The direct answer to research question C is **no**.

`@geminixiang/pi-supervisor` is described by its own README as a "crash-reconciled local task daemon". The code reconciles **by killing**. On daemon restart, `recover()` terminalises every non-terminal task without exception (`packages/pi-supervisor/src/runner.ts:77-96`): if process identity cannot be revalidated the task is marked `orphaned`; if identity *does* revalidate, it SIGTERMs, waits 2s, SIGKILLs, and marks the task `failed` with reason `daemon_restart_interrupted`. There is no reattach path, and the extension's own user-facing warning admits it: *"a restarted daemon cannot reattach to their output"* (`extensions/pi-supervisor.ts:141`). Separately, `session_shutdown` calls `stopOwner` to kill the session's tasks unless the reason is `reload` (`extensions/pi-supervisor.ts:255-261`).

The branch that decides the fate of surviving work (`runner.ts:88-94`) has **zero test coverage**. The one test named for reconciliation (`test/daemon.test.ts:182-218`) closes the daemon *gracefully*, hand-writes a synthetic record with **no `pid` field**, and constructs a second daemon in the same Node process — so `processIdentity()`, the package's headline safety feature, is never called by any test. Across all 34 test files `process.kill` never appears outside production code.

Other material facts:

- **Publication.** Only `@geminixiang/pi-agent-team` is on npm (0.3.0, 2026-08-12, MIT, SLSA provenance). `pi-task-protocol`, `pi-supervisor`, `pi-verification`, `pi-memory`, `pi-hooks`, `pi-remember` are all E404 — and this is deliberate: `.github/workflows/publish.yml:11-19,44-47` hard-codes a publishable allowlist that excludes them. Their inter-package specifiers (`"@geminixiang/pi-supervisor": "0.1.0"`) cannot resolve from the registry at all. Even published `pi-agent-team@0.3.0` lags repo HEAD by two commits, one of them a security-shaped fix ("restrict implicit group claims").
- **Maintenance.** A 15-day repo (created 2026-08-03, last push 2026-08-20), 50 commits, **one author**, 1 star. Decisively: **six of the seven packages have exactly one commit each — the repo's root commit.** `pi-supervisor` is a day-one artifact that has never met a bug report. Only `pi-agent-team` has real iteration (34 commits).
- **Peer ranges vs. the pin.** Measured empirically at 0.84.4: all six packages typecheck clean and pass all 225 tests. But the *declared* ranges exclude it — `pi-agent-team` `>=0.82.1 <0.83.0`, `pi-verification` and `pi-memory` pinned to the exact string `"0.82.1"`. Only `pi-hooks` declares `"*"`. Working-in-practice does not survive `npm install`.
- **Licensing.** No LICENSE file at the repo root (`gh api …/license` → 404); every `package.json` declares MIT. A license field without license text is a weak grant for code you must vendor.
- **`pi-agent-team` durability is in-memory only.** `src/` contains **zero filesystem imports**; state is bounded `Map`s, and its own `CONTEXT.md` invariant 27 says "Retained-team control is in-process, not durable persistence". Its SHA-256 causal audit chain is real but never written to disk.

Worth stealing rather than depending on: `pi-task-protocol`'s state machine and transition matrix (`src/index.ts:102-114`, 304 lines, no Pi coupling), `pi-supervisor`'s `processIdentity()` pid+pgid+start-time check (`runner.ts:36-49`), and its rotating logical-offset log (`logfile.ts`).

### 2.3 Memory — and a Pi-core fact that makes "one authority" unenforceable by convention

**Pi 0.84.4 has no conflict detection on `session_before_compact`.** In the installed runtime, `node_modules/@earendil-works/pi-coding-agent/dist/core/extensions/runner.js:626-641`, every handler result for a `session_before` event **overwrites** the previous one. When two extensions each return a `compaction` object, the last one loaded silently wins — no error, no warning, no merge. Handler exceptions are swallowed into `emitError` (`:639-645`).

This is the single most important architectural finding in this review. "One authority per concern" cannot be a documented convention in the Command Governor distribution; it must be an **install-time assertion**, because a second compaction-hooking extension is not a degraded mode — it is an undetectable, load-order-dependent override.

Hooks registered by each candidate, verified by grep:

| Package | Hooks |
| --- | --- |
| elpapi42 `pi-observational-memory` | `session_before_compact`, `agent_settled`, `agent_start`, `turn_end` |
| amosblomqvist `observational-memory` | `session_before_compact`, `session_start`, `session_shutdown`, `turn_end` ×2, `agent_start` |
| `pi-continual-harness` | `before_agent_start`, `turn_end` ×3, `session_start` |

All three typecheck clean and pass their full suites against a forced 0.84.4 pin (elpapi42 257 tests, amosblomqvist 98, continual-harness 137). None uses the frozen `@mariozechner/*` scope. Compatibility is not the discriminator here.

#### `pi-observational-memory` (elpapi42) — DEFER; adopt HEAD, never 3.0.4

**The npm artifact is broken in a way that destroys context.** Tag `3.0.4` is commit `e07d2b2` (2026-08-11); six commits landed after it, including `37986b6` "fix/empty-compaction-fallback" 3.5 hours later, never published. At HEAD, `src/hooks/compaction-hook.ts:41-45` declines ownership when the rendered summary is empty so Pi's native summarizer preserves the pre-cut context. At 3.0.4 that guard does not exist, and `render-summary.ts:21` returns `""` when memory is empty — so **the published version hands Pi an empty compaction summary and destroys the pre-cut context on every fresh session before the first observer run**. Its own test suite was green because `tests/compaction-hook.test.ts:55` asserted the bug as correct behaviour; HEAD renames that test to "delegates to native compaction".

Against ADR 0007: **(a) partial** — advisory in framing (`render-summary.ts:3-10`), but it owns what survives compaction, the dropper may delete `critical` observations with only a prompt asking it not to (`src/agents/dropper/prompts.ts:28`; `tests/dropper.test.ts:116` asserts critical ids are *accepted* for dropping), and the observer takes raw transcript chunks with no inert-data fencing (`src/agents/observer/agent.ts:104,170`). **(b) best of the three, with a ceiling** — the compaction projection is a pure model-free fold (`src/session-ledger/projection.ts:173-208`) rendered by string concatenation (`render-summary.ts:20-31`), and it is the only candidate with `sourceEntryIds` provenance (`types.ts:25-32`) plus a verbatim `recall` tool (`src/tools/recall-observation.ts:438-471`); but observation text is still LLM prose and `types.ts:110` forbids newlines in reflections, so a multi-line exact constraint cannot be stored. **(c) NO** — no test runs a constraint through N compaction generations; the fold algebra is proven, constraint survival is not. **(d) NO** — no eval harness. **(e) gap** — it folds "whatever ledger state is already present" without waiting for observers (`docs/how-it-works.md:335`) and never checks coverage reaches `firstKeptEntryId`, so a lagging observer means the uncovered tail is cut with nothing representing it, and because older observations make the summary non-empty the HEAD decline-path does not fire. Coverage does correctly refuse to advance on observer failure. **(f) NO** — no cost accounting at all.

Maintenance is the best here: 104 commits, 8 contributors, 36 commits in 30 days, CI on every push and PR with SHA-pinned actions.

#### `observational-memory` (amosblomqvist) — REJECT, harvest three ideas

Unpublished, no `.github/` directory at all, 15 commits, single author, 1 commit in 30 days. Two live context-destroying defects: it **never declines compaction ownership** (`src/hooks/compaction-hook.ts:157-164` returns unconditionally while `src/ledger/render.ts:32` returns `""` when empty — the same defect as 3.0.4, unfixed at HEAD), and it has an **unrecoverable watermark bug** — `src/hooks/observer-trigger.ts:96` advances `dispatchedCoversUpToId` at dispatch and the `catch`/`finally` at `:182-192` never rolls it back, so a crashed observer permanently skips its span for the rest of the session. That is precisely the ADR 0007 (e) failure mode. It also has no provenance and no recall (`src/ledger/types.ts:53-57`), and `JOURNEY.md` is LLM-rewritten prose read verbatim into every compaction — textbook ACE context collapse.

Three ideas worth harvesting, each filling a gap elpapi42 has: **`snapCutoff`** cut alignment to observation-chunk boundaries (`compaction-hook.ts:44-67`), **per-role USD cost accounting** (`agent/cost.ts:20-44`, `src/ledger/types.ts:69-73` — the only cost accounting in any candidate), and **inert-data fencing of the observer prompt** (`src/hooks/observer-trigger.ts:130-146`, added after observed role-confusion failures).

#### `pi-continual-harness` — REJECT for memory; steal the storage model

Not an observational-memory package — it never hooks `session_before_compact`. 2 stars, one author, 30 commits all inside a 4-day burst then 22 days silent; CI runs only on `v*` tags, so commits and PRs are never tested. **`harness_mutate` is an ungated model-facing tool** (`src/tools.ts:98-129`) whose output lands in the **system prompt** next turn (`src/inject.ts:74-92`) with no user approval step, and its item kinds include `skill` and `subagent` — a closed self-modification loop into the control path. Mitigations are real (server-stamped `ownerModel`, mandatory `evidence`, branch-local `pi.appendEntry` so `/tree` can roll back, and the autonomous paths default off at `config.ts:51,73`), but the tool itself is not gated.

Its storage model is nonetheless **the single best idea across all the memory candidates and the direct answer to ADR 0007 requirement (b)**: a structured CRUD delta on a discrete item, content stored verbatim (`store.ts:89,113`) and rendered verbatim (`inject.ts:49`). Nothing ever summarizes an item; an exact constraint written once survives byte-identical unless explicitly updated or deleted. The residual risk is omission, not corruption — `select.ts:57-62` defaults (`maxTokens: 1500`, `maxPerKind: 10`) silently drop low-`importance` items, and `importance` is model-assigned. Note also `store.ts:21` writes machine-global state to `~/.pi/agent/harness-state.md`, which conflicts with this machine's declarative-configuration policy.

#### Stronger alternatives exist and were not fully evaluated

| Package | Version | Published | Stars | peerDep | Hooks `session_before_compact`? |
| --- | --- | --- | --- | --- | --- |
| `pi-hermes-memory` | 0.9.7 | 2026-08-29 | 402 | `>=0.80.6` | **Yes** |
| `pi-memory` (jayzeng) | 0.4.2 | 2026-08-11 | 143 | `>=0.81.1` | **Yes** |
| `pi-blackhole` | 0.4.10 | 2026-08-29 | 115 | `>=0.81.1 <1.0.0` | **Yes** (+ `session_compact`, `context`) |
| `pi-active-memory` | 1.9.0 | 2026-08-29 | 0 | `>=0.82.0` | No |
| `pi-memsearch` | 1.2.1 | 2026-08-19 | 3 | `>=0.84.1` | not checked |

Two verified details: **`pi-blackhole` vendors elpapi42's code** — its shipped sourcemap lists elpapi42's `session-ledger` module set renamed under `src/om/ledger/`, so it inherits that lineage while grabbing three compaction-related hooks instead of one. **`pi-hermes-memory` ships zero tests in its tarball** (89 files) despite claiming 732 tests; not necessarily wrong, but unverifiable from the artifact. These five were checked for identity, licence, peer range and compaction collision only — **`pi-hermes-memory` is the one candidate that plausibly outranks elpapi42 on maintenance and deserves a full ADR 0007 evaluation before Phase E is decided.**

Caveat on method: npm text search is not a trustworthy filter in this ecosystem — the sweep surfaced packages with 404ing repos (`pi-observational-memory-pct`), null repository fields (`pi-mem-cc`), and copy-pasted descriptions (`pi-mcb` carries pi-blackhole's exact description but peer-depends on `@huggingface/transformers` and no Pi package at all).

**The finding that should drive Phase E planning: not one of the six memory packages examined satisfies ADR 0007 (c) or (d).** No test anywhere proves a constraint survives N compactions, and no MemoryArena-style dependent-session eval exists in this ecosystem. Adoption is therefore not the decision in front of us — building the constraint-survival test and the dependent-session eval is, because that is what would let us *evaluate* a candidate instead of trusting its README.

### 2.4 Prime Agent — pattern evidence, and an upstream option it revealed

**It is a hard fork of Pi v0.74.0, not a downstream dependent.** Fork commit `8b5abc6a` "fork pi-mono as prime agent" (2026-05-08), parent `0bcaab42`, ~7 commits after upstream's `1eee081e Release v0.74.0`. No merge-back commits exist. Its README says so plainly (`packages/coding-agent/README.md:16`), and its four workspace packages still carry upstream's names at `"version": "0.8.1"`.

**REJECT as a dependency — it cannot coexist with our pin.** It ships `@earendil-works/pi-agent-core`, `-ai`, `-tui` under names it does not own, pinned to R2 tarball URLs at a 0.74-derived `0.8.1`, while we pin upstream `pi-coding-agent@0.84.4` whose deps are `^0.84.4` of those same names. npm resolves that by nesting: **two divergent Pi cores in one tree.** Add: no npm registry presence (`prime-agent` → E404, verified), a single vendor CDN in the dependency graph, a hard Python runtime requirement (`ipython` is the only tool), default-on telemetry, and a contribution policy that auto-closes unvouched PRs — so no route to land a fix. Nothing is extractable as a Pi extension; the durability layer is welded into a 124k-LOC application.

**ADOPT as pattern evidence.** It is the strongest available prior art, and load-bearing rather than aspirational: 450 test files / 6,137 cases, test-to-source LOC ratio 1.11:1, 2 retries in 4,407 coding-agent cases, and process tests that spawn the real CLI and SIGKILL/SIGSTOP real processes. Patterns worth copying, in order:

1. **Non-message durable state re-injected each turn.** Goals persist as a `custom` JSONL entry that is *never converted to a message* (`session-manager.ts:478-488`), so it cannot be compacted away; it is re-serialized fresh from a force-flushed record each turn (`goals.ts:4`, `agent-session.ts:1664-1671`). **This is the correct structural answer to ADR 0007's "exact facts never summarized away"** — the fact never enters the context window to be summarized. Cost: those tokens on every continuation turn.
2. **Admission-durable topology in a supervisor-owned append-only ledger.** A spawn is not admitted until its parent/child edge is durably recorded — a failed append fails admission (`daemon-mode.ts:2586-2609`) — and topology is read *only* from the ledger, with writer-claimed headers actively stripped (`rlm-ledger.ts:595-609`). This is how children stay findable after process death without trusting any worker's account of its own parentage.
3. **`(pid, startId)` process identity behind one oracle** returning `current | replaced | gone | unknown` (`daemon-supervisor.ts:5906`, `session-lease.ts:140-164`) — the same idea as `pi-supervisor`'s `processIdentity()`, but with the conservative-in-both-directions rule written at the decision point.
4. **Leak-over-kill as explicit policy** (`daemon-supervisor.ts:3640`): never signal a live worker whose identity cannot be verified; park it `failed` with the process running, bounded at 10 rounds with a manual retry escape.
5. **Journal-before-dispatch with no replay** (`command-recovery-journal.ts`): a command whose result is uncertain is *reported* uncertain, never re-executed. This is ADR 0008 invariant 4, implemented.
6. **Admission returns a handle; results arrive as messages** (`agent-session.ts:10533`) — kills the parent-blocks-on-child deadlock class.

**Do not copy:** its per-child loadout model (`rlm.run` accepts only `name`/`model`/`thinking`; everything else is inherited wholesale from the parent, `agent-session.ts:9462-9483` — strictly weaker than `pi-subagents`' capability ceilings), per-parent-only child ids, the unauthenticated public daemon socket (`daemon-supervisor.ts:1197` sets `authenticated: true` unconditionally), and the refinement layer as a memory *guarantee*.

**Its self-improvement claim does not survive the source.** `HarnessState` has exactly one consumer outside the refinement module — two lines in the prompt builder (`system-prompt.ts:147-149`). No tool registry, permission, retry or routing code reads it. A harness `skill` entry is not an installed skill. Injection is lossy (180-char overview limit, max 6 entries per kind). There is no evals/benchmarks directory: **refinement is measured for persistence, never for effect.** This independently corroborates §2.3's conclusion — nobody in this ecosystem has built the dependent-session efficacy test.

**Durability caveat worth carrying:** `fsync` coverage is inconsistent across three tiers, and the two most authority-bearing writers (`daemon-supervisor-ownership.ts:935`, `persistWorker` at `daemon-supervisor.ts:1176`) are on the weakest tier — rename-atomic with no fsync at all. Session transcripts have zero fsync calls. Clean survival of a process crash; not of power loss. And `grep -rn fsync packages/*/test` returns **0** — every `fsyncSync` could be deleted and CI stays green.

#### The upstream option this surfaced

Chasing the fork comparison turned up something we had missed, and I verified it directly against the registry (2026-09-01): **upstream Pi publishes a composable server stack at 0.84.4**, all MIT, all published 2026-08-28, all in the namespace we already pin:

| Package | Version | License |
| --- | --- | --- |
| `@earendil-works/pi-server` | 0.84.4 | MIT |
| `@earendil-works/pi-protocol` | 0.84.4 | MIT |
| `@earendil-works/pi-client` | 0.84.4 | MIT |
| `@earendil-works/pi-session-backend-sqlite-node` | 0.84.4 | MIT |
| `@earendil-works/pi-telemetry` | 0.84.4 | MIT |

`pi-server` describes itself as an "experimental server package for pi" — a session server over unix sockets with length-prefixed CBOR, where the consumer supplies the `PiServerService` (session listing, locks, attachment). This matters for two ADR 0008 decisions. It means §5's "Pi-native durability" has an **upstream-native, same-namespace, same-version option** rather than only third-party packages or a bespoke helper. And it is the honest first stop on the composition ladder before anyone proposes a Command Governor daemon: it composes, where Prime Agent does not. It is marked experimental, it gives a session server rather than obligations, and it was **not** evaluated in this review — flagging it as the highest-value unexamined lead.

---

## 3. Authority collision map

Pairs that would fight over the same lifecycle state if composed. Per §2.3, Pi 0.84.4 detects none of these at runtime.

| Concern | Contending candidates | Failure mode if both installed |
| --- | --- | --- |
| **Compaction summary** (`session_before_compact`) | elpapi42 `pi-observational-memory` · amosblomqvist `observational-memory` · `pi-hermes-memory` · `pi-memory` (jayzeng) · `pi-blackhole` | **Silent last-loaded-wins** (`runner.js:626-641`). Summary, `firstKeptEntryId` and details payload all come from whichever extension loaded last. No error. Pick exactly one, forever. |
| **Subagent process lifecycle** | `pi-subagents` · `@tintinweb/pi-subagents` · `pi-background-tasks` · `@geminixiang/pi-supervisor` · `@geminixiang/pi-agent-team` | Two run registries, two reconcilers, two definitions of "this child is finished". `@tintinweb` at least stands down when another `Workflow`/`SubagentWorkflow` tool is present; the others do not. |
| **Durable owed work / objectives** | `pi-goal-x` · `pi-subagents` missions · Command Governor obligations | Three ledgers disagreeing about what is still owed. `pi-goal-x` additionally writes auto-continue checkpoints into the Pi session file. |
| **Independent completion review** | `pi-goal-x` completion auditor · Command Governor foreman | Two things claim to decide whether work is done. This is the product's core differentiator; it cannot be delegated to a package. |
| **Tool gating / veto** | `@geminixiang/pi-hooks` (`index.ts:81-97` returns `{block:true}`) · Command Governor policy extension | Two independent deny paths over one decision. Invariant 7 (user-owned high-risk decisions) needs a single arbiter. |
| **System-prompt / context injection** | `pi-continual-harness` (`before_agent_start`) · `@geminixiang/pi-memory` (`before_agent_start`) · any memory package's compaction block | Additive rather than overwriting, so not silent corruption — but unbounded, uncoordinated context spend with no shared budget. |
| **When to compact** | elpapi42 (`agent_settled` → `ctx.compact()`) · `pi-active-memory` (`agent_settled`) | Not an authority clash, but two independent compaction clocks interleave badly. |
| **Durable "remembered decisions" substrate** | `@geminixiang/pi-memory` (machine-local JSONL) · `@geminixiang/pi-remember` (git-tracked `AGENTS.md`) | Two memory surfaces that will disagree. |

**Design consequence.** The Command Governor distribution must ship an install-time conflict assertion, not a documented convention.

Note the mechanism constraint: **Pi 0.84.4 exposes no runtime API for enumerating loaded extensions.** `ExtensionContext` has no such method; `extensions: Extension[]` exists only on the internal `LoadExtensionsResult` (`dist/core/extensions/types.d.ts:1330`), which an extension cannot reach. So the check must run at distribution build/install time over the pinned `settings.json` `packages`/`extensions` manifest — which is fine, because the distribution controls that manifest — optionally backed at runtime by a claimed `Symbol.for("commandgovernor.authority.<concern>")` registry, mirroring the pattern `pi-subagents` already uses for `Symbol.for("pi-subagents.background-work.v1")`.

---

## 4. Recommended one-authority-per-concern stack

### Phase C — subagents

| Concern | Authority | Rationale |
| --- | --- | --- |
| Base agent loop, sessions, compaction hooks, `agent_settled` | **Pi 0.84.4** (ADOPT) | Pi ships subagents only as an *example extension* (`examples/extensions/subagent/`, per the v0.84.4 extension docs), so the composition-first ladder correctly moves to step 2 for delegation. |
| Subagent process lifecycle: spawn, detached background runs, fan-out, steering, resume, reconciliation | **`pi-subagents@0.62.0`** (ADOPT) | The only candidate that detaches children so they outlive the parent, reconciles orphans on `alive/dead/unknown` PID liveness, fails closed on ambiguous process proof, and intersects capability ceilings on resume. |
| Least-authority loadouts and role restriction | **`pi-subagents` capability ceilings**, registered by a Command Governor policy extension | `registerSubagentCapabilityCeiling` gives us invariant 6 and 7 enforcement without owning the spawn path. |
| Durable obligations, foreman event/action correlation, delivery idempotence, stale-revision rejection | **Command Governor extension** (BUILD — the only genuinely missing piece) | `pi-subagents` missions are a recovery ledger with `pull_request\|ci\|deployment\|release` receipts and `open\|resolved` decisions. No delivery id, no revision, no dedupe. Register it with `registerBackgroundWorkProvider` so `bg_wait` sees our work without us owning run lifecycle. |
| Task/obligation state machine schema | **Vendored `pi-task-protocol`**, extended with an idempotency key and generation number | 304 lines, zero Pi coupling, clean transition matrix; unpublished so we own it regardless. |
| Process supervision daemon | **None — do not build or adopt one for the foundation** | `pi-subagents` already detaches children that outlive the parent and reconciles them. `@geminixiang/pi-supervisor` would be a regression (§2.2). If a helper genuinely proves necessary later, evaluate upstream `@earendil-works/pi-server@0.84.4` *before* proposing a bespoke one (§2.4). |

Everything else in category A is rejected: `@tintinweb/pi-subagents` (in-process children die with the parent), both amosblomqvist packages (no license / frozen `@mariozechner` scope / tmux dependency), `@geminixiang/*` (version-incompatible as declared, one commit each, unpublished), `pi-goal-x` and `pi-background-tasks` (authority collisions, and the latter explicitly does not reattach).

### Phase E — memory

**Recommendation: adopt nothing yet; build the evaluation harness first.**

| Concern | Authority | Rationale |
| --- | --- | --- |
| Exact lifecycle, capability, safety and user-owned decision facts | **Command Governor's own deterministic store** — never a memory package | ADR 0007 §8 and ADR 0008 invariant 5. Non-negotiable and unchanged by this review. |
| Compaction summary (`session_before_compact`) | **Exactly one package, not yet chosen** | Currently `DEFER`. elpapi42 `pi-observational-memory` at HEAD `ce9fc98` is the reference architecture; `pi-hermes-memory` must be evaluated before deciding. |
| Advisory orientation memory | Same single owner as above | — |
| Structured exact-constraint retention | **Command Governor**, combining two borrowed designs | `pi-continual-harness`'s delta-over-items verbatim storage (content stored and rendered byte-identical, never summarized) for *what* is stored; Prime Agent's goal pattern — a `custom` JSONL entry that is never converted to a message and is re-injected fresh each turn — for *how it survives compaction*. Structurally immune, because the fact never enters the window to be summarized. Neither package is adopted; both designs are. |

The gating work for Phase E is not selection, it is **Gate P3's two tests**, which do not exist anywhere in this ecosystem — confirmed independently across the memory candidates and Prime Agent: a repeated-compaction constraint-survival test, and a MemoryArena-style dependent-session test where earlier experience must change a later action. Prime Agent has no evals directory at all; upstream Pi has `packages/evals` (a real `AgentSession` adapted to `vitest-evals`) but does not publish it — that is the better starting shape.

---

## 5. What blocks Gate P1, and what does not

**Blocks P1 (must be fixed in the first PR):**

1. **`PI_SUBAGENTS_TEMP_ROOT` must be set to a durable path.** Otherwise all async run status, results and artifacts live under `os.tmpdir()` and are lost to a reboot or tmp reaper. This is undocumented upstream, so it will not be found by reading the configuration guide. Gate P1's "loads the distribution reproducibly" must include asserting this value.
2. **Correct ADR 0008's package table.** `pi-subagents` and `pi-observational-memory` name different projects than the ADR reviewed (§0). Leaving this uncorrected means the pinned manifest and the ADR disagree about what is being installed.
3. **Install-time authority assertion.** Because Pi silently resolves `session_before_compact` conflicts by load order and offers no enumeration API, the distribution needs a build/install-time check over its own pinned manifest.

**Does not block P1 (but must be scheduled):**

- Command Governor's own conformance suite must run against real Pi 0.84.4. Upstream `pi-subagents` CI tests against a shim and skips its one real-session test, so upstream green is not evidence for us. This is Gate P2 work, not P1.
- Completion-wake ownership does not survive parent restart (`completion-owner.ts:9-14`). Recovery via `mission.show`/`status`/`bg_wait` exists; the durable obligation store we are building is what closes it. Gate P2.
- No npm provenance attestation on `pi-subagents@0.62.0`. Mitigate with a lockfile integrity pin; revisit if the maintainer's `--provenance` publish starts producing attestations.
- Single-maintainer risk on `pi-subagents` (46 of 50 recent commits by one author). Mitigation is the pinned fork-ability of an MIT, source-shipped, unbundled package plus our own conformance suite — not a second subagent package.
- `@earendil-works/pi-server@0.84.4` and its siblings are unevaluated. They do not block P1, but they must be assessed before any proposal for a Command-Governor-owned helper daemon, because ADR 0008 §3 puts ADOPT-upstream above EXTEND and this is an upstream option that did not exist when the ADR was written.

---

## 6. Recommended amendments to ADR 0008

1. **Correct the package table** (§0). Name the repositories explicitly alongside the npm names, since the two disagree.
2. **Record the substrate-level constraint** that Pi 0.84.4 resolves competing `session_before_compact` handlers silently by load order and exposes no extension-enumeration API. ADR 0008's "overlapping packages can create conflicting authorities if installed carelessly" is listed as a *cost*; it is actually an undetectable failure mode, and the mitigation belongs in the decision, not the consequences.
3. **Add `@earendil-works/pi-server` and siblings to the investigation-order table** under "process/task supervision" and "sessions", ahead of the third-party candidates.
4. **Soften the `pi-observational-memory` / `pi-continual-harness` naming in §9** to "evaluate" rather than "default starting point" — on the evidence, neither is adoptable as-is, and the stronger `pi-hermes-memory` was not considered.
5. **Note that Gate P3's two tests do not exist in the ecosystem** and are net-new Command Governor work, not a package selection.



