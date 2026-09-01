# The Command Governor Pi distribution

How the pinned Pi runtime is obtained, launched, and re-pinned, and why the
project's own trust and authority rules are checked by tests rather than
documented as conventions.

Status: **Gate P1 foundation.** Pinning, package structure, version checking,
resource-loading smoke tests and the conformance-suite shape exist. Subagents,
foreman transport, memory, MCP and analytics do not; `harness/authorities.json`
names each of those concerns and records that nothing owns it yet.

---

## Layout

```
package.json                     the pi package manifest + tooling devDeps
package-lock.json                the tooling lockfile
tsconfig.json                    typecheck configuration (noEmit)
.npmrc                           ignore-scripts=true
harness/
  extensions/
    cg-version-guard.ts          refuses an unpinned runtime
    cg-foreman/transport.ts      transport interface, types only
  skills/  prompts/  themes/     declared resources
  agents/*.md                    role definitions; NOT in the pi manifest
  profiles/<role>/.pi/           role-scoped settings templates
  authorities.json               concern -> owner
pins/
  pi-0.84.4/package.json         verbatim release asset
  pi-0.84.4/package-lock.json    verbatim release asset
  pi-0.84.4/.npmrc               ignore-scripts=true
  SHA256SUMS                     verbatim release asset
  pins.json                      the pin record
bin/cg-pi                        launcher
scripts/bootstrap.sh             reproducible install
scripts/conformance.sh           the suite
conformance/                     tier 1 (credential-free), tier 2 (credentialed)
.pi/settings.json                this repository loading its own package
```

`harness/agents/` and `harness/profiles/` are deliberately outside the `pi`
manifest. Pi's manifest parser recognises exactly four resource fields —
`extensions`, `skills`, `prompts`, `themes` — and drops anything else silently.
There is no `agents` field, and a `"pi": { "minVersion": ... }` entry would be
accepted by JSON and ignored by Pi. A future Command Governor subagent extension
reads `harness/agents/` directly.

---

## Bootstrap

```sh
scripts/bootstrap.sh
```

Six steps, in an order that matters:

1. **Node is present and at or above the floor** recorded in `pins.json`. The
   floor lives in one place; `package.json` declaring a different one is a
   conformance failure.
2. **The committed pin assets are the bytes upstream published.** Each file is
   checked against the release's own `SHA256SUMS`, keyed by its original asset
   name, *and* the record in `pins.json` is checked against `SHA256SUMS` too.
   Verifying the file against `pins.json` alone would pass happily after
   someone edited `pins.json` to match a file they had changed. This happens
   before npm is allowed to read the lockfile, so a tampered pin cannot cause a
   download.
3. **`npm ci --ignore-scripts`** against the vendored lockfile root. This is
   upstream's own installer input, so it cannot drift from what `pi update`
   produces, and it reproduces the whole transitive tree by `resolved` plus
   `integrity`. Two pinned transitive dependencies (`@google/genai`,
   `protobufjs`) carry install scripts, so a committed `.npmrc` sets
   `ignore-scripts=true` as well — the hardening must not depend on everyone
   remembering to go through this script.
4. **The repository's own tooling is installed** (`npm ci --ignore-scripts` at
   the root: TypeScript and the Node type definitions, both devDependencies, so
   `pi install`'s `npm install --omit=dev` never fetches them for a consumer).
   This runs *before* step 5, because `npm ci` deletes `node_modules` wholesale
   and links created first would be silently destroyed.
5. **Exactly the five Pi-provided packages are linked** into
   `node_modules/`, one by one, so
   `import { VERSION } from "@earendil-works/pi-coding-agent"` inside
   `harness/extensions/` resolves to the pinned copy and nothing else.
   Symlinking the whole `@earendil-works` scope would be shorter and wrong in
   both directions: it exposes `pi-client`, `pi-protocol` and `pi-telemetry`,
   which an extension may **not** import and which would then typecheck locally
   and fail inside Pi, and it misses `typebox`, which *is* provided but is not
   in that scope. `P1-PACKAGES` asserts both directions.
6. **The binary that came out reports the pinned version.** Asserted against
   `pins.json`, not against a second hardcoded copy. Last, so it runs against
   the tree the previous steps actually produced.

