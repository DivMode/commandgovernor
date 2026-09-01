# Gate S0/S1 evidence — Prime Agent v0.8.1 substrate bake-off on the real Mac

Executed 2026-09-01 (evening, Pacific/Honolulu machine time) from Claude Code session `019B1Q2WCrhkPo2LJfhfvuB4`. Everything below was run, not inferred; each claim points at a scenario log.

## Gate S0 — substrate smoke (real Mac, disposable state roots)

**Machine:** macOS 15.7.7 (Darwin 24.6.0), Apple Silicon arm64, Node v24.19.0, npm 11.17.0, uv 0.11.21 on PATH. Every candidate ran under a redirected `HOME` and agent directory inside a session scratchpad; the real `~` was diffed against a baseline listing after every run and never changed. Model traffic went to a local mock OpenAI-compatible server (`127.0.0.1:18765`, no credentials) that logs every request, so "did the runtime call the model again" is answerable from a file rather than inferred.

### Candidate pins and integrity

| Candidate | Pin | Tag → commit verified via GitHub API | Distribution | Integrity |
|---|---|---|---|---|
| upstream Pi | v0.84.4 | `b79e4cc834970cca69daebffab7df1da7d1e52c4` ✔ | npm `@earendil-works/pi-coding-agent@0.84.4` via upstream installer lockfile | vendored `package.json`/`package-lock.json` sha256 match release `SHA256SUMS`; `npm ci --ignore-scripts` → `pi --version` = 0.84.4 |
| Prime Agent | v0.8.1 | `514633727bf26d74f39f3119c2b0e31a5ceb2a9d` ✔ | **not on the npm registry**; GitHub release tarballs `prime-agent-0.8.1.tgz` + three siblings | all four tarballs match release `SHA256SUMS`; `prime-agent --version` = 0.8.1 |
| Oh My Pi | v18.0.11 | `b8ce33a58911c26bed1d84f0db9a5e2e727c49a2` ✔ | single Bun-compiled binary `omp-darwin-arm64` (131 MB), codesigned | matches release `SHA256SUMS.txt`; `omp --version` = omp/18.0.11 |

Prime packaging facts that matter for a pinned distribution:

- The wrapper package's three sibling dependencies are declared as **bare URLs on a Cloudflare R2 bucket** (`pub-728493de92a943e2a9b2d17b4719f318.r2.dev`) with no integrity hash in `package.json`. The bytes npm fetched from R2 have sha512 identical to the GitHub release assets (all three checked), so the mirror was honest today, but the pin is a URL, not a hash. A Command Governor manifest must pin the tarball hashes itself.
- Prime republishes upstream Pi's package names (`@earendil-works/pi-agent-core`, `pi-ai`, `pi-tui`) at version 0.8.1. One `node_modules` tree cannot hold both Pi 0.84.4 and Prime 0.8.1; Governor must never co-install them.
- Prime's documented install is `npm install -g`; on this Nix-managed Mac a guard blocks global npm installs. A repo-local `npm install <tgz>` works and is what the bake-off used.
- npm 11's `allow-scripts` policy skipped every install script (Prime's own `postinstall.cjs`, zeromq, koffi). Nothing broke: zeromq ships a darwin-arm64 prebuilt and Prime's postinstall is a no-op unless opt-in env vars are set. Worth knowing for the manifest.
- First tool use triggers a kernel bootstrap: `uv` downloaded CPython 3.11.15 and built a 267 MB venv (`~/.prime/agent/kernel-venv`, plus a 296 MB uv cache), all under the redirected HOME. Network required; ~1 minute.

### Smoke matrix

