## Summary

Describe the outcome, why it is needed, and which architecture/ADR contract it implements or changes.

## Architecture / existence proof

Before correctness review, answer:

- Which current Prime/Pi capability or existing package was evaluated first?
- What exact requirement remains unmet?
- Classification: `USE EXISTING` / `PLUGIN` / `TEMP WORKAROUND` / `DELETE`.
- Why is this the smallest owning surface?
- If `TEMP WORKAROUND`, what upstream defect/reproducer proves it is needed on the actual product path, and what removes it later?

If this section does not demonstrate why custom production code should exist, stop the review before evaluating implementation quality.

## Lifecycle impact

Describe affected identities, source events, projection states, obligations, claims, retries/resumes, binding generations, and recovery behavior. Write "None" only when the change cannot affect lifecycle truth.

## External I/O / ambiguity impact

For browser, worker, runtime, GitHub, or other external writes, identify:

- the durable intent/claim written before I/O when Command Governor owns one;
- the exact ambiguity boundary;
- accepted/failed/ambiguous evidence;
- restart behavior; and
- why an ambiguous side effect cannot be blindly replayed.

Write "None" when the change performs no consequential external write.

## Security / data boundary

Describe any new persisted/logged fields and prove they do not introduce credentials, raw private transport data, or unnecessary user/repository content. State how untrusted worker/repository content is kept separate from Command Governor control policy.

## Test ownership

For every added/retained test family, state:

- the product invariant or named `TEMP WORKAROUND` it protects;
- why this is the strongest useful boundary;
- whether a stronger existing black-box/package test makes a weaker test redundant; and
- for a `TEMP WORKAROUND`, the condition that deletes the test with the workaround.

Do not use test volume as an existence proof for production code.

## Verification

List exact checks performed and results. Map merge-gating behavior to `docs/testing.md` where applicable.

## Checklist

- [ ] I answered "should this code exist?" before "is this code correct?"
- [ ] The capability is classified as `USE EXISTING`, `PLUGIN`, `TEMP WORKAROUND`, or `DELETE`.
- [ ] The change does not recreate a generic Prime/Pi/package authority without a demonstrated gap.
- [ ] Tests protect current product behavior or a named temporary workaround, not obsolete implementation shape.
- [ ] Redundant weaker test families were removed when stronger package/black-box evidence exists.
- [ ] The change is focused and documented.
- [ ] Failure, ambiguity, and restart behavior are defined where relevant.
- [ ] Stale identity/revision/generation/claim fences are handled where relevant.
- [ ] No credentials or raw private transport/session data are included in safe state/logs/evidence.
- [ ] Third-party provenance and notices were updated if code/material was introduced.
- [ ] A changed architecture/authority boundary includes an ADR update before implementation expansion.
