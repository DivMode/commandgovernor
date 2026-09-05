# Repository and test shrink review — 2026-09-02

Status: implementation record for ADR 0010 composition-first cleanup.

## Finding

The repository had accumulated two active architecture generations:

1. the original standalone Rust daemon/SQLite/artifact/testkit control plane; and
2. the Prime-native TypeScript adaptation/conformance layer.

CI merge-gated both. The TypeScript layer then accumulated increasingly detailed CAS/race/durability tests while hardening the external D1/D2 adaptation. The result was a large test surface that protected implementation topology as much as product behavior.

That was appropriate as temporary migration/bug-forensics work, but it is not the intended permanent architecture after ADR 0010.

## Before this cleanup

Active dedicated test files on the current tree:

- TypeScript conformance: **29** files, **255,981 bytes** of test source;
- Rust integration/acceptance: **21** files, **552,511 bytes** of test source;
- total: **50 test files**, **808,492 bytes** of dedicated test source;
- additional Rust `governor-testkit` and per-suite support code existed beyond those test-file bytes.

The Rust and TypeScript suites were both merge-gating CI despite representing superseded and current implementation generations respectively.

## This cleanup

### Retired from the active product tree

- the entire `crates/` standalone Rust workspace;
- root Cargo manifest/lockfile/toolchain/deny policy that existed solely for that workspace;
- Rust fmt/clippy/test/audit/deny merge-gating jobs.

Historical Rust invariants remain in Git history and `docs/research/2026-09-01-rust-invariant-catalog.md`.

### Removed TypeScript implementation-forensics tests

The cleanup removes the largest internal race/durability suites whose stronger product boundary is the surviving real Prime conformance:

- `digest.test.ts`;
- `durable.test.ts`;
- `ledger-adoption.test.ts`;
- `ledger-cas.test.ts`;
- `ledger-race.test.ts`;
- `process-identity.test.ts`;
- `registry-cas.test.ts`;
- `registry-race.test.ts`;
- `registry.test.ts`;
- now-unused ledger/registry race child helpers.

These removals eliminate **125,615 bytes** of TypeScript test source.

### Preserved tests

The active suite keeps the real pinned-Prime runtime reproducers:

- D1 resident-root recovery;
- D2 worker-loss uncertainty/no duplicate effect;
- D2 post-effect `import_jsonl` ordering plus pre-effect control;
- D8 explicit session-path behavior;
- environment boundary;
- S1 regression cases;
- final process sweep.

It also keeps a smaller unit/policy layer for pin integrity, ownership/disposition, structural D2 classification, client/journal identity, probe mismatch refusal, basic temporary ledger behavior, role/policy validation, and path/environment preflight.

## After this cleanup

Active dedicated test files:

- TypeScript: **20**;
- Rust: **0**;
- total dedicated test source: approximately **130,366 bytes**.

That is:

- **50 -> 20 active test files** (60% fewer files);
- **808,492 -> 130,366 bytes** of dedicated test source (about **84% less**);
- plus removal of the standalone Rust testkit/support implementation.

The goal is not a smaller number for its own sake. The goal is one active product architecture and tests concentrated at the strongest useful boundary.

## What is deliberately not deleted yet

The current D1/D2 production adaptation remains until ADR 0010's required package-path reproducers answer whether it is actually needed.

`harness/authorities.json` now marks the custom D1/D2/D8 owners as `TEMP WORKAROUND` and records explicit removal conditions.

The next cleanup must:

1. build the minimum installable `@commandgovernor/harness` package path;
2. run D1/D2/D8 through that path;
3. delete the custom registry/recovery/mutation machinery when Prime/package behavior makes it unnecessary;
4. delete the remaining workaround-only tests in the same change.

This avoids two opposite mistakes: retaining obsolete control-plane machinery because it has tests, or deleting a proven safety shim before the higher-level product path has reproduced the required behavior.

## Outcome (2026-09-04)

The package-path reproducers this review deferred to were run through stock
Prime clients on 2026-09-04
([`../research/2026-09-04-zero-custom-code-proof.md`](../research/2026-09-04-zero-custom-code-proof.md)).
None of the retained D1/D2/D8 machinery was needed; `governor/*`, its
remaining tests, the transport stub, the role schema and `authorities.json`
were deleted in the same pull request, and the conformance suite was
rewritten as black-box tests through stock clients. Custom production code:
0 lines.