Any failure exits non-zero with the two values that disagree.

The machine this was developed on also has pi 0.84.4 on `PATH` via Nix. The
bootstrap does not use it and must not need it.

---

## Launching

```sh
bin/cg-pi [pi arguments...]
```

Never `pi` directly. The launcher does three things an extension cannot.

### The version preflight is fatal here and nowhere else

`cg-version-guard` checks the same pin from inside the session, but
`ctx.shutdown()` is deferred to idle in interactive and RPC modes and is a
**no-op in print mode**. It is a request, not a kill. Refusing before the
process starts is the only refusal that is guaranteed. The in-session guard
still earns its place by blocking every tool call on a refusal, so a print-mode
session that ignores the shutdown cannot do work either.

The launcher accepts a `pi` from `PATH` only when its `--version` equals the
pin. That is a convenience for a machine that already manages pi declaratively,
not a fallback that lets the pin slide.

### `--approve` is passed deliberately

This is the sharpest edge in the whole substrate, so it is stated plainly.

Pi's non-interactive modes — `-p`, `--mode json`, `--mode rpc` — **never prompt
for project trust**. Under the default `defaultProjectTrust: "ask"` they ignore
every project-local resource: no project extensions, no project skills, no
project prompt templates, no project settings. Nothing errors. The process exits
successfully with an empty loadout.

Measured against the pinned binary, with an isolated agent directory:

| invocation | resolved project resources |
| --- | --- |
| `--approve` | `cg-version` extension, `cg-review` prompt, `cg-conformance` skill |
| *(no flag)* | none |
| `--no-approve` | none |

A distribution whose entire point is a curated project-level loadout cannot
leave that to a default, so `bin/cg-pi` passes `--approve`.

But Pi resolves *the project* from the **working directory**, not from where the
launcher lives — so a bare `--approve` is not trust of this repository at all.
It is trust of whatever directory the caller happened to be standing in.
Measured: run from an unrelated directory holding a `.pi/extensions/`, the
launcher loaded that directory's extension and did **not** load
cg-version-guard. A launcher that auto-trusts unreviewed code while dropping its
own safety extension is worse than no launcher.

So the grant is confined structurally, in two halves that are both needed:

1. **Refuse** unless the working directory is inside this checkout. That is what
   stops the foreign directory being trusted.
2. **`cd` to the repository root** before exec. That is what makes the approval
   mean *this* repository — Pi only discovers this loadout from the root, so
   without it a call from a subdirectory would resolve nothing, silently and
   with a successful exit.

`P1-LAUNCHER` pins both, with a negative control: from a temp directory outside
the checkout the launcher must exit non-zero **and** the foreign extension must
not appear in the resolved inventory.

A consumer-facing launcher will need a different trust story and must not copy
this one: a consumer trusts *their* project, which pins Command Governor as a
package in its own `.pi/settings.json`, so the confinement would be to their
repository root and the loadout would arrive through the pinned package rather
than through this checkout.

Read that narrowly. It is trust of **this repository's own committed
configuration**, which is reviewed like any other code here. It is not a general
trust bypass and it grants no permission Pi would otherwise withhold: project
trust governs which files are **loaded**, not what the loaded code may do. Pi
ships no sandbox and no permission system, and runs with the permissions of the
user that launched it. Do not extend `--approve` to a repository whose `.pi`
directory nobody has read.

`--approve` is passed as the first argument so a caller can override it.
Characterized against the pinned binary: **the last flag wins.** `--approve
--no-approve` resolves no project resources; `--no-approve --approve` resolves
them. A caller appending `--no-approve` therefore gets what they asked for.

### Delegated work gets a durable home

`PI_SUBAGENTS_TEMP_ROOT` is set to `${CG_STATE_DIR:-$HOME/.command-governor}/subagents`.

The subagent package selected for Phase C hangs its async run status, results
and artifacts off a temp root defaulting to `os.tmpdir()`, and that override
appears nowhere in its README, docs or changelog. Left alone it means the record
of delegated work dies on a reboot or a tmp sweep — the first line of the
reliability contract. Setting it before the package is installed costs nothing.

---

## The three pin layers

