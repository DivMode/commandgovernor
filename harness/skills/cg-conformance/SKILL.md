---
name: cg-conformance
description: How to bootstrap the pinned Pi runtime and run the Command Governor conformance suite, and how to read a failure. Use when asked to verify the distribution, re-pin Pi, or diagnose a bootstrap, pin, or resource-loading failure.
---

# Running the Command Governor conformance suite

## Bootstrap first

```sh
scripts/bootstrap.sh
```

This verifies the committed pin assets against `pins/SHA256SUMS`, installs the
pinned Pi from upstream's own installer lockfile with `npm ci --ignore-scripts`,
and asserts the resulting binary reports the version recorded in
`pins/pins.json`. It does not use, and must not need, a `pi` on `PATH`.

The install lands in `pins/pi-0.84.4/node_modules/`, which is gitignored.

## Run the suite

```sh
scripts/conformance.sh
```

Tier 1 is credential-free and must pass on every change. Tier 2 needs a real
provider credential and is skipped unless `CG_CONFORMANCE_LIVE=1` is set; its
tests are marked skipped with a stated reason and never reported as passing.

## Launch Pi through the launcher, never directly

```sh
bin/cg-pi [pi arguments...]
```

`bin/cg-pi` performs the version preflight before the process starts, passes
`--approve` so the project loadout is actually loaded, and points
`PI_SUBAGENTS_TEMP_ROOT` at a durable directory. Running `pi` directly skips all
three.

## Reading a failure

**`checksum mismatch`** — a committed pin file no longer matches the release it
claims to be. Do not regenerate the checksum to make it pass; that inverts the
check. Re-download the asset from the pinned release and compare.

**`pinned pi reports X, pins.json requires Y`** — the install and the pin record
disagree. Either `npm ci` resolved something unexpected or the pin record was
edited without re-pinning. Both are drift; neither is fixed by relaxing the
assertion.

**A resource is missing from the resolved command list** — Pi's headless modes
never prompt for project trust and, under the default `defaultProjectTrust`,
silently ignore project resources. Nothing errors; the loadout is simply absent.
Check that `--approve` was passed. This is the single most likely cause of "my
extension did not load".

**`[cg-version-guard] runtime-version-drift`** — a session started against a Pi
that is not the pinned one. The guard blocks every tool call in that session, so
nothing was done under the wrong runtime.

## Re-pinning Pi

The conformance suite is the gate on a re-pin, not a formality after one.
Upstream releases roughly weekly, so this is a scheduled ritual rather than an
ad-hoc upgrade. The procedure is in `docs/pi-distribution.md`.
