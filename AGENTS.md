# Command Governor agent instructions

These instructions are mandatory for implementation and review work in this repository.

## Read before changing production code

Read, in order:

1. `docs/adr/0008-adopt-pi-native-command-governor-harness.md`
2. `docs/adr/0009-prime-agent-substrate-and-acp-boundary.md`
3. `docs/adr/0010-keep-command-governor-composition-first.md`
4. `docs/research/2026-09-02-command-governor-composition-deduplication-audit.md`

`main` plus accepted ADRs are the architecture authority. Old branches, closed PRs, stale plans, and prior agent verdicts are evidence only.

## Core architecture rule

Command Governor is a small Prime/Pi harness/package distribution, not a second general-purpose runtime or control plane around Prime.

Before writing or retaining a custom production component, classify it as exactly one of:

- `USE EXISTING` — Prime/Pi/an existing reviewed package already owns the capability.
- `PLUGIN` — genuinely Command-Governor-specific policy/integration in the smallest practical extension/package.
- `TEMP WORKAROUND` — minimum shim for a proven upstream defect on the actual product path, with a removal condition.
- `DELETE` — redundant, speculative, or unnecessary custom machinery.

Do not invent a fifth category.

## Mandatory order of reasoning

1. **Should this code exist?**
2. What current Prime/Pi/package capability was checked first?
3. What exact gap remains?
4. What is the smallest owner that can close that gap?
5. Only then: is the implementation correct?

Green tests, a large sunk implementation, or a prior approval do not establish architectural necessity.

## Do not recreate generic substrate capabilities

Do not build another generic worker/session runtime, supervisor, scheduler, goal engine, subagent framework, task/evidence system, review framework, workflow engine, memory engine, event spine, mailbox, or ChatGPT browser runtime unless current primary-source evidence proves the selected substrate/package path cannot satisfy the requirement.

Generic missing primitives should be fixed upstream where practical. Local workarounds remain narrow and temporary.

## Architecture changes

If work changes an authority boundary established by ADRs 0008–0010, update or supersede the ADR before growing the implementation around the new boundary.

When an architecture pivot supersedes active work, preserve useful research on `main`, close or retarget stale PRs, and remove stale roadmap language. Do not leave obsolete implementation PRs open as accidental instructions for the next session.

## Reviewer rule

Review necessity before correctness. A reviewer must stop a change whose existence proof is missing even if its code and tests are excellent.

Challenge premises that conflict with repository evidence. Do not agree with a proposed direction merely because it was requested or because previous work already followed it.