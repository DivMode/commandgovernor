# Command Governor threat model

Status: current for the composition-first Prime distribution in ADRs
0008–0010, as proven on 2026-09-04
([`research/2026-09-04-zero-custom-code-proof.md`](research/2026-09-04-zero-custom-code-proof.md)).

Older standalone Rust daemon / SQLite / browser / MCP threat assumptions are
in [`history/`](history/) and Git history; they are not the current product
topology.

## System shape

```text
Prime Agent (pinned release, verified assets)
  + selected reviewed Prime/Pi packages (exact versions, one owner per concern)
  + @commandgovernor/harness
      - skills / prompts / configuration
      - pin and package manifest
      - black-box conformance
```

Command Governor owns no runtime, daemon, session store, scheduler, subagent
engine, memory engine, mutation ledger, permission engine or browser
automation. Everything that executes is Prime or a package Prime loads.
The security consequences follow directly: Command Governor cannot weaken a
Prime guarantee by wrapping it, and it cannot add a guarantee Prime and its
packages do not provide.

## Assets

Protect:

- repository/worktree contents;
- user credentials and environment secrets;
- provider/API keys and Prime's `auth.json`;
- GitHub authentication;
- authenticated browser/session material used by a ChatGPT transport;
- exact task/revision/session identities and the evidence needed for
  independent review;
- the component manifest: pins, hashes, package versions, owned concerns;
- user-owned decisions.

## Trust assumptions

### Trusted-local profile

The local OS user is the administrative trust root. Prime, every package and
every tool run with that user's authority. Owner-only files are privacy
against other principals, not a same-user sandbox. Sandboxing is optional
hardening for intentionally untrusted repositories, skills or packages
(ADR 0010 §19).

### The substrate is not a policy engine

Prime 0.9.1 ships no permission or approval system. Its only host-side
interception point is the `tool_call` extension event, and its only default
tool is the Python REPL, inside which `bash()`, `subprocess`, `shutil` and
file writes run with no host round trip
([`upstream/2026-09-04-prime-extension-and-daemon-gaps.md`](upstream/2026-09-04-prime-extension-and-daemon-gaps.md) §D).
Measured: the highest-adoption Pi permission package does not load on
Prime, and when patched to load it cannot see kernel-side shell commands.

Consequence: **ADR 0008 invariant 7 (high-risk decisions stay user-owned)
has no enforcement point on the pinned substrate.** No Command Governor
extension could supply one either, because the interception granularity is
Prime's. The load-bearing control for a destructive workload is OS
containment of the kernel process, chosen by the user; the upstream ask is a
kernel-boundary host hook for `rlm.bash`. Until one exists this is an open,
documented limitation, not a mitigated risk.

## Boundaries that hold

### Component and supply-chain integrity

- Prime release assets are verified against the release's own `SHA256SUMS`
  and the committed manifest before npm runs; sibling tarballs are enforced
  by sha512 lockfile integrity.
- Every package is pinned to an exact version or commit with license and
  review date, and admitted only after it is observed to register on the
  pinned Prime, because a package that fails to load is silent in headless
  modes and would otherwise be believed present.
- One component owns each concern. Two packages that both gate the same
  `tool_call`, both mark work done, or both persist memory are rejected at
  the manifest rather than resolved by load order.
- Install scripts are ignored throughout bootstrap.

### External-effect ambiguity

A worker that dies after an external effect is recovered by Prime with a
transcript marker stating the work was not replayed, and no stock client
re-issues a model tool call, agent message, scheduled prompt or RPC bash
command after worker loss. Measured on every stock surface; asserted by the
D2 conformance tests. The remaining upstream defect is a reporting one: the
RPC client is told an untyped `Daemon worker socket closed`. It duplicates
nothing.

### Session identity and recovery

Prime keys every persisted session by canonical JSONL path with a
process-safe lease, converges concurrent opens on one worker, reopens a dead
resident root on the same `sessionId` through `prime-agent -r`, and replaces
a dead supervisor from a live worker. Asserted by the D1 and D8 tests.

### Environment and credentials

Prime resolves provider credentials per model call from `auth.json`
(owner-only) or a named environment variable, never copies them into
session state, and its status surfaces return fingerprints only. Command
Governor forwards no environment of its own because it runs no process of
its own; the user's shell environment is Prime's environment.

### ChatGPT foreman transport

Any ChatGPT Web integration is unofficial. The pinned transport, `pi-gpt`,
drives ChatGPT's undocumented backend with the user's Codex login token from
`~/.codex/auth.json` and solves the provider's security-control checks on
its send path. That is a terms and account-suspension risk on the user's
own ChatGPT account, accepted explicitly by the user (ADR 0008 §8
amendment), and a compatibility risk: pinned client build strings and the
solvers can stop working without notice, and the package has no public
repository to track. The transport never receives any credential other than
that token; Command Governor runs no browser and stores no session material.
The `cg-foreman` skill binds every send to the thread's current leaf and
resolves an ambiguous send by reading, never by resending, so a transport
failure cannot duplicate a message to the foreman. The browser-backed
alternative, `pi-oracle`, waits on its compatibility patch under
`docs/upstream/`.

## Residual risks, stated

| Risk | Status |
| --- | --- |
| Destructive shell/Python inside the kernel cannot be gated by any extension | open; upstream hook or OS sandbox |
| Silent extension load failure in headless modes | mitigated by the package-load conformance probe |
| `ctx.hasUI` true in headless modes misleads approval logic | open upstream; no approval package admitted |
| Undocumented ChatGPT transport (`pi-gpt`) | accepted by the user; account and compatibility risk recorded; sends bound to the leaf and reconciled by readback (TRN-004) |
| Package churn (daily releases, version floors, no Prime compatibility statements) | every re-pin re-runs the load probe and the black-box suite |