**1. The Pi runtime.** The two `pi-coding-agent-install-*` release assets,
committed verbatim and checksum-verified, installed with `npm ci`. `pins.json`
additionally records the git tag `v0.84.4`, the commit
`b79e4cc834970cca69daebffab7df1da7d1e52c4` that the lightweight tag resolves
directly to, and the npm integrity hash.

**2. Third-party Pi packages.** Currently none. The policy is enforced by
conformance, not by convention, because Pi cannot enforce it: Pi keeps **no
lockfile** for the packages it installs, and its `pinned` flag is satisfied by
any git ref — a branch or a mutable tag included. Only an exact npm version or a
40-character commit SHA is a pin. Every entry in `pins.json` `packages[]` must
carry `exactVersion` or `resolvedSha`, and must name the authority it owns.
Conformance also diffs `.pi/settings.json` `packages[]` against `pins.json`,
because Pi installs missing packages automatically on trusted startup and a
hand-run `pi install` would otherwise make the two disagree silently.

**3. Command Governor's own package.** Consumers install it as an immutable
source:

```sh
pi install -l git:github.com/DivMode/commandgovernor@<40-char commit sha>
```

`-l` writes it into the consuming project's `.pi/settings.json`, which Pi then
auto-installs on trusted startup. `harness/profiles/*/` carries settings
templates for that. They ship with `packages: []` rather than a placeholder
SHA — a fake pin that passes a policy check is worse than no pin.

---

## One authority per concern

`harness/authorities.json` maps each lifecycle concern to the single component
that owns it, and names the ones nothing owns yet.

This is checked rather than documented because the failure mode is undetectable.
Pi 0.84.4 resolves competing extension handlers **silently by load order**: for a
`session_before` event, each handler's result replaces the previous one, so two
extensions that both answer `session_before_compact` do not conflict — the last
one loaded wins, with no error and no warning. The single exception is a handler
returning `{cancel: true}`, which short-circuits and returns immediately, so
cancellation is first-wins while every other outcome is last-wins. Both halves
are the same problem for a distribution: load order decides. Pi also exposes no
runtime API for enumerating loaded extensions, so nothing can detect the
collision from inside a session. The only place it can be caught is over the
distribution's own pinned manifest, at build time.

Two consequences for the file's shape. `concerns` is an array, not an object
keyed by concern, because an object cannot represent two owners for one key and
the duplicate check would be vacuous — the check has to be able to fail. And an
unassigned concern is a real entry carrying a phase and a planned owner, because
naming an unowned concern is what stops it being quietly adopted by whichever
package is installed next.

The highest-risk unassigned concern is `compaction-summary`. It must have
exactly one owner forever, and it is deliberately unowned: no evaluated memory
package satisfies ADR 0007's constraint-survival or dependent-session criteria.

---

## Conformance

```sh
scripts/conformance.sh
```

Node's own `node --test` with native type stripping. No test-framework
dependency and no build step — the `.ts` files the suite imports are the ones Pi
loads through jiti.

The run starts with `tsc --noEmit`. Node's type stripping erases types without
checking them, so without that step every type in the harness and the suite is
decoration. `tsconfig.json` sets `erasableSyntaxOnly`, which also rejects syntax
the stripper cannot run — enums, parameter properties, namespaces — that tsc
would otherwise accept and Node would refuse at load. TypeScript is an
exact-version devDependency with a committed lockfile.

**Tier 1 (credential-free)** must pass on every change and is the gate on any
re-pin:

| id | what it establishes |
| --- | --- |
| P1-PIN | committed pins match the published checksums; the record is internally consistent; third-party pins are immutable; the installed binary reports the pinned version |
| P1-JSON | every committed JSON file parses, discovered by walking the tree rather than from a list |
| P1-MANIFEST | the `pi` block contains only the four keys Pi reads, all arrays of strings, all paths existing; Pi-provided packages are peer dependencies and unbundled |
| P1-LOAD | the distribution loads: the resolved inventory is read back out of a live Pi; `-p` reaches the credential refusal, proving it got past loading; the untrusted-project default is pinned |
| P1-GUARD | the version comparison, both refusal codes, what a refusal does with and without a UI, and tool blocking before and after the check |
| P1-ROLES | role frontmatter validates against the schema; tools and models are checked against the pinned runtime; delegation targets exist; reviewer independence |
| P1-AUTH | no concern has two owners; settings and pins agree; every pinned package names its authority |
| P1-DRIFT | a fabricated pin record makes the guard refuse, in both directions, and mutates nothing |
| P1-SCAFFOLDING | the injected clock and the two domain-separated seeded streams behave; delivery ids are redaction-safe over 10,000 draws; the restart primitive refuses to pretend |
| P1-LAUNCHER | the launcher refuses outside the checkout and does not resolve a foreign project's extensions; inside it, the loadout resolves from the root and from a subdirectory |
| P1-PACKAGES | exactly the five Pi-provided packages resolve from the repository root, and the three same-scope non-contractual ones do not |

