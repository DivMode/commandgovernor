# Upstream record: Prime Agent 0.9.1 gaps found by the 2026-09-04 zero-custom-code proof

Status: **drafted, not yet filed.** Prime routes external bug reports through
GitHub Discussions (Bug reports category); maintainers promote a Discussion
to an Issue when they accept it. The Command Governor D2 report is already
[discussion #1978](https://github.com/PrimeIntellect-ai/prime-agent/discussions/1978)
(open, no replies as of 2026-09-04; see
[`2026-09-01-prime-worker-loss-journal.md`](2026-09-01-prime-worker-loss-journal.md)).

Every item below was measured on the pinned build (`v0.9.1`,
`81ae3cb34d27d38ee37f9e205a1e73694993b344`) in isolated roots with a
credential-free scripted model. None of them is worked around by Command
Governor code: each is either a reporting defect that duplicates nothing, a
gap that only Prime can close, or a limitation that a package or an OS
sandbox must own. The reproducers live with the proof
(`docs/research/2026-09-04-zero-custom-code-proof.md`).

## A. Extension-facing surface (blocks third-party Pi packages)

Prime aliases `@earendil-works/pi-coding-agent` to its own entry
(`dist/core/extensions/loader.js:41-42`, `dist/core/extensions/bundled-modules.js`).
`dist/index.d.ts:1` re-exports only `{ getAgentDir, VERSION }` from
`config.js`. Packages written for upstream Pi import runtime values that are
defined in Prime but not exported, and fail at module evaluation:

| Missing on the extension surface | Defined in Prime at | Package that breaks |
| --- | --- | --- |
| `getPackageDir` | `dist/config.js:275` | `@gotgenes/pi-permission-system` 31.1.0 (`src/index.ts:70`) |
| `CONFIG_DIR_NAME` | `dist/config.js:385` | `pi-oracle` 0.7.20 (`lib/runtime.ts:11`, `lib/config.ts:9`) |
| `ProjectTrustStore`, `hasTrustRequiringProjectResources` | (trust store internals) | `pi-oracle` 0.7.20 (`lib/config.ts:9`) |
| `ctx.mode` on `ExtensionContext` | absent | `pi-oracle` (three branches) |
| `ctx.isProjectTrusted()` on `ExtensionContext` | absent | `@gotgenes/pi-permission-system` (three call sites) |

Proposed: export the config helpers that Pi exports, and add the two
`ExtensionContext` members (or document their absence so packages can
feature-detect). The attached
[`2026-09-04-pi-oracle-prime-compat.patch`](2026-09-04-pi-oracle-prime-compat.patch)
shows the package-side feature-detection alternative for pi-oracle.

## B. Extension load failures are invisible in headless modes

`resource-loader.js` collects `loadExtensions().errors`; the only consumer is
`dist/modes/agent-connection/snapshot.js:120`, which renders them to an
attached interactive client. In `-p`, `--mode json` and `--mode rpc` a
package that throws at load is simply absent: exit 0, empty stderr, even
with `--verbose`. Measured for both packages in table A. For a permission or
policy extension this is the worst failure mode: the operator believes a
gate is running.

Proposed: print load errors to stderr in every mode, and expose them on the
daemon `list`/`status` surfaces.

## C. `ctx.hasUI` is `true` in `-p` and `--mode json`

`docs/extensions.md` ("Mode Behavior") says UI methods are no-ops in print
and JSON modes and tells extensions to check `ctx.hasUI`. Measured:
`ctx.hasUI === true` in both, and `ctx.ui.theme` then throws
`Theme not initialized`. An extension that touches the theme after checking
`hasUI` raises an unhandled rejection inside the daemon worker and the worker
dies (`pi-oracle` did; minimal repro in
[`2026-09-04-prime-hasui-theme-crash.md`](2026-09-04-prime-hasui-theme-crash.md)).
A permission extension that uses `hasUI` to decide whether a human can be
asked picks the interactive authorizer in a headless run; the denial then
depends on a deeper daemon fallback and is logged as a user decision.

## D. No kernel-boundary decision point for `bash()`

Prime's default tool set is `{ ipython }` (`dist/core/tools/index.js:7`).
Shell execution is a Python builtin inside the kernel
(`dist/prime-agent-runtime/src/rlm/bash.py:652`, spawning at `:183`) with no
host round trip; the kernel-to-host bridge (`dist/core/kernel/shared.d.ts:21`,
wired at `dist/core/agent-session.js:7427`) serves `rlm.run`, goals,
agent-observe and MCP, not bash. The only host-side interception point is
the `tool_call` extension event, which sees one opaque `{code}` per cell.

Consequence, measured with a real kernel and the permission package patched
to load: a policy of `bash: {"*": "deny"}` produced no permission entry and
the target directory was deleted. No extension, whether third-party or
Command-Governor-specific, can gate destructive shell work on Prime today.
The options are a host hook for `rlm.bash` (the same bridge shape Prime
already has for `rlm.run`) or OS containment of the kernel process.

Proposed: route `rlm.bash` spawns through a `HostRequestHandler` so an
extension can allow/ask/deny before `subprocess.Popen`. This would make the
existing bash gate of `@gotgenes/pi-permission-system` work unmodified. It
would still not cover `shutil.rmtree`-class Python; that is a sandbox job.

## E. `agent_settled` is absent

Upstream Pi has emitted `agent_settled` (no automatic retry, compaction
retry or queued continuation remains) since 0.80.4. Prime 0.9.1 and `main`
have no such event: `pi.on("agent_settled", ...)` registers and never fires
(the handler map is keyed by raw string with no validation,
`dist/core/extensions/loader.js:137-143`). `pi-squad` treats `agent_settled`
as the only terminal edge and, on Prime, fails every task after five backoff
retries with an error that blames the model provider
(`src/agent-pool.ts:479-484`, `src/scheduler.ts:904`); `pi-subagents` and
`pi-pr-review` degrade silently. `@agimon-ai/doompi-autostop`, `pi-loops`
and `@bacnh85/pi-advisor` are inert.

Proposed: emit `agent_settled` with Pi's semantics, or reject registration
of unknown event names so the failure is loud.

## F. Daemon and CLI defects observed while driving stock clients

1. **Worker transport loss journaled as a definite failure** (the D2
   report, discussion #1978). Still present at 0.9.1 and `main`
   (`daemon-supervisor.js` catch path; `serializeDaemonError` has no case).
   Reachable from a stock client: `prime-agent --mode rpc` + `{"type":"bash"}`
   whose worker dies after the effect returns
   `success:false, error:"Daemon worker socket closed"` with no `errorInfo`.
   Measured consequence: the effect happened once and Prime never re-issued
   it; the defect misreports, it does not duplicate.
2. **`prime-agent attach <agent>` cannot reopen a resident root whose worker
   died.** `docs/long-running-agents.md` names `attach` as the reconnect
   command; after worker loss it reports one of `Session worker is failed`,
   `No active agent found matching '<id>'`, or `connect ENOENT <socket>`
   (`dist/main.js:877-888`). `prime-agent -r <sessionFile>` reopens the same
   `sessionId` every time and is the undocumented way back.
3. **The interactive client crashes with a raw Node stack** when the
   supervisor stops between connect and attach (`DaemonSocketClosedError`,
   `Session worker is stopping`, `Daemon supervisor generation … is shutting
   down`). Intermittent; no data loss observed; the user must retype the
   command.
4. **`prime-agent shutdown` cannot target `--daemon-socket`**
   (`dist/cli/public-command.js:226` rejects it; `runShutdownAll` scans only
   the default socket directory), so a supervisor started with a custom
   socket has no supported stop command, and `shutdown --force` from a shell
   without a per-fixture `TMPDIR` is machine-wide.
5. **`prime-agent --daemon-socket <p> <subcommand>` is sent to the model as
   a chat message** (`normalizeLeadingDaemonSocketOption` rewrites the
   leading flag only for `stop` and `rename`); `list --json` printed the
   model's answer with exit 0.
6. **`./hooks` is a broken package export.** `package.json` declares
   `"./hooks"` → `./dist/core/hooks/index.js`; the path does not exist and
   `import('prime-agent/hooks')` fails with `ERR_MODULE_NOT_FOUND`.

## G. What Command Governor does with each

| Item | Command Governor disposition |
| --- | --- |
| A, B, C | No workaround. Packages are admitted only after the package-load conformance test proves they register on the pinned Prime; a package that needs the patched surface waits for upstream (pi-oracle: patch attached). |
| D | No workaround possible in an extension. Recorded as the substrate's open security limitation for the trusted-local product; OS containment of the kernel is the load-bearing control until a kernel hook exists. |
| E | No workaround. Packages that require `agent_settled` are not admitted until Prime emits it. |
| F1 | No workaround needed: the product path never consumes that response, and the conformance suite asserts the effect happens exactly once. |
| F2–F5 | Documented in `docs/prime-distribution.md`; the conformance fixture uses `-r <sessionFile>` and its own socket shutdown. |
| F6 | Not used. |
