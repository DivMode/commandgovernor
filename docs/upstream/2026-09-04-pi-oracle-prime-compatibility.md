# pi-oracle does not load on hosts that export a smaller `pi-coding-agent` surface (repro: Prime Agent 0.9.1)

**Target:** https://github.com/fitchmultz/pi-oracle/issues/new
**Affected version:** `pi-oracle@0.7.20` (tag `v0.7.20`, commit `b26f56106fb362b849fdb55dc14d9bc6fb9b28d1`)
**Host used for the repro:** `prime-agent@0.9.1` (PrimeIntellect-ai/prime-agent), a host that
implements the pi extension contract and aliases `@earendil-works/pi-coding-agent` to its own
entrypoint.
**Patch:** attached below / `pi-oracle-prime-compat.patch` — 21 changed lines across 5 files plus
one new module. No behaviour change on the reference build.

---

## Summary

`pi-oracle` fails to load on Prime Agent 0.9.1. **No tool is registered and no error is shown** —
the host swallows the extension load failure, so from the user's side oracle is simply absent.

There are three independent causes. Two are in `pi-oracle` (it imports optional host symbols as
runtime values, and it branches on an optional `ctx` field). One is in the host, filed separately
against Prime, but `pi-oracle` can defend against it cheaply and I think should.

I am not asking you to support Prime specifically. The pattern that bites here — "assume the host
exports exactly the reference surface" — will bite on any host or any future reference version
that trims an export, and the fix is small and behaviour-preserving.

---

## Repro

Isolated, credential-free. Every path below is inside a disposable root; nothing global is
installed.

```bash
ROOT=$(mktemp -d /tmp/cg-foreman-XXXXXX)
mkdir -p "$ROOT"/{home,tmp,agent,sessions} "$ROOT/proj/.prime/agent"
echo '{}' > "$ROOT/proj/.prime/agent/settings.json"
export HOME="$ROOT/home" TMPDIR="$ROOT/tmp" \
       PRIME_AGENT_CODING_AGENT_DIR="$ROOT/agent" \
       PRIME_AGENT_SESSION_DIR="$ROOT/sessions" \
       PRIME_AGENT_TELEMETRY=0 PRIME_AGENT_INSTALL_UV=0

cd "$ROOT/proj"
prime-agent package install npm:pi-oracle@0.7.20 --local
prime-agent --print --provider <any> --model <any> "hello"
```

Observed: the run succeeds and prints a normal answer. **No oracle tool is registered**, and
`grep -rl oracle "$ROOT/agent/logs/"` matches nothing — stdout, stderr and every host log are
silent.

I measured the registered tool set from the wire rather than from a banner, by pointing the host
at a local OpenAI-compatible mock that logs the full `tools` array of every request:

```
request toolCount=1 tools=['ipython']
```

**Control** — a ten-line extension in the same auto-discovery directory does register, so the host
loads extensions fine and this is specific to `pi-oracle`:

```
request toolCount=2 tools=['ipython','cg_control_probe']
```

---

## Cause 1 — optional host symbols imported as runtime values (hard failure)

`extensions/oracle/lib/runtime.ts:11` and `extensions/oracle/lib/config.ts:9` import
`CONFIG_DIR_NAME`, `ProjectTrustStore` and `hasTrustRequiringProjectResources` as **values**.
Prime 0.9.1's entrypoint exports only `getAgentDir` and `VERSION` from its config module, so all
three resolve to `undefined`:

```
prime-agent index export CONFIG_DIR_NAME                    = undefined
prime-agent index export getAgentDir                        = function
prime-agent index export ProjectTrustStore                  = undefined
prime-agent index export hasTrustRequiringProjectResources  = undefined
```

`extensions/oracle/lib/runtime.ts:44-48` then evaluates, at module scope:

```ts
const WORKSPACE_ROOT_MARKERS = [
  join(CONFIG_DIR_NAME, "extensions", "oracle.json"),
  CONFIG_DIR_NAME,
  "AGENTS.md",
] as const;
```

Reproducing the load with the host's own jiti alias map gives the exact failure:

```
LOAD FAILED
TypeError [ERR_INVALID_ARG_TYPE]: The "path" argument must be of type string. Received undefined
    at join (node:path:1339:7)
    at .../pi-oracle/extensions/oracle/lib/runtime.ts:45:20
    at .../pi-oracle/extensions/oracle/lib/config.ts:20:16
    at .../pi-oracle/extensions/oracle/index.ts:10:15
```

