---
name: cg-conformance
description: How to bootstrap the pinned Prime Agent substrate and run the Command Governor conformance suite, and how to read a failure. Use when asked to verify the distribution, re-pin Prime, or diagnose a bootstrap, pin, or runtime-fixture failure.
---

# Running the Command Governor conformance suite

## Bootstrap first

```sh
scripts/bootstrap.sh
```

This fetches the pinned Prime Agent release assets from the immutable GitHub
release, verifies each against both `pins/pins.json` and the release's own
`pins/SHA256SUMS`, installs the wrapper and its three sibling packages with
`npm ci --ignore-scripts` from the committed lockfile (sha512 integrity), and
asserts the resulting `prime-agent --version` is the pinned one. It does not
use, and must not need, a `prime-agent` on `PATH`, and it never installs
globally.

The install lands in `pins/prime-0.8.1/node_modules/` (gitignored); the
verified tarballs in `pins/prime-0.8.1/vendor/` (gitignored).

## Run the suite

```sh
scripts/conformance.sh
```

Typecheck, then Tier 1 pure (`conformance/tier1`), then Tier 1 runtime
(`conformance/runtime`), then a process sweep. Every runtime file starts its
own isolated Prime supervisor under `/tmp/cg-XXXXXX` with a mock model; no
credential is needed and no tool call is ever made, so the Python kernel is
never bootstrapped. The runtime tier runs sequentially because it kills
supervisors and workers on purpose.

Set `CG_KEEP_FIXTURE=1` to keep a failed fixture's root for inspection
(ledger under `governor/<name>/mutations`, registry and recovery leases
under `governor/<name>/sessions`, the journal identity at
`governor/<name>/client-identity.json`, Prime's journal under
`agent/daemon-workers/*/command-journal.jsonl`, the wire log at
`wire.jsonl` with env values redacted).

## Reading a failure

**`upstream SHA256SUMS differs from the committed pins/SHA256SUMS`** — the
release changed under the pin. Do not update the committed copy to make it
pass; that inverts the check. Find out what was re-published and re-pin
deliberately.

**`checksum mismatch`** — a downloaded asset does not hash to the manifest.
Same rule: never regenerate the hash to match the bytes.

**`pinned prime-agent reports X, pins.json requires Y`** — the install and
the pin record disagree. Both are drift; neither is fixed by relaxing the
assertion. Note that `prime-agent --version` prints on stderr.

**`daemon reports appVersion X; pin requires Y`** (`SubstrateMismatch`) — a
Governor refused to speak to a daemon that is not the pinned one. Nothing
was sent to it.

**`TMPDIR ... is too long for Prime's worker sockets`** — macOS caps Unix
socket paths at 104 bytes and Prime's worker sockets sit 50 bytes below
`TMPDIR`. Use a short directory; the fixture already does.

**`processes referencing a conformance fixture survived the run`** — a
supervisor or worker outlived `shutdown`. The sweep kills them and fails,
on purpose. Read the kept fixture's `supervisor.log`.

**A D2 assertion fails with `verdict: failed`** — the classifier produced a
definite failure without a typed pre-effect code. That is the invariant
the whole layer exists for; do not weaken `DEFAULT_POLICY`. Read
`docs/prime-native/adaptation-layer.md` first.

## Re-pinning Prime

The conformance suite is the gate on a re-pin, not a formality after one.
The procedure, including the mandatory re-read of the D2 code path, is in
`docs/prime-distribution.md`.
