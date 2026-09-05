# Command Governor conformance strategy

This document defines the **current** test contract for the composition-first
Prime product described by ADRs 0008–0010 and proven by
[`research/2026-09-04-zero-custom-code-proof.md`](research/2026-09-04-zero-custom-code-proof.md).

Command Governor ships no custom production runtime code. Every test
therefore asserts behaviour of something Command Governor does not implement:
the pinned Prime Agent, the packages it selects, and the manifest that binds
them. A test that can only be made to pass by writing Command Governor code
is a finding against the architecture, not a reason to write the code.

## What a test is for

A Command Governor test must protect one of two things:

1. a product invariant (ADR 0008 §4) as observed through a surface a user
   actually runs: the stock `prime-agent` clients, the pinned daemon, the
   installed packages, the committed manifest; or
2. a distribution fact: the pin is exactly what the release published, the
   manifest is internally consistent, one component owns each concern, and
   every admitted package registers on the pinned Prime.

Before adding or keeping a test, answer:

- Which product invariant or distribution fact does it protect?
- Is the assertion observable at the smallest useful product boundary
  (stock client, daemon socket, transcript on disk, effect on disk)?
- Can the measurement come back negative? A suite proves this with a
  negative control next to each safety assertion (the D2 counter must see
  a deliberate duplicate).
- Does it encode an upstream defect as a permanent expectation? It must not.
  Upstream defects are recorded under `docs/upstream/`; a test asserts the
  product invariant that holds despite the defect, so an upstream fix
  changes nothing.

Do not add a test whose purpose is to prove a Command Governor subsystem
works. There are none.

## Tiers

### Tier 1 — distribution and policy (`conformance/tier1`)

Credential-free, seconds, run in parallel:

- **PIN-001** `pins/pins.json` agrees with `pins/SHA256SUMS` (sha256 per
  asset), with the install-root lockfile (sha512 integrity per sibling, the
  wrapper resolved from `vendor/`), and with the installed binary
  (`prime-agent --version`, sibling versions). No upstream Pi co-install.
- **PKG-001** every entry in `packages[]` is an exact npm version or a
  40-character commit, with a `sha512-` integrity, license, review date and
  exactly one owned concern; no two entries own the same concern. A vendored
  entry (committed tarball plus patches under `pins/`) is pinned by the
  tarball's sha512. The rules run over fabricated violating records first
  so they are shown able to fail.
- **OWN-001** one owner per concern in the manifest; unassigned concerns
  are named, not implied.
- **JSON-001** every committed JSON document parses.
- Typecheck of the conformance sources themselves.

### Tier 1 runtime — black-box pinned Prime (`conformance/runtime`)

Each file starts its own isolated Prime supervisor under `/tmp/cg-XXXXXX`
(own HOME, agent directory, session directory, socket, scripted mock model)
and drives it only through stock clients: the interactive TUI on a pty,
`prime-agent -r`, `prime-agent list --json`, `--mode rpc`, `-p`,
`--mode json`. The files run sequentially because they kill supervisors and
workers on purpose, and every file ends by sweeping its own process tree
(from recorded pids and Prime's worker descriptors; `ps` cannot see
`prime-agent` by command line) and failing on survivors.

- **HELLO-001** the live `daemon_hello` reports the protocol name, version
  and schema revision recorded in the manifest.
- **D1-001** after SIGKILL of a resident worker (the supervisor's own
  relaunch behaviour is recorded, not asserted), `prime-agent -r <sessionFile>` reopens it
  with the same `sessionId` and `sessionFile`, a new `activeSessionId`, and
  exactly one live row; the transcript only grew and carries exactly one
  `prime-agent.worker_recovery` entry.
- **D1-002** two simultaneous `prime-agent -r <same file>` clients converge
  on one worker and one record.
- **D1-003** SIGKILL of the supervisor under a live worker yields a
  replacement supervisor on the same socket serving the same
  `activeSessionId` and worker pid.