Because it throws during module evaluation, nothing in `index.ts` runs and no tool is registered.

## Cause 2 — `ctx.mode` is assumed to exist

`extensions/oracle/index.ts` branches on `ctx.mode` at lines 84, 117 and 129. Prime 0.9.1 does not
put `mode` on `ExtensionContext` at all. Probed from a minimal extension in `session_start`:

```json
{"mode":"undefined","modeType":"undefined",
 "ctxKeys":"abort,compact,cwd,getContextUsage,getSystemPrompt,hasPendingMessages,
            hasUI,isIdle,model,modelRegistry,sessionManager,shutdown,signal,ui"}
```

With `mode` undefined:

- `index.ts:84` — the `print`/`json` guard never matches, so the poller starts in a
  non-interactive run;
- `index.ts:117` — `["print","json","rpc"].includes(undefined)` is false, so
  `resources_discover` never contributes `prompts/`, and `/oracle` and `/oracle-followup` are
  unusable in **every** mode on such a host (the TUI interceptor at `:129` is also inert);
- `index.ts:129` — inert, which is the safe branch, so no fix is needed there.

## Cause 3 — `ctx.ui.theme` reached behind a `ctx.hasUI` guard

`pi-oracle` guards correctly (`lib/poller.ts:164`, `if (!snapshot.hasUI) return;`). The host lies:
Prime 0.9.1 reports `hasUI === true` in `--print` and `--mode json` while its theme is
uninitialised, so touching `ctx.ui.theme` throws. Filed against Prime separately.

With cause 1 worked around locally, cause 3 is fatal on its own — the status refresh runs from an
async lifecycle callback, so the throw becomes an unhandled rejection and takes the host's session
worker down:

```
Error: Daemon worker socket closed
```
```
unhandled rejection: Error: Theme not initialized. Call initTheme() first.
    at refreshOracleStatusSnapshot (.../lib/poller.ts:175:55)
    at setOracleReadiness           (.../lib/poller.ts:188:3)
    at                              (.../index.ts:98:55)
```

`index.ts:91` has the same shape (`if (ctx.hasUI) ctx.ui.setStatus(..., ctx.ui.theme.fg(...))`).

---

## Patch

All optional host capability now lives behind a new `extensions/oracle/lib/host-compat.ts`, which
**always prefers the host's own value** and only falls back when the host omits one:

- `CONFIG_DIR_NAME` — the host's export when present; otherwise derived from the host's own
  default agent dir (read with any `*_CODING_AGENT_DIR` override temporarily neutralised, so a
  relocated agent dir cannot corrupt the project-relative name); otherwise `.pi`.
- `hasTrustRequiringProjectResources` — the host's function when present; otherwise `false`,
  which keeps the caller on its historical "project config loads" path.
- `createProjectTrustStore()` — returns `undefined` on a host with no project-trust model, and
  `isProjectConfigTrusted` then returns `true` rather than failing closed. This matches
  `lib/trust.ts`'s existing `ctx.isProjectTrusted?.() ?? true` default and the documented
  "`.pi/extensions/oracle.json` loads by default for compatibility" behaviour.
- `isNonInteractiveHost(ctx)` — **when the host declares a mode the historical rule is byte-for-byte
  preserved** (`print`/`json` only). Only when `mode` is absent does it fall back to "can this
  context render themed UI?".
- `needsPromptTemplateFallback(ctx)` — same: the existing `["print","json","rpc"]` list when a
  mode is declared; `true` when it is not, because the TUI interception branch cannot run there.
- `canRenderThemedUi(ctx)` — `ctx.hasUI === true` **and** a probed `theme.fg()` call that does not
  throw. Used at `index.ts:91`, `lib/poller.ts:142` and `lib/commands.ts:84`.

Diffstat:

```
 extensions/oracle/index.ts           |   7 +-
 extensions/oracle/lib/commands.ts    |   3 +-
 extensions/oracle/lib/config.ts      |  12 ++-
 extensions/oracle/lib/host-compat.ts | 190 +++++++++++++++++++++++++++++++++++
 extensions/oracle/lib/poller.ts      |   5 +-
 extensions/oracle/lib/runtime.ts     |   2 +-
 6 files changed, 211 insertions(+), 8 deletions(-)
```

### Verification against the reference build (`@earendil-works/pi-coding-agent@0.80.9`)

That build exports all four symbols (`CONFIG_DIR_NAME === ".pi"`), so every feature-detect takes
its first branch and nothing changes.

