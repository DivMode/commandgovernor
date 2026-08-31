# Security Policy

## Supported versions

Command Governor is pre-release and does not yet publish supported versions.
Security reports about the design, repository, or future implementation are still
welcome.

The detailed V1 design is documented in [`docs/threat-model.md`](docs/threat-model.md).

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or include sensitive
logs, credentials, prompts, conversation contents, result artifacts, repository
source, browser profile data, or session data in a public report.

Use GitHub's private vulnerability reporting for this repository:

<https://github.com/DivMode/commandgovernor/security/advisories/new>

Include:

- affected component/document/version/commit;
- impact and conditions required to reproduce;
- a minimal reproduction when safe;
- suggested mitigation if known; and
- whether the issue is already public or under active exploitation.

Maintainers will validate impact and coordinate remediation/disclosure. No
response deadline is promised while the project is pre-release.

## Security boundaries

Command Governor coordinates tools capable of reading source code, running
processes, interacting with GitHub, and controlling an authenticated ChatGPT
browser. Reports involving these areas are especially important:

- authorization and project/session ownership;
- stale binding/generation/claim bypass;
- duplicate browser or worker delivery;
- ambiguous external-I/O recovery;
- wrong-conversation browser submission;
- MCP mutation from an unrelated/stale ChatGPT conversation;
- worker lifecycle/runtime-state conflicts;
- managed Claude hook/settings tampering;
- credential/profile leakage;
- result-artifact tamper/path traversal;
- SQLite/event/projection corruption;
- prompt injection crossing from repository/worker data into control policy;
- unsafe dependency/supply-chain changes.

## Local trust model

V1 is single-user/local-first. The local OS user is the administrative trust root.
State, artifacts, hook inboxes, browser profiles, and local IPC are owner-private.

This does not claim to contain a fully compromised process already running as the
same OS user. Same-user malware can generally reach credentials available to the
user. The design still minimizes accidental exposure, cross-user access, and
credential copies.

## Sensitive data policy

Routine events/logs/diagnostics must not persist:

- prompt text;
- raw tool arguments;
- shell commands;
- cwd;
- Claude transcript path;
- terminal transcript;
- browser cookies/session tokens;
- GitHub authentication material;
- arbitrary environment variables;
- raw private ChatGPT request/response dumps.

A bounded final worker result that must survive runtime shutdown is stored only in
an explicit private result-artifact boundary, not in the general event ledger or
wake message. Open obligations pin required result artifacts until a closing
foreman disposition and retention policy permit cleanup.

The dedicated ChatGPT browser profile is credential-equivalent. Command Governor
must not export its cookies/tokens into a standalone private API client as the
normal architecture.

## ChatGPT Web adapter posture

The ChatGPT Web browser adapter is unofficial. Its security posture is:

- user signs in normally through the visible first-party browser UI;
- one dedicated local browser profile;
- exact conversation binding and generation fencing;
- real ChatGPT SPA performs sensitive submission;
- CDP observes only the minimum evidence needed for delivery/reconciliation;
- no CAPTCHA/Turnstile/Sentinel/proof-of-work bypass;
- no entitlement bypass;
- no rate-limit/anti-abuse circumvention;
- no claim of OpenAI endorsement or official ChatGPT Web automation support.

If normal first-party browser interaction or supported MCP connectivity does not
work, Command Governor fails visibly rather than adding bypass machinery.

## OpenAI terms / compatibility risk

Command Governor is an independent open-source project. Users and contributors
should review the terms applicable to their use of ChatGPT/OpenAI services.
Current sources reviewed during architecture work include:

- OpenAI Terms of Use, effective 2026-01-01:
  <https://openai.com/policies/terms-of-use/>
- OpenAI App Developer Terms, updated 2026-07-09:
  <https://openai.com/policies/app-developer-terms/>

Those terms contain restrictions relevant to reverse engineering, automated data
extraction, restrictions/rate limits, and protective measures. The local
normal-login/no-bypass architecture reduces some risk but is **not** a legal
conclusion that a particular use is permitted and does not eliminate account or
compatibility risk.

If OpenAI provides an official foreman/wake API, the architecture intentionally
allows replacement/removal of `governor-chatgpt-web` without rewriting the control
plane.

## User-owned authorization

A worker asking for permission does not grant that permission. The ChatGPT foreman
may answer ordinary engineering coordination questions within recorded delegated
scope, but destructive, credential-sensitive, materially broader, or unknown
permission requests are user-owned by default. Command Governor must fail closed
with an explicit authorization requirement.

## Security testing policy

The acceptance suite injects sentinel secrets into prompt/cwd/tool/transcript/
browser/GitHub fields and byte-scans SQLite, WAL, hook inbox, logs, safe
diagnostics, and crash state for leakage. Delivery/restart tests also prove that
accepted/ambiguous external writes are not replayed automatically.

See [`docs/testing.md`](docs/testing.md) for the exact test matrix.