| Step | Pi v0.84.4 | Prime Agent v0.8.1 | OMP v18.0.11 |
|---|---|---|---|
| checksum / install | PASS | PASS | PASS |
| `--version`, `--help` / mode discovery | PASS (text/json/rpc; **no ACP mode**) | PASS (text/json/rpc/**acp**/daemon) | PASS (text/json/rpc/rpc-ui/**acp**) |
| fresh session (print mode, mock model) | PASS | PASS | PASS |
| resume (`--continue`) appends to the same transcript | PASS (7 lines, both markers) | PASS (10 lines, both markers) | PASS (10 lines, both markers) |
| load harmless local extension | PASS (marker file written on `loaded` + `session_start`) | PASS (same extension file, loaded inside the worker process) | not exercised (donor only) |
| load harmless local skill | PASS | PASS (skill marker present in the system prompt the mock received) | not exercised |
| ACP `initialize` | n/a | PASS: `agentInfo prime-agent 0.8.1`, `_meta["ai.primeintellect.prime-agent"]` present, `loadSession:false`, `sessionCapabilities:{close}` | PASS: `oh-my-pi 18.0.11`, `loadSession:true`, `sessionCapabilities:{list,fork,resume,close}` |
| clean shutdown / no surviving processes | PASS (one-shot process exits) | **see findings** | PASS |
| writes outside the disposable roots | none | `$TMPDIR/prime-agent-<uid>/` socket dir (documented location) | none |

Stdin note: all three runtimes read a prompt from stdin when it is a non-TTY pipe, so a harness must close stdin (`</dev/null`) or every print-mode run hangs. Not a defect; a footgun.

### Prime S0 findings

1. **Resident daemon by design.** A single `prime-agent -p` leaves a detached supervisor plus a catalog subprocess running (documented). `prime-agent shutdown` refuses without a TTY unless `--force` is given.
2. **ACP mode leaks a resident worker.** After `prime-agent --mode acp` received `initialize` and its stdin closed, a resident root worker (with a Python kernel child) stayed alive with `attachedClients: 0`, `messageCount: 0`, `isStreaming: false`, no bash, no children, no queue, and was still listed as `activity: "working"` 15 minutes later. Its descriptor has no `ownerClientId`, so it is not a client-owned worker; the daemon doc lists print/JSON/RPC/`--no-session` as client-owned and does not mention ACP. Every ACP invocation would accumulate one such worker until `shutdown --force`.
3. **`--daemon-socket` is not accepted by the `status`, `list`, `shutdown` subcommands**, only by run options. And a socket path longer than the macOS `sun_path` limit fails with a bare `listen EINVAL` (environment fault, but the error is unhelpful).
4. **Full client environment crosses the socket.** `create`/`attach` carry `launchEnv` = the client's entire environment minus `PRIME_AGENT_INTERNAL_*`. On this machine that included a 1Password service-account token and a Claude Code messaging token. Prime did not persist them anywhere under its home (verified by grepping for the value), but they sit in supervisor memory and traverse the 0600 socket. Gate S3 must treat the daemon socket as a credential boundary.
5. OMP side effects worth noting for a donor: it downloads a native addon at first run (`~/.omp/natives/18.0.11/pi_natives.darwin-arm64.node`), creates SQLite DBs (`agent.db`, `models.db`) and a `mise` data directory.

**S0 verdict: PASS with findings 2 and 4 carried into S1/S3.** No blocking packaging or runtime defect; the documented daemon/ACP/RLM foundation is present in the stable release.

## Gate S1 — Prime Agent durability / ambiguity conformance

### Method

Every scenario drives Prime's **public local daemon protocol v7 / schema 22** over its Unix socket with an independent raw JSONL client (about 80 lines, no Prime code imported), so what is observed is the wire, not the vendor client's conveniences. Failure injection uses only public interfaces: `SIGKILL` on supervisor or worker pids reported by the daemon itself, abrupt socket destruction, and command envelopes with fixed `clientId + commandId`. No failpoint, patch, or private API was needed, so no spike code was added to Prime.

Evidence per scenario is a `.log` with PASS/FAIL lines plus a `.wire.jsonl` of every envelope in and out. Wire logs are kept out of the repository because `create`/`attach` envelopes carry the client environment (see S0 finding 4); the harness now redacts secret-shaped keys before sending. "Observable work" and "was the model called again" are read from the mock provider's request log, and "did the mutation happen" from a file in the disposable work directory.

The process/state paths used: agent dir `<disposable HOME>/.prime/agent` (sessions, `daemon-workers/<socket-hash>/{<worker>.json,<worker>.recovery.jsonl,<worker>.orphans.jsonl,command-journal.jsonl}`, `session-artifacts/<sessionId>/scheduled-jobs.json`, `session-leases/`, `supervisor-owners/`, `logs/`), socket dir `$TMPDIR/prime-agent-<uid>/{daemon.sock,worker-*.sock}`.

### Discrepancies between the v0.8.1 documentation and observed behaviour

| # | Documented (daemon.md / agent-connection.md / acp.md) | Observed | Weight for Governor |
|---|---|---|---|
| D1 | "After a worker crash, recovery … restores the root under the same active-session ID" | For a **resident** root whose worker process dies, the supervisor sets `workerState: failed`, `lastError: "Waiting for a client with fresh runtime context"` within ~350 ms and never relaunches; `retry_worker` returns the same message. Recovery is a client-issued `create` on the same session path, which reopens the transcript with the same `sessionId` and history (including a `prime-agent.worker_recovery` marker) but a **new active-session id**. Only **client-owned** workers relaunch automatically under the same active id. Root cause in `recoverWorker`: `recoveryCommand` exists only when `ownerClientId` is set; runtime config (provider/model/apiKey) is deliberately never persisted. | High. An unattended resident worker crash leaves the root dead until a client reopens it. Governor must own that reopen loop and must key durable state by `sessionId`, not active-session id. |
| D2 | "A received command without a durable result is reported as uncertain and is not replayed" | True when the **supervisor** dies mid-command (`command_result_uncertain` on retry, effect ≤ 1). When the **worker** dies mid-command, the supervisor catches "Daemon worker socket closed", and `recordResult` stores that **failure as the durable result**; retries replay the stored failure. With the effect already on disk, the client is told the mutation *failed*. | **Critical.** A false "failed" invites a legitimate client retry under a new command id, which executes: duplicate external effect via the client, not the runtime. Governor must treat any worker-loss failure as uncertain itself. |
| D3 | Generation-aware cursors: "A generation change invalidates comparison with the old sequence" (protocol tests emit `reason: event_generation_changed`) | Safety property holds: presenting a dead generation's cursor yields `replay.status: complete, toSequence: 0` in the new generation plus a full snapshot; no stale sequence is honoured. But the live attach path never emitted `reason: event_generation_changed`. | Low. Clients must key on `toCursor.generation`, not on `reason`. |
| D4 | "Concurrent opens return `session_already_active` with the owning active-session ID" | A second daemon client's `create` on an owned path **converges** onto the existing worker and returns the owner's summary (no error). `switch_session` onto the owned path and a one-shot `-p --resume <path>` client are rejected with the typed error naming the owner. No second writer was ever created; transcript bytes unchanged. | Low. Semantics are safe; the convergence is documented one sentence later. |
| D5 | Client-owned modes listed as print / piped stdin / JSON / RPC / `--no-session` | `--mode acp` starts a **resident** worker at process startup; after the ACP process exits it stays alive indefinitely with 0 clients and 0 messages, reported as `activity: working`. | Medium. Each ACP client run leaks a worker plus a Python kernel until `shutdown --force`. Gate S2 must include cleanup. |
| D6 | `ack_result` is a command | `ack_result` never gets a response envelope (supervisor returns `undefined`); Prime's own client fires and forgets. After ack the same `clientId+commandId` is admitted as new work and executes again (documented compaction; worth stating loudly). | Low. |
| D7 | Global `--daemon-socket` run option | Not accepted by `status`, `list`, `shutdown` subcommands; `shutdown` without a TTY requires `--force`. | Low. |
| D8 | Client-owned recovery "restores the root under the same active-session ID" | True for the id and the process (relaunch in ~1.7 s). But a client-owned session created **without** an explicit `sessionPath` came back with an **empty live transcript** although its JSONL on disk holds the pre-crash turns plus the recovery marker; created **with** `sessionPath`, the relaunch resumes the file (5 messages, marker included). | Medium. Governor must always create with an explicit `sessionPath`. |
| D9 | "The parent-scoped child registry survives compaction, kernel restart, and parent restoration" | After a resident parent crash + client reopen, kernel-side `rlm.list_subagents()` lists the child (`completed`) and the daemon roster (`list all`) shows the same `rlmChildId` and child `sessionId` adopted under the new parent, no duplicate. But the daemon command `get_rlm_children` on the reopened parent returns `{children: [], eventSequence: 0}`. | Low. Read the roster or the kernel, not `get_rlm_children`, after a reopen. |
| D10 | `activity` reflects work | Two roots that had a bash/tool operation in flight when their worker died showed `activity: "working"` for 3+ minutes after recovery/reopen with `isStreaming:false`, no bash, no children, empty queue. Same symptom on the ACP-orphaned root. | Low, but Governor health checks must not trust `activity` alone. |
| D11 | Clean shutdown | `shutdown --force` stopped all 58 processes, but 13 `worker-*.sock` files from crashed workers remained in `$TMPDIR/prime-agent-<uid>/`. | Cosmetic. |

## Per-check evidence (from the scenario logs)

### s1-01-client-detach — **PASS** (5 pass / 0 fail)

- ✅ worker kept consuming the model stream after client detach (no client-gone, 1 request, completed)
- ✅ worker survived with zero attached clients
- ✅ reattached to the exact same session identity
- ✅ completed assistant turn visible after reattach (tick29 present, not streaming)
- ✅ replay info returned for the presented cursor  
  `{"status":"complete","toSequence":41,"toCursor":{"generation":"fb86b16aa1bb","sequence":41}}`

### s1-02-supervisor-replacement — **PASS** (5 pass / 0 fail)

- ✅ a replacement supervisor came up without any client starting it
- ✅ every root adopted under the same active-session id with the same worker pid
- ✅ no duplicate root workers  
  `{"descriptors":11}`
- ✅ exactly one process listens on the public daemon socket  
  `{"listeners":[44055],"hello":44055}`
- ✅ adopted worker still serves prompts through the new supervisor  
  `[{"type":"text","text":"after-supervisor-replacement"}]`

### s1-03-worker-crash-isolation — **PASS** (7 pass / 0 fail)

- ✅ two roots run in two distinct worker processes
- ✅ root B finished its in-flight turn untouched while A crashed
- ✅ crashed root entered the documented failed state (no auto-relaunch) and was reopened with the same sessionId
- ✅ A transcript carries a visible recovery marker; B transcript has none
- ✅ recovered A keeps its history and serves new prompts
- ✅ B transcript parses cleanly (no contamination)
- ✅ no duplicate registration for A's session after reopen
- ℹ️ observation: active-session id preserved across reopen? false

### s1-04-session-lease — **PASS** (7 pass / 0 fail)

- ✅ (1) contender receives the owning active-session identity (converged, not a second writer)  
  `{"convergedTo":"e689bf4fecfa"}`
- ✅ (1) no additional worker was launched
- ✅ (2) rejected with typed session_already_active naming the owner
- ✅ (3) one-shot client refused rather than writing the owned transcript
- ✅ transcript bytes unchanged by all three contenders  
  `{"before":1606,"after":1606}`
- ✅ transcript still parses line by line
- ✅ owner keeps working after contention

### s1-05-generation-reconnect — **FAIL** (6 pass / 1 fail)

- ✅ (1) supervisor replacement keeps the worker generation; interval reported complete
- ✅ (1) sequence continues monotonically within the same generation  
  `{"generation":"886f9ccbf4a3","sequence":23}`
- ✅ (2) dead-generation cursor is not treated as a live position: replay restarts at the new generation with the snapshot as baseline  
  `{"n0":2}`
- ✅ (3) client-owned worker is relaunched automatically under the same active-session id
- ✅ (3) old generation's sequence is not honoured as a live position (new generation, replay restarts at its baseline)  
  `{"status":"complete","toSequence":0,"toCursor":{"generation":"cef615b56c5d","sequence":0}}`
- ❌ (3) relaunched client-owned worker kept its transcript (pre-crash turn still present)  
  `{"count":0}`
- ✅ (3) relaunched owned worker serves prompts; events carry the new generation  
  `{"generation":"cef615b56c5d","sequence":11}`
- ℹ️ (3) observation: replay.reason absent (docs/protocol tests describe event_generation_changed; the live attach path reports a bare complete/0 baseline)

### s1-06-idempotence — **PASS** (5 pass / 0 fail)

- ✅ repeat returns the stored result
- ✅ no duplicate external effect (file has exactly one line)  
  `{"lines":1}`
- ✅ mutation was journaled (receipt before dispatch, result after)
- ✅ idempotency key is clientId+commandId (another client's same commandId is new work)
- ✅ acknowledged entry is compacted/removed from the journal  
  `["received","result","received","result","acknowledged"]`
- ℹ️ observation: after ack the same clientId+commandId is admitted as new work (documented: ack lets the journal compact); lines now 3

### s1-07-uncertain — **FAIL** (7 pass / 2 fail)

- ✅ (a) retry is reported uncertain, not re-executed
- ✅ (a) target mutated at most once and never again on retries
- ✅ (a) journal holds the receipt without a durable result  
  `["received"]`
- ✅ (a) uncertainty is scoped to the command identity; a new id on the same root executes  
  `{"lines":2}`
- ✅ (b) the mutation was not re-executed on retry  
  `{"lines":0}`
- ✅ (b) retry surfaces the loss explicitly (uncertain code or the stored socket-loss failure), never a fabricated success  
  `Daemon worker socket closed`
- ✅ (c) not re-executed (file still has exactly one line)  
  `{"lines":1}`
- ❌ (c) CRITICAL: an effect that happened is reported as uncertain, not as a definite failure  
  `{"reported":"Daemon worker socket closed","effectOnDisk":1}`
- ❌ (c) consequence: the misreported verdict leads to a duplicate external effect  
  `{"linesAfterClientRetry":2}`
- ℹ️ (b) observation: worker loss mid-command is journaled as a definite failure result, not as command_result_uncertain ["received","result"]

### s1-08-schedule — **PASS** (4 pass / 0 fail)

- ✅ recurring job accepted
- ✅ the interrupted tick was not re-delivered on restart (no immediate replay)  
  `{"deliveredWithin15s":0}`
- ✅ job persisted with an advanced schedule and no dangling dispatch after recovery  
  `{"jobs":[{"id":"01d7d1d0-152e-4881-a6f7-74411ca90db5","status":"active","source":"cron","runtimeKind":"top-level","activeSessionId":"ff7b12c92603","sessionId":"01a05f1d-5af5-7319-9098-0908569f84e8","sessionFile":"/privat`
- ✅ future ticks continue after recovery

### s1-09-child-recovery — **FAIL** (4 pass / 1 fail)

- ✅ exactly one child was registered
- ❌ get_rlm_children on the reopened parent still lists the child (documented: registry survives parent restoration)  
  `{"before":[{"id":"sub-1a90a452","name":"cg-child","status":"done"}],"after":[]}`
- ✅ kernel-side registry lists the child after parent reopen
- ✅ child work was not silently re-run (model saw the child prompt exactly once)  
  `{"childModelCalls":1}`
- ✅ child session identity (sessionId) is stable across parent recovery  
  `{"all0":[{"id":"5d2e58044f6d","child":"sub-1a90a452","pid":53359,"sessionId":"01a05f22-8767-75ad-a420-a0488eb224e9"}],"all1":[{"id":"01a05f22-8767-75ad-a420-a0488eb224e9","child":"sub-1a90a452","pid":53603,"sessionId":"0`

## Scenario verdicts

| S1 scenario (issue wording) | Verdict | One-line evidence |
|---|---|---|
| Client lifetime ≠ worker lifetime | **PASS** | Socket destroyed 1 s into a 15 s model turn; mock saw 1 request, 30 chunks, completed, no client-gone; reattach 20 s later: same `activeSessionId`/`sessionId`/worker pid, `tick29` present, replay `complete`. |
| Supervisor replacement / adoption | **PASS** | `SIGKILL` supervisor → replacement up in 5.7 s, launched by a worker, new generation; all 4 roots adopted with identical (`activeSessionId`, `workerPid`); exactly one process on `daemon.sock`; prompts served. Worker generation and cursors unchanged across the replacement. |
| Worker crash isolation | **PASS** (documented failure path) | Root B's in-flight turn completed untouched; root A → `failed` in 365 ms, transcript got `prime-agent.worker_recovery`, B's did not; reopen restored history under the same `sessionId`; no duplicate registration. Active-session id changed (D1). |
| Session lease / single writer | **PASS** | Second client `create` converged onto the owner (same id/pid, no new worker); `switch_session` and one-shot `-p --resume` rejected with `session_already_active` naming the owner; transcript bytes identical before/after; parses line by line. |
| Generation-aware reconnect | **PASS with note** | Dead-generation cursor never honoured as a position: new generation, `toSequence 0`, full snapshot; supervisor replacement keeps generation and sequence continuity. `reason: event_generation_changed` never emitted (D3). Client-owned relaunch without `sessionPath` returned an empty transcript (D8). |
| Completed command idempotence | **PASS** | Same `clientId+commandId` → byte-identical stored result, file has 1 line; journal `received` then `result`; another client's same `commandId` is new work; after `ack_result` (no response envelope) the id is admitted again. |
| Uncertain mutation no-replay | **FAIL on the reporting half** | Supervisor killed mid-command: retry → `command_result_uncertain`, effect ≤ 1, never replayed, fresh id executes. Worker killed mid-command: runtime never re-executed, **but** the loss was journaled as a definite `success:false "Daemon worker socket closed"` result and replayed as such; with the effect already on disk the client is told it failed, retried under a new id, and the file ended with 2 lines (D2). |
| Scheduled prompt claim-before-delivery | **PASS** | `every 20s` job; first tick at 20.2 s; worker killed 1.5 s into the tick's 10 s model turn; after reopen: 0 re-deliveries within 15 s, job persisted with advanced `nextRunAt` and no dangling dispatch; next tick fired on schedule; `cron_cancel` ok. |
| Child recovery | **PASS with note** | One child (`rlm(...)` from the kernel), model saw its prompt exactly once; after parent crash + reopen the child keeps `rlmChildId` and `sessionId`, is adopted under the new parent, kernel `rlm.list_subagents()` lists it; daemon `get_rlm_children` returns empty (D9). |

Failpoints: none. Every window was reached through public interfaces (signals on daemon-reported pids, socket destruction, fixed command ids, a slow mock model).

## Recommendation

**ACCEPT Prime Agent v0.8.1 as the substrate candidate — conditionally. Do not move ADR 0009 to Accepted until the three blocking conditions below are met and pinned by Tier-1 conformance tests in the foundation PR.**

Why not fall back: on the axis that decided ADR 0009, Prime demonstrably has the machinery — detached supervisor with worker-launched replacement, process-safe leases keyed by canonical path, journaled command ids, `command_result_uncertain` on supervisor loss, claim-before-delivery scheduling that survived a crash, generation-scoped cursors, adoptable children. Upstream Pi v0.84.4 has none of it (no daemon, no ACP); OMP was not a durability candidate. Falling back means Governor rebuilds this layer.

Why not accept outright: two findings cut into the exact semantics ADR 0009 relies on, and both would let a Governor built naively on the stable release do the wrong thing silently.

Blocking conditions (each must have a failing-by-construction Tier-1 test before the pin is production):

1. **D2 — worker-loss ambiguity.** Governor must classify any journaled mutating command that fails with a worker-transport loss (`"Daemon worker socket closed"` today) as *uncertain*, never as a definite failure, and must never auto-retry it under a new command id. In parallel, file upstream: the supervisor should leave the journal entry pending (or record an explicit uncertain marker) instead of `recordResult`-ing a transport failure. If upstream rejects that and the Governor-side classification cannot be made airtight (the error is a string), re-open the substrate decision.
2. **D1 — resident roots do not self-heal.** Governor owns the reopen loop: watch `workerState: failed` + `"Waiting for a client with fresh runtime context"`, reopen with `create(sessionPath)`, and key every durable Governor record by `sessionId` (stable) rather than active-session id (changes on reopen). This is exactly the loadout/lineage invariant the frozen Rust oracle encodes (`feat/session-lineage-loadout-store`).
3. **D8 — always create with an explicit `sessionPath`**, for resident and client-owned sessions alike, so relaunch/reopen resumes the persisted transcript.

Non-blocking follow-ups to carry into S2/S3: ACP-mode worker leak (D5) needs an explicit `shutdown`/`kill` discipline in the ACP conformance harness; the daemon socket carries the full client environment (S0 finding 4) so Gate S3 must treat it as a credential boundary and the Governor client must filter its env; `activity` is not a health signal after recovery (D10); read child rosters from `list all`/kernel, not `get_rlm_children` (D9); pin the three R2 tarball hashes in the component manifest because Prime's own manifest pins URLs.

## Reproduce

Harness, mock provider, and the per-scenario evidence logs are on branch `spike/prime-agent-s0-s1-bakeoff` under `spikes/2026-09-01-prime-agent-bakeoff/`. Wire-level logs are intentionally not committed (they contain the forwarded client environment). Requirements: Node ≥ 22.8, `uv` on PATH, network for the one-time kernel bootstrap, stdin closed on every Prime/Pi/OMP invocation.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_019B1Q2WCrhkPo2LJfhfvuB4
