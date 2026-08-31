# Contributing to Command Governor

Thank you for helping build Command Governor. The project is at an early,
design-first stage, so clear invariants and failure behavior matter more than
implementation volume.

## Before proposing a change

1. Search existing issues and pull requests.
2. Open an issue for changes that affect architecture, lifecycle semantics,
   persistence, security boundaries, or external integrations.
3. Keep proposals provider-neutral unless a provider-specific adapter is the
   explicit subject of the change.

## Pull requests

- Keep each pull request focused on one coherent outcome.
- Explain the failure modes considered and the invariant that prevents each
  one.
- Add or update tests once an executable implementation exists.
- Update architecture documentation and ADRs when behavior or boundaries
  change.
- Do not commit credentials, private prompts, conversation contents, session
  transcripts, or other user data.
- Do not vendor third-party code without documenting its source, license, and
  required notices in `THIRD_PARTY_NOTICES.md`.

## Design expectations

Changes should preserve these baseline properties:

- durable state is authoritative over process labels and UI observations;
- retries are explicit and safe for the operation involved;
- ambiguous browser submission is reconciled, not blindly repeated;
- worker results remain discoverable until their consumption obligation closes;
- adapters cannot silently weaken lifecycle guarantees;
- crash and restart behavior is specified and testable.

## Development workflow

Implementation tooling and commands will be documented when the technology
stack is selected. Until then, documentation changes should use clear Markdown,
relative repository links, and line lengths that remain readable in review.

By contributing, you agree that your contribution is licensed under the MIT
License in this repository.