```
npx tsc --noEmit -p tsconfig.json   -> exit 0
npm run typecheck:worker-helpers    -> exit 0
npm run check:oracle-extension      -> exit 0   (esbuild bundle 225.8kb)
npm run sanity:oracle               -> fails IDENTICALLY before and after the patch
```

On `sanity:oracle`: this machine has neither `zstd` nor `agent-browser`, so the suite cannot go
green here. I ran it on the pristine tag and on the patched tree with the same stubs and the same
working directory, and both stop at the same assertion with the same stack —
`whole-repo archive selection from a subdirectory should still include workspace-root files`,
caused by my no-op `zstd` stub. **No behavioural difference is attributable to the patch.** If you
have a machine with real `zstd` I would appreciate a confirming green run.

### Verification on the host that could not load it

Packed with `npm pack`, production-installed (`npm install --omit=dev`, so the only dependency
present is `@steipete/sweet-cookie` and the host alias is the sole resolution path for
`@earendil-works/pi-coding-agent`), then installed by local path:

```
request toolCount=6 tools=['ipython','oracle_preflight','oracle_auth','oracle_submit',
                           'oracle_read','oracle_cancel']
```

No shims, no daemon crash. End to end on that host, unauthenticated:

- `oracle_preflight{chatGptConversationId:"https://chatgpt.com/c/<id>"}` →
  `auth_seed_profile_missing` with the correct isolated seed path;
- `oracle_submit{...}` → job dispatched, `job.json` records the requested conversation exactly
  (`chatUrl`, `conversationId`), detached worker reparented to init;
- `SIGKILL` of the worker mid-`launching_browser` → `job.json` and `lifecycleEvents` intact;
- next `oracle_submit` → the stale job is recovered to `failed`
  (`Recovered stale job: Oracle worker PID 58215 is no longer running.`) and its conversation
  lease is released so the next delivery is admitted.

---

## Three smaller things found on the way, reported separately from the patch

I have deliberately **not** folded these into the patch — they are pre-existing, host-independent,
and each deserves its own decision.

**1. `worker/run-job.mjs` references an undeclared `ORACLE_JOBS_DIR`.**
The module imports `getOracleJobsDir` (`:22`, used at `:54`) but then uses the bare identifier
`ORACLE_JOBS_DIR` at **`:148`, `:166` and `:167`**, where it is never declared or imported. Those
are `readAnyJob` and `listQueuedJobs`, i.e. the worker's queued-job promotion path. Observed live:

```
[2026-09-04T23:25:20.456Z] Queued cleanup promotion warning: ORACLE_JOBS_DIR is not defined
```

Effect: a finishing worker never promotes a queued job; promotion only happens from the extension
side at session start or on the next `oracle_submit`.

**2. Stale-job reconciliation never runs in a non-interactive session.**
`runStartupMaintenance()` — the only caller of `reconcileStaleOracleJobs()` outside
`oracle_submit` — sits inside `startPollerForContext()` **after** the early return for
`print`/`json` (`index.ts:84-100`). So on `pi --print` / `pi --json`, a job whose worker died is
never recovered; it stays `waiting` forever and keeps its runtime and conversation leases.

Measured: after `SIGKILL`ing a worker in `launching_browser`, three consecutive non-interactive
sessions left the job at `waiting`/`launching_browser`, and `oracle_read` reported
`heartbeat: fresh (2m 38s ago)` — i.e. the surface says healthy while the worker is gone.
`readProcessStartedAt(pid)` correctly returns `undefined` and `isTrackedProcessAlive` correctly
returns `false`, so the detection logic is fine; it is simply never called. The next
`oracle_submit` recovers it immediately.

Suggested fix, if you agree it is one: hoist `runStartupMaintenance(ctx)` above the interactive
early return, so recovery is independent of whether the poller runs.

**3. A hard-killed worker orphans its browser.**
`SIGKILL` skips the `finally` cleanup in `worker/run-job.mjs`, leaving the isolated Chrome and its
helper processes running (10 processes in my run) and both leases held until something calls
reconcile. Combined with (2), on a non-interactive host that is indefinite. A reaper keyed on the
runtime profile dir at reconcile time would close this.

---

Happy to split the patch, adjust naming, or drop the `lib/commands.ts` hunk if you would rather
keep `ctx.hasUI` there. Thanks for pi-oracle — the job store, the conversation lease and the
`lifecycleEvents` breadcrumbs are exactly the right primitives, and the lease in particular is the
reason this package was the first one I evaluated.
