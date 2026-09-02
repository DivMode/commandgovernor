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

- the durable intent/claim written before I/O;
- the exact ambiguity boundary;
- accepted/failed/ambiguous evidence;
- restart behavior; and
- why an ambiguous side effect cannot be blindly replayed.

Write "None" when the change performs no consequential external write.

## Security / data boundary

Describe any new persisted/logged fields and prove they do not introduce prompt, cwd, raw tool arguments, terminal transcript, browser credentials/tokens, GitHub auth, or other forbidden data. State how untrusted worker/repository content is kept separate from Command Governor control fields.

## Verification

List exact checks performed and results. Executable changes should map tests to the acceptance IDs in `docs/testing.md` where applicable.

## Checklist

- [ ] I answered "should this code exist?" before "is this code correct?"
- [ ] The capability is classified as `USE EXISTING`, `PLUGIN`, `TEMP WORKAROUND`, or `DELETE`.
- [ ] The change does not recreate a generic Prime/Pi/package authority without a demonstrated gap.
- [ ] The change is focused and documented.
- [ ] It preserves the central invariant: open delegated work cannot disappear without an explicit closing disposition.
- [ ] Failure, ambiguity, and restart behavior are defined where relevant.
- [ ] Stale session incarnation / binding generation / claim fences are handled where relevant.
- [ ] Tests were added or updated for executable behavior, including crash/failure injection where relevant.
- [ ] No credentials, raw prompts, cwd, raw tool arguments, terminal transcripts, browser protocol dumps, or session secrets are included in safe state/logs.
- [ ] Browser accepted / ChatGPT settled / foreman ACK are not conflated.
- [ ] Third-party provenance and notices were updated if code/material was introduced.
- [ ] A changed architecture/authority boundary includes an ADR update before implementation expansion.