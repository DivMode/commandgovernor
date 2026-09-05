# `ctx.hasUI === true` in `--print`/`--mode json` while `ctx.ui.theme` throws — kills the daemon worker

**Target:** https://github.com/PrimeIntellect-ai/prime-agent/issues/new
**Version:** `prime-agent@0.9.1` (npm), macOS 15 (Darwin 24.6.0), Node 24.19.0

---

## Summary

In `--print` and `--mode json`, `ExtensionContext.hasUI` is `true`, but the theme behind
`ctx.ui.theme` is never initialised, so **reading the `theme` property throws**:

```
Error: Theme not initialized. Call initTheme() first.
```

`hasUI` is the documented signal for "this context has a UI", and extensions guard themed output
with it. Because extension lifecycle handlers are async, a throw from inside one becomes an
**unhandled rejection**, and that takes the daemon worker down:

```
Error: Daemon worker socket closed
```

So a well-written extension that does exactly what the docs suggest can kill a non-interactive
Prime session. I hit this with a real third-party extension (`pi-oracle`) before reducing it to
the minimal case below.

A second, related observation in the same probe: **`ctx.mode` is not present on
`ExtensionContext` at all** in 0.9.1, though `docs/extensions.md` documents a "Mode Behavior"
section and extensions written for the inherited ecosystem branch on it. That is what sends such
an extension down the interactive path in the first place. I am reporting it here because the two
together are what produce the crash; happy to split it out.

---

## Minimal repro

`ctxprobe.ts`, placed in `<project>/.prime/agent/extensions/`:

```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { appendFileSync } from "node:fs";

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_e, ctx) => {
    let themeErr = "none";
    try {
      (ctx as any).ui.theme.fg("accent", "x");
    } catch (e) {
      themeErr = String((e as Error).message);
    }
    appendFileSync(process.env.CG_PROBE_LOG!, JSON.stringify({
      mode: String(ctx.mode),
      modeType: typeof ctx.mode,
      ctxKeys: Object.keys(ctx as any).sort().join(","),
      hasUI: ctx.hasUI,
      hasTheme: typeof (ctx as any).ui?.theme,
      themeErr,
    }) + "\n");
  });
}
```

Run it in an isolated root (nothing global is touched):

```bash
ROOT=$(mktemp -d /tmp/prime-probe-XXXXXX)
mkdir -p "$ROOT"/{home,tmp,agent,sessions} "$ROOT/proj/.prime/agent/extensions"
export HOME="$ROOT/home" TMPDIR="$ROOT/tmp" \
       PRIME_AGENT_CODING_AGENT_DIR="$ROOT/agent" \
       PRIME_AGENT_SESSION_DIR="$ROOT/sessions" \
       PRIME_AGENT_TELEMETRY=0 CG_PROBE_LOG="$ROOT/probe.jsonl"
cp ctxprobe.ts "$ROOT/proj/.prime/agent/extensions/"
cd "$ROOT/proj"
prime-agent --print    --provider <any> --model <any> "hi" </dev/null >/dev/null 2>&1
prime-agent --mode json --provider <any> --model <any> "hi" </dev/null >/dev/null 2>&1
cat "$ROOT/probe.jsonl"
```

### Actual output — identical in both modes

```json
{
  "mode": "undefined",
  "modeType": "undefined",
  "ctxKeys": "abort,compact,cwd,getContextUsage,getSystemPrompt,hasPendingMessages,isIdle,hasUI,model,modelRegistry,sessionManager,shutdown,signal,ui",
  "hasUI": true,
  "hasTheme": "object",
  "themeErr": "Theme not initialized. Call initTheme() first."
}
```

Note `mode` is absent from `ctxKeys` entirely — it is not merely undefined-valued.

### Expected

One of:

- `hasUI === false` in non-interactive modes; or
- `ctx.ui.theme` usable (or a no-op/plain-text theme) wherever `hasUI === true`; or
- theme access that degrades to plain text instead of throwing.

Any of the three makes the documented guard sound. My preference is the second or third, since
`hasUI === true` is arguably correct for a mode that still renders *something*, and extensions
should not have to probe.

---

## The crash it causes in practice

`pi-oracle@0.7.20` guards correctly — `lib/poller.ts:164` is `if (!snapshot.hasUI) return;` — and
still dies, because the guard is given `true`. From the daemon worker log:

```
[2026-09-04T23:22:20.926Z] Prime Agent daemon listening on .../worker-8647b62322e6-73873bc89492.sock
[2026-09-04T23:22:21.829Z] unhandled rejection: Error: Theme not initialized. Call initTheme() first.
    at Object.get (.../prime-agent/dist/bundle/chunk-MMG2DX73.js:28545:13)
    at refreshOracleStatusSnapshot (.../pi-oracle/extensions/oracle/lib/poller.ts:175:55)
    at setOracleReadiness           (.../pi-oracle/extensions/oracle/lib/poller.ts:188:3)
    at                              (.../pi-oracle/extensions/oracle/index.ts:98:55)
```

and the client side:

```
Error: Daemon worker socket closed
```

The throw originates in a property getter (`Object.get`), so `typeof ctx.ui.theme` does not help —
an extension has to wrap the *access itself* in `try`/`catch` to find out.

---

## Severity and blast radius

Any extension that renders themed status behind a `hasUI` guard will crash a non-interactive
Prime session. That is the pattern `docs/extensions.md` steers authors toward
("**User interaction** — Prompt users via `ctx.ui`"), and it is common in the inherited extension
ecosystem that Prime explicitly aims to be compatible with (`docs/packages.md`: "For compatibility
with the inherited extension ecosystem, a package declares resources in `package.json` under the
`pi` key"). It is not specific to `pi-oracle`.

---

## Secondary: extension load failures are silent

While reducing this I also found that when an extension throws during module evaluation, **nothing
is reported anywhere**: not stdout, not stderr, not `--mode json`, and not any file under the
agent dir (`grep -rl '<extension name>' $AGENT_DIR/logs/` matched nothing). The only symptom is
that the extension's tools are absent from the model's tool list.

A one-line warning on stderr, or a diagnostic in the `resources_discover` result, would have
turned a multi-hour investigation into a ten-second one. `LoadExtensionsResult` already appears to
carry error information — surfacing it in non-interactive modes would be enough.

Happy to send a PR for any of these if you point me at the preferred shape.
