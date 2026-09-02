# Command Governor

Command Governor is a curated Prime/Pi-family harness distribution for durable, foreman-led AI/software-engineering work.

[commandgovernor.com](https://commandgovernor.com)

## Current architecture

**Do not infer the current implementation direction from old Rust topology documents, closed PRs, or previously green implementation branches.**

Read these first:

1. [ADR 0008 — adopt the Pi-native harness architecture](docs/adr/0008-adopt-pi-native-command-governor-harness.md)
2. [ADR 0009 — Prime Agent substrate selection](docs/adr/0009-prime-agent-substrate-and-acp-boundary.md) — still Proposed while final package-path acceptance is proven
3. [ADR 0010 — keep Command Governor composition-first](docs/adr/0010-keep-command-governor-composition-first.md)
4. [2026-09-02 composition/de-duplication audit](docs/research/2026-09-02-command-governor-composition-deduplication-audit.md)
5. [Current conformance strategy](docs/testing.md)

The intended product shape is:

```text
Prime Agent
  + selected reviewed Pi/Prime packages
  + @commandgovernor/harness
      - small Command-Governor-specific policy/integration extensions
      - roles / skills / prompts / configuration
      - focused conformance tests
      - temporary compatibility shims only for proven upstream defects
```

Command Governor is **not** a second general-purpose runtime or control plane around Prime.

Every custom capability must be classified as one of:

```text
USE EXISTING
PLUGIN
TEMP WORKAROUND
DELETE / DO NOT BUILD
```

The first review question is always:

> **Should this code exist in Command Governor at all?**

Only after that is demonstrated should implementation correctness be reviewed.

## Reliability goals

The architecture pivot does not weaken the product semantics:

- delegated work must survive relevant restarts and remain discoverable until required disposition;
- worker completion is not the same as independent acceptance/foreman disposition;
- stale revisions/identities cannot close or mutate newer work;
- ambiguous external effects are reconciled rather than blindly replayed;
- lossy memory/compaction is never authority for exact lifecycle, capability, safety, or user-owned decisions;
- independent review cannot be satisfied by implementer self-certification.

These are product requirements, not justification for duplicating Prime/Pi capabilities that already satisfy them.

## Active implementation status

Prime Agent `v0.8.1` remains the pinned substrate candidate while the actual package-shaped product path is proven.

The merged PR #18 D1/D2/D8 adaptation code is **temporary by default**, not permanent architecture:

- D1 session registry/recovery exists only until the package path proves whether Prime can own recovery/identity directly;
- D2 mutation classification/ledger exists only until the exact worker-loss reproducer proves whether the package path can duplicate an external effect without it;
- D8 path policy exists only until the package path proves the same explicit persistent session-path invariant at the higher-level API.

See [`harness/authorities.json`](harness/authorities.json) for the current owner, disposition, and removal condition for each concern.

## Tests

There is one active merge-gating test strategy: [`docs/testing.md`](docs/testing.md).

CI runs:

1. exact Prime/component bootstrap and integrity checks;
2. TypeScript typecheck;
3. a small credential-free policy/temporary-workaround unit tier;
4. isolated real-Prime D1/D2/D8/environment/S1 conformance;
5. a zero-surviving-process sweep.

The original Phase-1 Rust daemon/SQLite/testkit workspace and its parallel acceptance universe have been retired from the active tree/CI. Historical invariants remain available in Git history and [`docs/research/2026-09-01-rust-invariant-catalog.md`](docs/research/2026-09-01-rust-invariant-catalog.md).

Tests protect product invariants; they do not grant permanent ownership to the component they happen to test.

## Security

Read [SECURITY.md](SECURITY.md) and the [current threat model](docs/threat-model.md).

The default profile is trusted-local: the local OS user is the administrative trust root. Owner-only files are not a hostile same-user sandbox. Sandboxing is optional hardening for intentionally untrusted workloads and should come from an existing isolation mechanism rather than a second Governor runtime.

## Historical architecture

ADRs 0001–0007 and the older Rust daemon/browser/MCP documents remain useful provenance and invariant material. Their implementation topology is superseded to the extent described by ADRs 0008–0010.

Correct historical code may be removed when the selected Prime/package architecture no longer needs it.

## Instructions and contribution policy

Global Claude Code and Codex instructions are managed declaratively by `DivMode/nix-config`; this repository does **not** duplicate them in repo-local `CLAUDE.md` or `AGENTS.md` files. Project-specific architecture lives in the ADRs, current research, `docs/testing.md`, and contributor policy.

Contributors should read [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Research

Current and historical source-grounded research lives under [`docs/research/`](docs/research/), including Pi/Prime package evaluation, ChatGPT transport research, DeepSeek Harness donor research, memory/session research, and the current de-duplication audit.

## Roadmap

See [`docs/roadmap.md`](docs/roadmap.md). The next milestone is the smallest installable `@commandgovernor/harness` package, followed by package bake-offs and D1/D2/D8 reruns through the actual package path.

## License

Command Governor is licensed under the [MIT License](LICENSE).
