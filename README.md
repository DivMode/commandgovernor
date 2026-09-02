# Command Governor

Command Governor is a curated Prime/Pi-family harness distribution for durable, foreman-led AI/software-engineering work.

[commandgovernor.com](https://commandgovernor.com)

## Current architecture

**Do not infer the current implementation direction from old Rust topology documents, closed PRs, or previously green implementation branches.**

Read these first:

1. [ADR 0008 — adopt the Pi-native harness architecture](docs/adr/0008-adopt-pi-native-command-governor-harness.md)
2. [ADR 0009 — Prime Agent substrate selection](docs/adr/0009-prime-agent-substrate-and-acp-boundary.md) — still Proposed while final substrate acceptance is proven
3. [ADR 0010 — keep Command Governor composition-first](docs/adr/0010-keep-command-governor-composition-first.md)
4. [2026-09-02 composition/de-duplication audit](docs/research/2026-09-02-command-governor-composition-deduplication-audit.md)

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

- delegated work must survive relevant restarts and remain discoverable until disposition;
- worker completion is not the same as independent acceptance/foreman disposition;
- stale revisions cannot close newer work;
- ambiguous external effects are reconciled rather than blindly replayed;
- lossy memory/compaction is never authority for exact lifecycle, capability, safety, or user-owned decisions;
- independent review cannot be satisfied by implementer self-certification.

These are product requirements, not justification for duplicating Prime/Pi capabilities that already satisfy them.

## Historical architecture

ADRs 0001–0007 and the older Rust daemon/browser/MCP documents remain useful provenance and invariant material. Their implementation topology is superseded to the extent described by ADRs 0008–0010.

The merged PR #18 Prime adaptation code is also subject to salvage under ADR 0010. Passing correctness review does not make a custom subsystem permanent architecture.

## Agent and contributor instructions

Coding and review agents must read [AGENTS.md](AGENTS.md). Claude Code also receives the same canonical rules through [CLAUDE.md](CLAUDE.md).

Contributors should read [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Research

Current and historical source-grounded research lives under [`docs/research/`](docs/research/), including Pi/Prime package evaluation, ChatGPT transport research, DeepSeek Harness donor research, memory/session research, and the current de-duplication audit.

## License

Command Governor is licensed under the [MIT License](LICENSE).