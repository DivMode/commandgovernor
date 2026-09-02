# The Prime Agent distribution

How Command Governor pins, installs, verifies and re-pins its runtime
substrate. Substrate selection is ADR 0009; the empirical evidence behind the
pin is Issue #15; the adaptation layer that makes the pin production-safe is
Issue #17 and [`prime-native/adaptation-layer.md`](prime-native/adaptation-layer.md).

## What is pinned

`pins/pins.json` is the component manifest (ADR 0009 §11) and the single
source of truth for the substrate. Nothing else hardcodes a version string;
the Governor's daemon client reads the expected protocol, version and schema
revision from it at runtime and refuses any daemon that disagrees.

| Field | Value |
| --- | --- |
| substrate | Prime Agent `v0.8.1`, commit `514633727bf26d74f39f3119c2b0e31a5ceb2a9d` |
| license | MIT (Mario Zechner 2025, Prime Intellect 2026) |
| daemon protocol | `prime-agent.daemon` v7, schema revision 22 |
| assets | wrapper `prime-agent-0.8.1.tgz` plus siblings `prime-agent-core`, `prime-agent-ai`, `prime-agent-tui`, each with sha256 and sha512 |
| install root | `pins/prime-0.8.1/` (committed `package.json`, `package-lock.json`, `.npmrc`; `vendor/` and `node_modules/` are derived and ignored) |
| fallback | upstream Pi v0.84.4, recorded, never co-installed; working bootstrap on frozen PR #16 |

Prime is not on the npm registry. Its wrapper package names its three
sibling packages as bare URLs on a Cloudflare R2 bucket with no integrity
hash, and it republishes upstream Pi's package names (`@earendil-works/*`) at
its own version line. Two consequences the manifest encodes:

- **A URL is never the authority.** Each sibling's sha512 is recorded in the
  manifest and enforced by the install-root lockfile's `integrity` field;
  `conformance/tier1/pin.test.ts` proves the two are equal. The bytes may
  come from the R2 mirror, but they are accepted only if they hash to what
  the GitHub release published.
- **Never co-install.** One `node_modules` tree cannot hold Pi 0.84.4 and
  Prime 0.8.1. Bootstrap and the pin test both refuse an
  `@earendil-works/pi-coding-agent` in the install root and any
  `@earendil-works` tree at the repository root.

## Bootstrap

```sh
scripts/bootstrap.sh
```

In order:

1. `node` satisfies the floor in the manifest.
2. The release's `SHA256SUMS` is fetched from the immutable GitHub release
   and must be byte-identical to the committed `pins/SHA256SUMS`.
3. Each asset is downloaded into `pins/prime-0.8.1/vendor/` (or reused if
   already present and correct) and verified against both the manifest and
   `SHA256SUMS`; the two must agree with each other first.
4. `npm ci --ignore-scripts` in the install root (lockfile integrity for the
   siblings and every transitive dependency), then at the repository root
   (TypeScript and the Node type definitions only).
5. The installed sibling versions and `prime-agent --version` (printed on
   stderr) must equal the pinned version; the co-install checks above run.

Install scripts are ignored throughout. Prime's own `postinstall` is a
no-op unless opt-in `PRIME_AGENT_BOOTSTRAP_*` variables are set, and the
native dependencies (zeromq, koffi) ship prebuilt binaries for the supported
platforms.

The Python kernel (uv, CPython 3.11, a ~270 MB venv under the agent
directory) is bootstrapped lazily by Prime on first tool use. The
credential-free conformance tier never triggers a tool call and runs with
`PRIME_AGENT_INSTALL_UV=0`, so neither CI nor a local run needs it.

## Runtime layout

The Governor never uses Prime's default socket (`$TMPDIR/prime-agent-<uid>/daemon.sock`)
or the developer's `~/.prime`. A Governor instance names its own supervisor
socket, agent directory, HOME and TMPDIR, and spawns the supervisor with
`prime-agent --mode daemon --daemon-socket <path>` under a positive
environment allowlist (`governor/prime/env.ts`).

`TMPDIR` must be short. Prime places worker sockets at
`<TMPDIR>/prime-agent-<uid>/worker-<12 hex>-<12 hex>.sock`, and macOS caps a
Unix socket path at 104 bytes. The default macOS `TMPDIR` overflows this for
worker sockets (a bare `listen EINVAL`), which is why the conformance fixture
lives under `/tmp/cg-XXXXXX` and why `spawnSupervisor` refuses a `TMPDIR`
that cannot fit.

Prime facts the Governor depends on, all measured on the pinned build
(Issue #15 unless noted):

- `create`/`attach` forward `launchEnv` to the supervisor, and the supervisor
  hands its own environment to every worker (Issue #17). Both edges are
  allowlisted.
- `prime-agent --version` prints to stderr.
- All print-mode invocations read a prompt from stdin; close it.
- `shutdown` without a TTY needs `force: true`; the fixture always sends it.
- `activity` in a session summary is not a health signal (D10);
  `workerState` is.
- `get_rlm_children` on a reopened parent is empty (D9); read the roster.
- `ack_result` compacts the journal entry and re-admits the same id as new
  work (D6). The Governor never sends it for a mutation it may still need
  to reconcile.

## Re-pin ritual

A new Prime release is a new substrate until proven otherwise.

1. Verify the tag resolves to a commit through the GitHub API; record both.
2. Download every release asset and `SHA256SUMS`; hash each asset (sha256
   and sha512); confirm the release's own checksum file agrees.
3. Update `pins/pins.json` (version, tag, commit, assets, protocol version
   and schema revision as reported by `daemon_hello`), replace
   `pins/SHA256SUMS`, and regenerate the install-root lockfile with
   `npm install --ignore-scripts --package-lock-only` against the vendored
   wrapper tarball.
4. Re-read the D2 code path. `conformance/tier1/prime-protocol.test.ts`
   asserts the pinned supervisor still journals a worker-transport failure
   as a definite result; if a new pin changes that, the assertion fails on
   purpose so the Governor guard is re-evaluated deliberately rather than
   kept by inertia. The read-only command set and the error-code
   vocabulary are diffed against the pinned build the same way.
5. `scripts/bootstrap.sh && scripts/conformance.sh` locally, then the
   `harness` CI job on the pull request. Both must be green.
6. An independent reviewer who did not perform the re-pin re-runs the
   D1/D2/D8 runtime tests.

Upgrading across a daemon protocol or schema revision is a substrate change
in its own right and goes back through ADR 0009's acceptance conditions.
