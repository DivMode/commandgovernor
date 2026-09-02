# Contributing to Command Governor

Thank you for helping build Command Governor. The project is design-first: architecture boundaries and failure behavior matter more than implementation volume.

## Before proposing or retaining production code

Read ADRs 0008–0010, the current composition/de-duplication research, and `docs/testing.md`.

Global Claude Code and Codex instructions are managed declaratively outside this repository by the user's Nix configuration. Do not add repo-local `CLAUDE.md` or `AGENTS.md` copies of global policy here.

For each meaningful custom capability, answer before implementation:

1. What current Prime/Pi capability or existing package was checked first?
2. What exact product requirement remains unmet?
3. Is the outcome `USE EXISTING`, `PLUGIN`, `TEMP WORKAROUND`, or `DELETE`?
4. Why is the proposed owner the smallest surface that can satisfy the requirement?

A change is not justified merely because its implementation is correct, already merged elsewhere, heavily tested, or expensive to discard.

## Before proposing a change

1. Search existing issues, pull requests, accepted ADRs, and current research.
2. Open an issue for changes that affect architecture, lifecycle semantics, persistence, security boundaries, or external integrations.
3. Keep proposals provider-neutral unless a provider-specific adapter is the explicit subject of the change.
4. If the proposal changes an authority boundary established by ADRs 0008–0010, update or supersede the ADR before the implementation grows around that new boundary.

## Pull requests

- Keep each pull request focused on one coherent outcome.
- Complete the architecture/existence section in the PR template before implementation correctness is reviewed.
- Explain the failure modes considered and the invariant that prevents each one.
- Add or update tests only for current product behavior or a named temporary workaround.
- Prefer black-box/package conformance over tests of internal implementation shape.
- Remove implementation-specific tests when their owning subsystem is removed or superseded.
- Update architecture documentation and ADRs when behavior or boundaries change.
- After an architecture pivot, preserve useful research and close or retarget stale implementation PRs rather than leaving them as accidental future instructions.
- Do not commit credentials, private prompts, conversation contents, session transcripts, or other user data.
- Do not vendor third-party code without documenting its source, license, and required notices in `THIRD_PARTY_NOTICES.md`.

## Test discipline

A test must map to a product invariant or to a `TEMP WORKAROUND` with a deletion condition. Coverage percentage, test count, and historical bug count are not goals by themselves.

Do not keep multiple permanent suites that prove the same invariant at weaker/internal boundaries. Do not pre-build acceptance matrices for components that have not been selected.

For test-heavy changes, review both questions:

1. **Does this assertion protect a real current invariant?**
2. **Should the component it protects still exist?**

## Design expectations

Changes should preserve these baseline properties:

- Prime/Pi/current reviewed packages own generic substrate capabilities where they already satisfy the requirement;
- durable state is authoritative over process labels and UI observations where Command Governor actually owns durable state;
- retries are explicit and safe for the operation involved;
- ambiguous external submission is reconciled, not blindly repeated;
- worker results remain discoverable until their required disposition closes;
- adapters cannot silently weaken lifecycle guarantees;
- crash and restart behavior is specified and testable;
- independent review cannot be satisfied by implementer self-certification.

## Review order

Reviewers answer **"Should this code exist in Command Governor?"** before reviewing implementation quality.

If the existence proof is missing, stop the review and resolve the architecture/ownership question first.

By contributing, you agree that your contribution is licensed under the MIT License in this repository.
