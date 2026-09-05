# Command Governor roadmap

This roadmap follows ADRs 0008–0010 and the 2026-09-04 proof
([`research/2026-09-04-zero-custom-code-proof.md`](research/2026-09-04-zero-custom-code-proof.md)).
The success metric is unchanged: **a small, usable Prime distribution with
strong product-level conformance**, measured by how little Command Governor
has to own, not by how much it builds.

## Done — 2026-09-04

- Custom production code removed entirely (`governor/*`, transport stub,
  role schema, authorities file; 4,723 lines) after D1/D2/D8 were proven on
  the stock Prime client path with zero custom code.
- Standalone Rust workspace and its tests retired (PR #24); Rust-era design
  documents moved to `docs/history/`.
- Substrate re-pinned to Prime `v0.9.1` with verified assets.
- Packages selected and pinned by observed load on Prime: `pi-tasks`,
  `@gotgenes/pi-subagents`, `pi-pr-review`. Rejected with measured reasons:
  `pi-squad`, `pi-subagents`, `pi-governance-pipeline`, `pi-oracle` (until
  patched upstream), `pi-gpt`, `@gotgenes/pi-permission-system`.
- Independent acceptance defined as the GitHub review/merge by the reviewer
  of record, bound to the head SHA (`pi-pr-review`), gated locally by
  Prime's `--autonomous-gate`.
- Conformance rewritten as a black-box suite through stock clients.
- Upstream records drafted: Prime extension-surface and daemon gaps, the
  pi-oracle compatibility patch, the `hasUI`/theme worker crash.

## Next — items that need the user or upstream

1. **File upstream** (outward-facing, user-approved): the Prime gaps in
   `upstream/2026-09-04-prime-extension-and-daemon-gaps.md` through Prime's
   Discussions gate; the pi-oracle issue with the attached patch.
2. **ChatGPT foreman authenticated proof**, once a transport loads on Prime:
   provide an exact `https://chatgpt.com/c/<id>`; add `zstd` and
   `agent-browser` to the Nix configuration; run `/oracle-auth` once in an
   interactive TUI. The correlation rules (envelope, reply echo,
   stale-revision rejection, never resend on ambiguity) become a skill in
   `harness/skills/`, not code.
3. **Tool gating**: no control exists on the substrate. Decide whether to
   run the trusted-local product without it (documented in the threat
   model) or to apply OS containment to the kernel process; ask Prime for a
   kernel-boundary hook.
4. **Real-model conformance lane** (optional, credentials required): the
   same scenarios with a real provider, to measure model behaviour rather
   than package mechanics.

## Later — only with a measured gap

- Memory: Prime's continual harness is the default owner; evaluate exactly
  one memory package only if downstream-action tests show a gap.
- ACP: use Prime's stable ACP v1 server when a client path needs it; an ACP
  client for driving other agents is an upstream contribution.
- Sandbox profile for intentionally untrusted workloads.
- Cost/cache accounting as a conformance measurement, not a feature.

## Re-pin cadence

Prime and the pinned packages release daily to weekly. A re-pin follows
`prime-distribution.md`: verify assets, regenerate the lockfile, re-read the
upstream records (a fix closes one; a regression reopens one), re-run the
package-load probe and the black-box suite.

## Explicitly not on the roadmap

Unless new evidence changes ADR 0010, do not build:

- another daemon, runtime, session store, scheduler, subagent engine,
  workflow engine, memory engine or mailbox beside Prime;
- a local acceptance record, mutation ledger or session registry — the
  proof showed each is either already Prime's or unenforceable locally;
- a browser/CDP ChatGPT automation stack;
- a permission extension — it cannot see kernel-side shell on Prime;
- tests of Command Governor subsystems, because there are none.