**Tier 2 (credentialed)** is skipped unless `CG_CONFORMANCE_LIVE=1`. Its five
tests are placeholders recorded as skips with stated reasons. Each stands in for
a claim this distribution has inherited and has **not** verified — most
importantly that `agent_settled` is non-vetoable, which is asserted by ADR 0008
and by the Pi review but has a precedent for being wrong about a different
harness's stop hook. None of them is faked green.

### Checks that could not otherwise come back negative

Two policies — the third-party pin rule and one-authority-per-concern — run over
collections that are legitimately empty today: `pins.json` `packages[]` has no
entries. An assertion looping over an empty array passes without evaluating
anything, which is not a measurement. Both rules therefore live in
`conformance/lib/policy.ts` as functions, and the suite runs them against
fabricated records that violate each one: a git tag ref, a branch, a bare name,
an abbreviated SHA, a package naming no authority, two packages claiming one
concern, a duplicated concern, an owner path that does not exist, and an
unassigned concern with no planned owner. The rules are shown to fail before
they are trusted to pass.

### What the suite deliberately does not claim

The role `tools` restrictions are **not** enforced by anything today, and the
suite says so rather than implying otherwise. Pi has no agents concept; those
fields bind only whatever extension reads them, and none exists. The Tier 2 test
for it is skipped with that reason attached, and `authorities.json` records
`role-loadout-enforcement` as unassigned.

The restart primitive throws. There is no durable Command Governor state to
restart against yet, and a stub that passed would report coverage the suite does
not have.

---

## Re-pinning Pi

The conformance suite is the **gate** on a re-pin, not a formality after one.
Upstream cut six releases in the roughly four and a half weeks from v0.83.0
(2026-07-29) to v0.84.4 (2026-08-28), so this is a scheduled ritual rather than
an ad-hoc upgrade.

1. Download the new release's `pi-coding-agent-install-package.json`,
   `pi-coding-agent-install-package-lock.json` and `SHA256SUMS`. Verify the two
   assets against the `SHA256SUMS` you just downloaded.
2. Replace `pins/pi-<version>/` and `pins/SHA256SUMS`. Rename the assets to
   `package.json` and `package-lock.json`.
3. Update `pins.json`: version, tag, commit, npm integrity, `installRoot`, the
   asset checksums, `reviewedAt`. Resolve the tag to a commit through the API
   and check `object.type` — a lightweight tag points at a commit, an annotated
   one does not, and only the commit SHA is the pin.
4. Run `scripts/bootstrap.sh`, then `scripts/conformance.sh`. Expect P1-ROLES to
   fail if the new release moved a built-in tool or a model out of the catalog;
   that is the pin doing its job.
5. Read the upstream changelog for anything touching sessions, compaction,
   `agent_settled`, extension lifecycle, or project trust, and record it in the
   commit message.

Never regenerate a checksum to make a failing verification pass. That inverts
the check.

At the time of pinning, upstream `main` was eight commits ahead of v0.84.4 with
no v0.84.5 cut. Two of those commits — settling an active turn before an
in-memory fork, and stopping prepared tools after a preflight abort — are inside
this product's blast radius. The pin is accepted anyway; the drift is tracked.

---

## Related

- [`adr/0008-adopt-pi-native-command-governor-harness.md`](adr/0008-adopt-pi-native-command-governor-harness.md)
- [`pi-native/dependency-matrix.md`](pi-native/dependency-matrix.md)
- [`pi-native/migration-notes.md`](pi-native/migration-notes.md)
- [`research/2026-09-01-pi-substrate-review.md`](research/2026-09-01-pi-substrate-review.md)
