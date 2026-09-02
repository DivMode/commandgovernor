# Command Governor conformance strategy

This document defines the **current** test contract for the composition-first Prime/Pi product described by ADRs 0008–0010.

It replaces the pre-pivot standalone-Rust V1 acceptance catalog. Historical Rust behavior remains available in Git history and in `docs/research/2026-09-01-rust-invariant-catalog.md`; it is not an instruction to rebuild or keep a second control plane.

## What tests are for

A Command Governor test must protect one of two things:

1. a product invariant that must remain true regardless of implementation owner; or
2. a narrowly identified `TEMP WORKAROUND` that still exists because a reproduced upstream defect affects the actual product path.

A test does **not** justify retaining the component it tests.

Before adding or keeping a test, answer:

- Which product invariant or temporary workaround does it protect?
- Is the assertion observable at the smallest useful product/package boundary?
- Is another test already proving the same property at a stronger boundary?
- If it protects a temporary workaround, what condition deletes both the workaround and this test?

Do not preserve an implementation-specific race matrix merely because it is thorough when the implementation itself is being retired.

## Test levels

### 1. Distribution and policy checks

Credential-free checks for facts Command Governor owns directly:

- exact Prime release/component pin and integrity metadata;
- dependency/package authority metadata;
- one owner per concern;
- `USE EXISTING` / `PLUGIN` / `TEMP WORKAROUND` classification for assigned concerns;
- required removal condition for every `TEMP WORKAROUND`;
- role/schema and repository-policy validity;
- TypeScript typechecking.

These checks should be small and deterministic.

### 2. Temporary-workaround unit checks

Unit tests are allowed for a compatibility shim only while that shim is necessary.

The current D1/D2 adaptation code is transitional under ADR 0010. Its unit coverage should prove the minimum fail-closed contract, not every internal storage interleaving:

- mutation classification is structural and defaults to `UNCERTAIN` when proof is absent;
- reviewed pre-effect rejection can be classified as effect-absent;
- the stable client/journal identity cannot silently change;
- identity/command mismatch refuses before daemon I/O;
- a lost Governor process cannot turn an unproven mutation into safe replay.

The black-box runtime tests below are the stronger authority. When the package-path D1/D2 reproducer passes without the custom adaptation, delete the workaround and its unit tests together.

### 3. Black-box pinned-Prime conformance

These are the highest-value current tests. They exercise the public pinned Prime runtime in isolated state roots rather than re-proving our implementation details.

Current merge-gating cases:

#### PIN-001 — exact substrate pin

Bootstrap verifies the selected Prime release/assets against committed immutable integrity metadata and refuses a mismatched runtime before commands are sent.

#### OWN-001 — one owner per concern

`harness/authorities.json` has exactly one owner for every assigned concern. Existing substrate/package ownership is preferred over custom Governor ownership. Every temporary workaround names its removal condition.

#### ENV-001 — positive environment boundary

A variable crosses the Governor-to-Prime boundary only when explicitly allowed. Secret-shaped and ordinary-name negative sentinels remain absent; granted controls cross.

#### D1-001 — resident-root recovery

Kill a resident root and recover the same logical Prime session exactly once. The stable logical `sessionId` survives while the active incarnation changes.

#### D1-002 — stale incarnation/cursor rejection

A cursor/incarnation captured before recovery cannot mutate or resume the replacement incarnation as though it were current.

#### D2-001 — worker-loss ambiguity does not duplicate an effect

Cause a worker to die after an observable mutation but before a trustworthy result reaches the client. Command Governor must surface uncertainty/reconciliation and must not automatically repeat the external effect.

#### D2-002 — post-effect typed failure stays fail-closed

Exercise the pinned `import_jsonl` ordering where a filesystem effect occurs before `missing_session_cwd`. The outcome cannot be called effect-absent merely because the error is typed. A reviewed pre-effect rejection remains the positive control.

#### D8-001 — explicit persistent session path

Every Governor-created session uses an explicit canonical persistent session path; omission is refused. Reopen/resume remains on the same transcript identity.

#### S1-001 — supervisor-loss uncertainty

Loss of the supervisor/transport cannot turn an unproven mutation into a safe replay.

#### S1-002 — completed-command idempotence

Repeating an already completed command identity returns the stored result rather than performing the effect again.

#### S1-003 — process-safe session lease

Concurrent ownership of the same persistent Prime session converges to one authoritative writer/owner.

#### CLEAN-001 — no fixture process leakage

Every conformance run ends with zero Prime processes referencing its disposable fixture roots.

## Falsifying controls

A negative/falsifying control is useful when it proves that an important safety assertion would fail under a plausible broken policy. It is **not** mandatory for every helper or every discovered review bug.

Prefer one strong negative control at the product boundary over many tests that encode the history of how an internal implementation was repaired.

## No duplicate test universes

Do not maintain parallel permanent suites solely because an old architecture once existed.

The Phase-1 Rust daemon/SQLite/testkit workspace has been retired from the active tree and CI. Its historical invariant catalog remains research/provenance. If a future Prime/package implementation needs one of those semantics, prove the semantic requirement against the current product path; do not restore the old runtime merely to reuse its tests.

Likewise, deterministic and multi-process versions of the same internal race are not both permanent by default. Keep both only when the component is a demonstrated permanent Command Governor owner and the two tests protect materially different failure modes that cannot be covered at the stronger boundary.

## Adding future features

Do not pre-build test catalogs for speculative components.

When a package or Command-Governor-specific plugin is actually selected, add the smallest acceptance matrix that proves its product contract. Examples include:

- independent candidate acceptance / implementer cannot self-approve;
- durable task/evidence gates;
- exact ChatGPT conversation correlation and stale-revision rejection;
- user-owned decision routing;
- optional memory downstream-action quality;
- ACP interoperability when ACP becomes part of a shipped path.

Package bake-offs may have temporary experimental tests that do not become permanent merge gates unless the package is adopted.

## CI

`./scripts/bootstrap.sh` installs and verifies the pinned Prime substrate in isolated repository state.

`./scripts/conformance.sh` then runs:

1. strict TypeScript typecheck;
2. small credential-free Tier-1 policy/workaround tests;
3. isolated real-Prime runtime conformance sequentially;
4. a final process sweep.

GitHub Actions runs the same harness on macOS and Linux. A local pass is evidence; the merge gate is CI.

## Review rule

A reviewer must ask **"should this test and the component it protects still exist?"** before asking whether the assertion is correct.

Deleting obsolete tests is required when their owning subsystem is deleted or replaced by stronger package-level evidence.
