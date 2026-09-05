# The Prime Agent distribution

How Command Governor pins, installs, verifies and re-pins its runtime
substrate. Substrate selection is ADR 0009; the composition boundary is
ADR 0010; the proof that no custom runtime code sits between the pin and the
user is [`research/2026-09-04-zero-custom-code-proof.md`](research/2026-09-04-zero-custom-code-proof.md).

## What is pinned

`pins/pins.json` is the component manifest (ADR 0009 §11) and the single
source of truth for the substrate and for every third-party package the
distribution installs. Nothing else hardcodes a version string; the
conformance suite reads the expected protocol, version and schema revision
from it and compares them with what the installed daemon reports.

| Field | Value |
| --- | --- |
| substrate | Prime Agent `v0.9.1`, commit `81ae3cb34d27d38ee37f9e205a1e73694993b344` |
| license | MIT (Mario Zechner 2025, Prime Intellect 2026) |
| daemon protocol | `prime-agent.daemon` v7, schema revision 25 |
| assets | wrapper `prime-agent-0.9.1.tgz` plus siblings `prime-agent-core`, `prime-agent-ai`, `prime-agent-tui`, each with sha256 and sha512 |
| install root | `pins/prime-0.9.1/` (committed `package.json`, `package-lock.json`, `.npmrc`; `vendor/` and `node_modules/` are derived and ignored) |
| fallback | upstream Pi v0.85.0, recorded, never co-installed |

Prime is not on the npm registry. Its wrapper package names its three
sibling packages as bare URLs on a Cloudflare R2 bucket with no integrity
hash, and it republishes upstream Pi's package names (`@earendil-works/*`) at
its own version line. Two consequences the manifest encodes:

- **A URL is never the authority.** Each sibling's sha512 is recorded in the
  manifest and enforced by the install-root lockfile's `integrity` field;
  `conformance/tier1/pin.test.ts` proves the two are equal. The bytes may
  come from the R2 mirror, but they are accepted only if they hash to what
  the GitHub release published.
- **Never co-install.** One `node_modules` tree cannot hold upstream Pi and
  Prime. Bootstrap and the pin test both refuse an
  `@earendil-works/pi-coding-agent` in the install root and any
  `@earendil-works` tree at the repository root.

## Vendored packages

A package the product needs but whose only source is an npm tarball from an
author with no public repository (`pi-gpt`) is kept **in this repository**:
the tarball is committed under `pins/packages/`, its sha512 is the
manifest's `integrity`, and any change this repository needs is a committed
patch under `pins/patches/`. `scripts/bootstrap.sh` verifies the tarball,
extracts it to `pins/packages/<name>-<version>/` (ignored) and applies the
patches; Prime then installs it by path (`harness/settings.project.json`, or
`prime-agent package install --local <repo>/pins/packages/<name>-<version>`
from another project). Nothing about it waits on a registry, an author, or an
upstream. Prime runs no `npm install` for a path package, so a vendored package
with runtime dependencies also commits a lockfile (`lockfile` in the manifest)
that bootstrap installs with `npm ci --ignore-scripts`. The manifest entry
carries `origin` (where the tarball came from), `tarball`, `patches` and
`lockfile`; `conformance/runtime/foreman-transport.test.ts`
(TRN-000, TRN-003) checks the committed bytes against the pin and that the
patch holds on the shipped tool.

To re-vendor: fetch the new tarball, record its sha512 as `integrity`, re-base
the patches, run bootstrap, and re-run LOAD-001 and TRN.

## Claude models on your subscription

The product never configures an Anthropic API key and never uses Prime's
built-in Claude Pro/Max login. Both bill per token: the key by design, the
login because Anthropic classifies a third-party harness speaking Claude
Code's wire shape as third-party usage ("Third-party harness usage draws from
extra usage", Prime's own login screen) and its terms do not permit it.

Claude runs instead through the vendored `pi-claude-agent-sdk` (`claude-bridge`
provider): Prime hands the turn to the real Claude Code binary through the
Agent SDK, Prime's tools are bridged into it, and the child authenticates with
Claude Code's **own** login (the macOS Keychain entry `claude auth login`
wrote). Prime holds no Anthropic token. This is how T3 Code and other wrappers
use a Claude subscription, and it bills to the plan. Requirements: Claude Code
2.1.251 or newer on `PATH` or `pathToClaudeCodeExecutable`, a completed
`claude auth login`, and `HOME` and `USER` in Prime's environment (Claude Code
locates its Keychain login by both; with `USER` unset it reports "Not logged
in", measured). Then:

