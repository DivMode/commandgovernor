# Security Policy

## Supported versions

Command Governor is pre-release and does not yet publish supported versions.
Security reports about the design, repository, or future implementation are
still welcome.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or include sensitive
logs, credentials, prompts, conversation contents, or session data in a public
report.

Use GitHub's private vulnerability reporting for this repository:

<https://github.com/DivMode/commandgovernor/security/advisories/new>

Include:

- the affected component or document;
- the impact and conditions required to reproduce it;
- a minimal reproduction when safe;
- any suggested mitigation; and
- whether the issue is already public or under active exploitation.

Maintainers will acknowledge a report as soon as practical, validate its
impact, and coordinate remediation and disclosure. No response deadline is
promised while the project is pre-release.

## Security boundaries

Command Governor will coordinate tools capable of reading source code, running
processes, and interacting with authenticated services. Reports involving
authorization, credential handling, cross-project isolation, conversation
binding, duplicate delivery, or durable-state corruption are especially
important.
