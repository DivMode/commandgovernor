---
name: cg-conformance
description: How to bootstrap the pinned Prime Agent substrate, run the Command Governor conformance suite, and read a failure. Use when asked to verify the distribution, re-pin Prime, admit or upgrade a package, or diagnose a bootstrap, pin, package-load or runtime-fixture failure.
---

# Running the Command Governor conformance suite

Command Governor ships no runtime code. The suite verifies the pinned Prime
Agent, the pinned packages and the manifest that binds them, through the
same clients a user runs.

## Bootstrap first

```sh
scripts/bootstrap.sh
```

Fetches the pinned Prime release assets from the immutable GitHub release,
verifies each against both `pins/pins.json` and the release's own
`pins/SHA256SUMS`, installs the wrapper and its three sibling packages with
`npm ci --ignore-scripts` from the committed lockfile (sha512 integrity), and
asserts `prime-agent --version` is the pinned one. It never installs
globally and needs no `prime-agent` on `PATH`.

The install lands in `pins/prime-<version>/node_modules/` (gitignored); the
verified tarballs in `pins/prime-<version>/vendor/` (gitignored).

## Run the suite

```sh
scripts/conformance.sh
```

Typecheck, then Tier 1 (`conformance/tier1`: pin, package policy, one owner
per concern, JSON), then the runtime tier (`conformance/runtime`), then a
process sweep. Every runtime file starts its own isolated Prime supervisor
under `/tmp/cg-XXXXXX` with a scripted mock model and drives it only
through stock clients (TUI on a pty, `prime-agent -r`, `list --json`,
`--mode rpc`, `-p`, `--mode json`). The D2 effect is a real `ipython` tool
call, so the first run bootstraps Prime's Python kernel (uv + CPython)
into the fixture's own agent directory; `uv` must be reachable and network
is required for that one step.

Set `CG_KEEP_FIXTURE=1` to keep a failed fixture's root for inspection
(supervisor log, mock request log, transcripts under `sessions/`, worker
descriptors under `agent/daemon-workers/`).

## Reading a failure

**`upstream SHA256SUMS differs from the committed pins/SHA256SUMS`** — the
release changed under the pin. Do not update the committed copy to make it
pass; that inverts the check. Find out what was re-published and re-pin
deliberately.

**`checksum mismatch`** — a downloaded asset does not hash to the manifest.
Same rule: never regenerate the hash to match the bytes.

**`pinned prime-agent reports X, pins.json requires Y`** — the install and
the pin record disagree. Both are drift; neither is fixed by relaxing the
assertion. `prime-agent --version` prints on stderr.

**HELLO-001 fails** — the live daemon reports a protocol name, version or
schema revision other than the manifest's. A re-pin crossed a protocol or
schema boundary; go back through ADR 0009's acceptance conditions.

**LOAD-001 fails for a package** — the package did not register on the
pinned Prime. Prime discards extension load errors silently in headless
modes, which is exactly why this probe exists. Do not admit the package;
find the load error with Prime's own loader in an interactive session or
by importing the extension entry, and record the Prime gap under
`docs/upstream/` (the known ones: missing config exports, `ctx.mode`,
`ctx.isProjectTrusted`, the `@earendil-works/pi-ai/*` subpath alias, no
`agent_settled`).

**A D1/D2/D8 assertion fails** — this is the product invariant, and there
is no Command Governor code to fix. Either the pinned Prime regressed
(re-pin backwards and report upstream with the fixture's evidence) or the
stock client behaviour changed (`prime-agent -r <sessionFile>` is the
measured way back into a dead resident root; `attach` is not). Read the
kept fixture's `supervisor.log` and the transcript's
`prime-agent.worker_recovery` entry first.

**`processes referencing a conformance fixture survived the run`** — a
supervisor, worker or kernel outlived the fixture's shutdown. The sweep
kills them and fails on purpose. Note that `ps` cannot see `prime-agent`
processes by command line (Prime sets its process title), so the fixture
sweeps from the pids it recorded and Prime's worker descriptors.

## Re-pinning Prime or upgrading a package

The suite is the gate on a re-pin, not a formality after one. The
procedure is in `docs/prime-distribution.md`; for a package, update
`pins/pins.json` `packages[]` and `harness/settings.project.json` together
(the suite compares them), re-run LOAD-001, and re-read the package's
authority note so it still owns exactly one concern.
