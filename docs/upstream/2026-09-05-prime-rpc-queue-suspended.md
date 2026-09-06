# `--mode rpc` cannot admit any further input after an abort or a compaction — only the daemon protocol can lift it

**Target:** https://github.com/PrimeIntellect-ai/prime-agent/issues/new
**Version:** `prime-agent@0.9.1` (npm), macOS 15 (Darwin 24.6.0), Node 24.19.0

---

## Summary

In `--mode rpc`, a session that has been aborted — or compacted, which aborts
internally — refuses every subsequent command:

```json
{"id":"t3","type":"response","command":"prompt","success":false,
 "error":"Cannot admit a session action while queued session input is suspended."}
```

The suspension is real and intended; the problem is that **`--mode rpc` has no
command that lifts it**. `resumeQueuedWork()` is reachable from outside
`AgentSession` only through the daemon protocol's `resume_queue`, which the rpc
command table does not have. `agent_messages_resume` looks like the right
command and is not: it resumes *agent messaging*, not the session input pump.

So an rpc client is a one-way door. The first abort ends the session for
practical purposes, and because `compact()` aborts on the way in, so does the
first `/compact` — even a completely successful one. The interactive client
recovers from both, which is why this is easy to miss.

I hit this twice: once building a cache measurement across an interruption, and
again writing a conformance test for compaction. Both had to be rewritten onto
a pty-driven interactive client.

---

## Minimal repro

No extension and no special provider is needed; any model that takes a few
seconds to answer will do.

```bash
ROOT=$(mktemp -d /tmp/prime-rpc-XXXXXX)
mkdir -p "$ROOT"/{home,tmp,agent,sessions,proj}
export HOME="$ROOT/home" TMPDIR="$ROOT/tmp" \
       PRIME_AGENT_CODING_AGENT_DIR="$ROOT/agent" \
       PRIME_AGENT_TELEMETRY=0
cd "$ROOT/proj"

# Case 1 — after an abort
{
  echo '{"id":"t1","type":"prompt","message":"Count slowly from 1 to 400, one number word per line."}'
  sleep 8
  echo '{"id":"x","type":"abort"}'
  sleep 5
  echo '{"id":"r","type":"agent_messages_resume"}'
  sleep 3
  echo '{"id":"t2","type":"prompt","message":"Reply with exactly: DONE"}'
  sleep 30
} | prime-agent --mode rpc --provider <any> --model <any> --session-dir "$ROOT/sessions"

# Case 2 — after a SUCCESSFUL compaction, with no abort anywhere
{
  echo '{"id":"t1","type":"prompt","message":"Reply with exactly: T1"}'
  sleep 25
  echo '{"id":"t2","type":"prompt","message":"Reply with exactly: T2"}'
  sleep 25
  echo '{"id":"c","type":"compact"}'
  sleep 60
  echo '{"id":"t3","type":"prompt","message":"Reply with exactly: DONE"}'
  sleep 30
} | prime-agent --mode rpc --provider <any> --model <any> --session-dir "$ROOT/sessions"
```

(For case 2, lower `compaction.keepRecentTokens` in
`<proj>/.prime/agent/settings.json` so a two-turn session is compactable —
otherwise the compaction is refused as "too short" and the session survives.)

### Actual output

Case 1, after the abort:

```json
{"id":"r","type":"response","command":"agent_messages_resume","success":true}
{"id":"t2","type":"response","command":"prompt","success":false,
 "error":"Cannot admit a session action while queued session input is suspended."}
```

`agent_messages_resume` succeeds and changes nothing.

Case 2 — the compaction genuinely worked, and the session is still dead:

```json
{"type":"compaction_start","reason":"manual"}
{"type":"compaction_end","reason":"manual","result":{"summary":"## Goal ...","firstKeptEntryId":"0244f5fe","tokensBefore":5329,...},"aborted":false,"willRetry":false}
{"id":"compact","type":"response","command":"compact","success":true,"data":{"summary":"## Goal ..."}}
{"id":"t3","type":"response","command":"prompt","success":false,
 "error":"Cannot admit a session action while queued session input is suspended."}
```

Observed on 2026-09-05: case 1 in three separate runs, case 2 in one run of a
conformance fixture (which is why that fixture now drives a pty instead).

### Expected

Either of:

- `--mode rpc` gains the equivalent of the daemon's `resume_queue`, so a client
  can lift the suspension it is told about; or
- the rpc `prompt`/`steer`/`follow_up` handlers resume the input pump the way
  the interactive path does, so a user-initiated turn after an abort or a
  compaction just works.

The second matches what an rpc client can reasonably expect: the abort was its
own command, and it has already been told the turn ended.

---

## Where it comes from, in 0.9.1's own source

Paths are inside the published package (`prime-agent@0.9.1`).

1. **The guard.** `dist/core/agent-session.js`,
   `_assertSessionActionAdmissionAvailable()`:

   ```js
   if (this._sessionInputPumpSuspended) {
       throw new Error("Cannot admit a session action while queued session input is suspended.");
   }
   ```

2. **What sets it.** `_sessionInputPumpSuspended = true` is assigned in exactly
   two places in that file: `requestAbort()` and `abortForUpdateRestart()`.

3. **Why a compaction sets it too.** `AgentSession.compact()`:

   ```js
   this._disconnectFromAgent();
   if (!options.skipAbort)
       await this.abort();
   ```

   and `abort()` calls `requestAbort()`. So a manual `/compact` — which the rpc
   `compact` command reaches through
   `in-process-agent-connection.js`'s `compact()` → `session.compact()`, with no
   `skipAbort` — leaves the pump suspended even when the compaction succeeds.

4. **What clears it.** `_resumeSessionInputAdmission()` is called from
   `resumeQueuedWork()`, from two queued-message mutation paths, and from
   `_admitSessionInput` for an already-admitted immediate turn — i.e. *after*
   the guard in (1) has already thrown for a fresh `prompt`.

5. **Who can call `resumeQueuedWork()` from outside the session.** One caller in
   the whole package: `dist/modes/daemon/daemon-mode.js`

   ```js
   case "resume_queue": {
       if (!state.runtime.session.resumeQueuedWork()) { ... }
   ```

   `resume_queue` appears in `dist/modes/daemon/daemon-protocol.{js,d.ts}`,
   `daemon-mode.js`, `daemon-supervisor.js` and `dist/package-manager-cli.js`
   (which sends it after an update restart). It does **not** appear in
   `dist/modes/rpc/`.

6. **What rpc does have.** The command switch in `dist/modes/rpc/rpc-mode.js`
   covers `prompt`, `steer`, `follow_up`, `abort`, `compact`, `refine`,
   `agent_messages_{status,pause,resume,clear}` and ~30 others — and nothing
   that reaches `resumeQueuedWork()`. `agent_messages_resume` routes to the
   agent-messaging lane, which is a different queue.

---

## Severity and blast radius

Anything that drives Prime programmatically over `--mode rpc` — a harness, a
test fixture, a CI job, an editor integration — loses the session on its first
abort, and on its first successful compaction. Compaction is the case that
matters most, because it is not an error path: a long-running rpc client that
compacts on schedule to stay inside its context window will compact
successfully and then be unable to send another prompt.

The workaround is to drive the interactive client on a pty instead, which is
what our conformance fixture now does — but a pty is a poor interface for a
program, and the JSON event stream that makes rpc worth using is exactly what
is lost.

Happy to send a PR for either shape if you say which you prefer.