- **D2-001** a model tool call appends one line and sleeps; the worker is
  SIGKILLed after the line is on disk; after reopen the file holds exactly
  one line, the model was asked exactly once, exactly one tool call was
  issued, and the recovery marker names `tool_execution_start` and says the
  work was not replayed.
- **D2-002** negative control: the same prompt sent twice produces two
  lines.
- **D2-003** a stock `--mode rpc` `{"type":"bash"}` whose worker dies after
  the effect yields exactly one failure response and the effect exactly
  once; Prime never re-issues it.
- **D8-001** TUI, `-p`, `--mode json` and `--mode rpc` each leave exactly one
  `<sessionDir>/*.jsonl` containing the turn; `--no-session` leaves none;
  `list --json` names a `sessionFile` that exists; after SIGKILL of worker
  and supervisor `-r` reopens with unchanged `sessionId` and `sessionFile`.
- **LOAD-001** the Command Governor package and every `packages[]` entry
  register their tools, commands or skills on the pinned Prime, observed
  positively (the mock model's `tools` array, the skill roster or a
  registered command), with a deliberately broken extension as the negative
  control, because extension load failures are silent in headless modes.
  The role files under `harness/agents/` are installed into the fixture
  project's `.pi/agents/` and observed through the delegation package
  itself, which is what reads them.
- **TRN-000…003** the ChatGPT foreman transport, on the vendored `pi-gpt`
  tree bootstrap extracted and patched, against a mock backend served by
  the test. Three things that would break the foreman loop if a re-vendor
  or re-base changed them: the committed tarball hashes to the pin and the
  tree carries the patch (000); a send lands in the requested thread under
  its current leaf with the caller's message id, persistently, and reads
  back on the active branch (001); an ambiguous send is exactly one request
  and is classified by reading, never by resending (002); the repository's
  patch holds on the shipped `gpt_chat` tool, with the passing control
  (003). Credential-free, no network. The correlation rules themselves are
  prose in the skill and are not re-encoded as tests: a test of a checker
  that lives in the test file measures nothing about the product.
- **GATE-001** Prime's `--autonomous --autonomous-gate "<cmd>"` is a
  host-owned gate: with an identical scripted model, a run does not finish
  while the gate command fails, and finishes once the test (not the model)
  makes it pass. This is the mechanism the acceptance rule relies on.
- **CLEAN-001** every file ends with zero processes referencing its
  fixture root.

### Opt-in live lane — not in the merge gate

- **LIVE-001…003** (`conformance/runtime/live-chatgpt.test.ts`) runs only
  with `CG_LIVE=1` and a Codex login: inside a real Prime worker, the
  pinned `pi-gpt` reads the account, sends into a temporary chat (nothing
  is kept), and, with `CG_FOREMAN_THREAD` set, reads the exact foreman
  thread with message ids. It exists because every other test is blind to
  the provider: TRN measures the package against a mock and cannot fail
  when chatgpt.com changes its endpoints, build strings or security-control
  checks. This lane can. Run it before relying on the foreman loop and after
  any sign of transport drift. It is skipped, with its reason, in CI.

Scenarios that need a real model provider are not stubbed into the suite.
The merge gate on `main` is the `protect-main` ruleset, whose required
checks are exactly the two `harness` jobs this workflow emits.

## Falsifying controls

A negative control is required next to every assertion whose passing could
be explained by a blind measurement: the effect counter, the load probe, the
pin comparison. Prefer one strong control at the product boundary over many
tests that re-encode the history of a repaired internal.

## No duplicate test universes

The standalone Rust workspace and the external raw-daemon-client adaptation
layer are gone from the tree; their tests went with them. Historical
invariants remain in `docs/research/2026-09-01-rust-invariant-catalog.md`
and Git history. If a future change needs one of those semantics, prove the
requirement against the current product path with a black-box test; do not
restore the old implementation to reuse its tests.
