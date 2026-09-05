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
  `@gotgenes/pi-subagents`, `pi-pr-review`, `pi-gpt` (the foreman transport,
  adopted on the user's explicit risk decision). Rejected with measured
  reasons: `pi-squad`, `pi-subagents`, `pi-governance-pipeline`, `pi-oracle`
  (until patched upstream), `@gotgenes/pi-permission-system`.
- ChatGPT foreman loop proven live on the exact user-created thread:
  send, correlated readback, verdict acted on once (proof §6; TRN suite).
- Independent acceptance defined as the foreman's correlated ChatGPT reply
  bound to the head SHA, with GitHub merge after it and Prime's
  `--autonomous-gate` locally; GitHub review is not the record on this
  single-identity repository.
- Conformance rewritten as a black-box suite through stock clients.
- `pi-gpt` vendored in-repo (committed tarball, committed two-guard patch,
  applied by bootstrap, installed by path) so nothing about the foreman
  transport waits on a registry or an author; TRN-003 proves the patch on
  the shipped tool; the opt-in live lane (LIVE-001…003) proves the transport
  inside a real Prime worker against the real account.
- Claude on the user's Max plan inside Prime: `pi-claude-agent-sdk`
  vendored with a three-hunk Prime compatibility patch (it did not load
  unpatched), Claude Code's own login used by the child, no Anthropic
  credential in Prime; a Haiku round trip inside a Prime worker measured
  2026-09-05; LIVE-004 in the opt-in lane. API keys and Prime's Claude
  login are excluded by the user's rule (subscriptions only).
- Substrate and package defect records kept under `docs/upstream/` as this
  repository's own records: Prime extension-surface and daemon gaps, the
  pi-oracle compatibility patch, the `hasUI`/theme worker crash. Nothing
  waits on them being filed elsewhere.

## Next — items that need the user

1. **Browser-backed transport alternative** (`pi-oracle`), only if wanted:
   vendor it the same way (`pins/packages/` + the compatibility patch under
   `pins/patches/`), then `zstd` and `agent-browser` in the Nix
   configuration and one `/oracle-auth`.
2. **Tool gating**: no control exists on the substrate. Decide whether to
   run the trusted-local product without it (documented in the threat
   model) or to apply OS containment to the kernel process; ask Prime for a
   kernel-boundary hook.
3. **First real-model run of the product**: open Prime in this repository
   with `--provider claude-bridge` (Max plan through Claude Code) or the
   Codex login, give it a genuine task, delegate the review, send the
   envelope to the foreman, act on the verdict. Both subscriptions are wired
   and proven inside a Prime worker; the model's judgement over the skills is
   the one thing still unmeasured.

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