```sh
prime-agent --provider claude-bridge --model claude-sonnet-5
```

The vendored package carries a three-hunk Prime compatibility patch (its
`@earendil-works/pi-ai/compat` import, `CONFIG_DIR_NAME`, and Prime's
`getApiKeyAndHeaders` in place of Pi's `getProviderAuth`) plus one behaviour
change: with no Anthropic credential in Prime it starts the child with every
inherited Anthropic variable stripped instead of failing. Measured live on
2026-09-05; LIVE-004 repeats it on demand. Two facts to keep in view: this
bills to plan limits because Anthropic paused its Agent SDK credit split on
June 15 2026, not because the policy settled; and a Claude Code child given an
*invalid* API key hangs, which is one more reason no key is ever configured.

## Bootstrap

```sh
scripts/bootstrap.sh
```

In order:

1. `node` satisfies the floor in the manifest.
2. The release's `SHA256SUMS` is fetched from the immutable GitHub release
   and must be byte-identical to the committed `pins/SHA256SUMS`.
3. Each asset is downloaded into `pins/prime-0.9.1/vendor/` (or reused if
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

The Python kernel (uv, CPython, a venv under the agent directory) is
bootstrapped lazily by Prime on first tool use. The conformance runtime tier
does exercise a real tool call (the D2 effect is a real `ipython` cell), so
a conformance run needs `uv` reachable and network access for the one-time
kernel bootstrap into the fixture's own agent directory.

## Runtime layout the conformance suite relies on

The suite never uses Prime's default socket
(`$TMPDIR/prime-agent-<uid>/daemon.sock`) or the developer's `~/.prime`. Each
fixture names its own supervisor socket, agent directory, HOME and TMPDIR
under `/tmp/cg-XXXXXX` and starts the supervisor with
`prime-agent --mode daemon --daemon-socket <path>`.

Prime facts measured on the pinned build (the 2026-09-04 proof unless noted):

- `TMPDIR` must be short. Prime places worker sockets at
  `<TMPDIR>/prime-agent-<uid>/worker-<12 hex>-<12 hex>.sock`, and macOS caps a
  Unix socket path at 104 bytes; the default macOS `TMPDIR` overflows this.
- `prime-agent --version` prints to stderr.
- Print-mode invocations read a prompt from stdin; close it.
- `prime-agent shutdown` cannot be pointed at a non-default socket; the
  fixture sends `{type:"shutdown", force:true}` on its own socket instead.
- Prime sets `process.title = "prime-agent"`, so a `ps` command-line sweep
  cannot see supervisors or workers on macOS. The fixture sweeps from the
  pids it started plus the worker pids Prime persists under
  `<agentDir>/daemon-workers/*.json`.
- A resident root whose worker dies is not relaunched by the supervisor;
  the stock way back is `prime-agent -r <sessionFile>`, which reopens the
  same `sessionId` on the same transcript. `prime-agent attach <agent>` does
  not (recorded upstream in `upstream/2026-09-04-prime-extension-and-daemon-gaps.md`).

## Re-pin ritual

A new Prime release is a new substrate until proven otherwise.

1. Verify the tag resolves to a commit through the GitHub API; record both.
2. Download every release asset and `SHA256SUMS`; hash each asset (sha256
   and sha512); confirm the release's own checksum file agrees.
3. Create `pins/prime-<version>/` with the install-root `package.json`,
   `.npmrc` and a lockfile regenerated with
   `npm install --ignore-scripts --package-lock-only` against the vendored
   wrapper tarball; remove the previous install root; update
   `pins/pins.json` (version, tag, commit, assets, protocol version and
   schema revision as reported by `daemon_hello`) and replace
   `pins/SHA256SUMS`.
4. Re-read the defect records under `docs/upstream/`. They are this
   repository's own records of substrate and package behaviour it works
   around, kept here whether or not anyone files them elsewhere; a release
   that changes one of them closes or reopens that record deliberately
   rather than by inertia. Re-base the patches under `pins/patches/`.
5. Re-screen every entry in `packages[]` against the new Prime: a package
   that loads on Pi is not thereby proven on Prime (extension load failures
   are silent in headless modes). The package-load conformance test is the
   gate.
6. `scripts/bootstrap.sh && scripts/conformance.sh` locally, then the
   `harness` CI job on the pull request. Both must be green.

Upgrading across a daemon protocol or schema revision is a substrate change
in its own right and goes back through ADR 0009's acceptance conditions.
