# Prime Agent v0.8.1 substrate bake-off (Gate S0/S1, Issue #15)

Real-machine evidence for proposed ADR 0009. `REPORT.md` is the full report as posted on
Issue #15; `evidence/` holds the per-scenario logs it cites; `harness/` is what produced them.

This is a spike, not product code. Nothing here is loaded by the distribution and nothing
here should be imported. It exists so the S1 scenarios can be re-run against a future Prime
pin (v0.8.2, a patched fork, or an upstream fix for finding D2) and compared line for line.

## Layout

- `harness/mock-provider.mjs` — credential-free OpenAI-compatible mock model. Behaviour is
  chosen by the prompt text (`ECHO:`, `SLOW:n:ms`, `TOOL:name:{json}`); every request is
  logged so "was the model called again" is a fact, not an inference.
- `harness/s1/cgd.mjs` — ~80-line raw client for Prime's public daemon protocol v7 (JSONL over
  the Unix socket). Independent of Prime's own client on purpose. Withholds secret-shaped env
  keys from `launchEnv` (Prime forwards the whole client environment otherwise).
- `harness/s1/common.mjs` — shared helpers, including `recoverRoot`, which encodes the only
  recovery path v0.8.1 offers a resident root (client `create` on the same session path).
- `harness/s1/s1-0*.mjs` — one scenario per Issue #15 S1 bullet. Each writes
  `evidence/<name>.log` (PASS/FAIL lines) and a wire log that is deliberately not committed.
- `harness/s1/run.sh` — sequential runner; `collect.mjs` renders the logs as markdown.
- `harness/cg-marker.ts`, `harness/skill/` — the harmless extension and skill used in S0.

## Re-running

Disposable roots only. Set `CG_S` to a scratch directory containing `bakeoff/prime/{install,home,work}`
with Prime installed from its verified release tarball (`npm install <tgz>` in `install/`,
never `npm install -g` on this machine), `bakeoff/prime/home/.prime/agent/models.json` pointing
at the mock, and the mock running on `127.0.0.1:18765`. Close stdin on every invocation.

```sh
node harness/mock-provider.mjs &            # MOCK_LOG=<CG_S>/bakeoff/mock/requests.jsonl
cd harness/s1 && CG_S=<scratch> ./run.sh s1-0*.mjs
node collect.mjs
```

Scenarios that kill the supervisor or workers must run sequentially. Finish with
`prime-agent shutdown --force` and check `pgrep -f prime-agent` is empty.
