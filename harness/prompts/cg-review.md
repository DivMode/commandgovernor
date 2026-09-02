---
description: Review a change against its requirement, from the source rather than from the summary
argument-hint: <commit, branch, or merge-base to review since>
---

Review the changes since `${1:-HEAD~1}` in this repository.

Establish the repository root from the checkout before reading anything: run
`pwd` and `git rev-parse --show-toplevel` and require both to resolve to the same
absolute path.

Read the diff, then read the source around it — the callers, the tests that were
meant to cover it, and the invariant it was meant to preserve. Then find the
requirement it was answering: the issue, the ADR, the specification, or the
commit message that states the intent.

Report two axes separately.

**Correctness.** Does the code do what it claims on the paths nobody exercised —
error handling, empty input, concurrent access, restart, partial failure? Name
the file and line for each finding.

**Specification.** Does it do what was asked? A correct implementation of the
wrong requirement is the failure most likely to survive a careful reading of the
code alone.

Then answer three questions explicitly:

1. What did you verify by running something, and what is the actual output?
2. What did you read but not execute?
3. What could you not check at all, and why?

Do not review from the implementer's account of the work. If the summary and the
source disagree, that disagreement is the most important thing in your report.

Rank findings by what shipping them would cost. Do not pad the list; stylistic
notes buried among real defects hide the real defects.
