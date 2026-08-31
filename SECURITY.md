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
- deterministic delivery metadata being mistaken for a secret;
- wrong-conversation browser submission;
- MCP mutation from an unrelated/stale ChatGPT conversation;
- worker lifecycle/runtime-state conflicts;
- false Claude completion from a vetoable Stop callback;
- non-interactive permission/defer misclassification;
- managed Claude hook/settings tampering;
- raw provider-stream or tool-argument persistence;
- credential/profile leakage;
- result-artifact tamper/path traversal;
- SQLite/event/projection corruption;
- prompt injection crossing from repository/worker data into control policy;
- unsafe dependency/supply-chain changes.

## Local trust model

V1 is single-user/local-first. The local OS user is the administrative trust root.
State, artifacts, hook inboxes, browser profiles, and local IPC are owner-private
against other OS principals.

This is **not a hostile same-user sandbox**. Claude and tools normally execute as
the same OS user as Command Governor, so `0600`/owner-only files cannot contain a
deliberately malicious process that already has that user's authority. V1
minimizes paths and capabilities exposed to workers, does not intentionally export
the general Command Governor state root, validates imported staging/inbox data as
untrusted, and makes no stronger OS-isolation claim.

A future separate-user/sandbox/broker design is required for hostile-worker
containment.

## Sensitive data policy

Routine events/logs/diagnostics, hook inboxes, and managed-run receipts/staging
must not persist:

- prompt text;
- raw tool arguments;
- raw tool results;
- shell commands;
- cwd;
- Claude transcript path;
- terminal transcript;
- complete Claude/provider structured streams;
- browser cookies/session tokens;
- raw private ChatGPT request/response bodies or headers;
- GitHub authentication material;
- arbitrary environment variables.

The managed Claude worker-host parses structured output **online**. Intermediate
provider records, including tool-use/tool-result records, are discarded after the
minimum safe evidence is extracted. The only provider content that may be retained
for durability is one explicitly bounded complete final assistant-result candidate
needed for independent review, plus sanitized run/child-exit receipts.

That bounded final worker result is promoted into an explicit private immutable
result-artifact boundary, not the general event ledger or wake message. Open
obligations pin required result artifacts until a closing foreman disposition and
retention policy permit cleanup.

The dedicated ChatGPT browser profile is credential-equivalent. Command Governor
must not export its cookies/tokens into a standalone private API client as the
normal architecture.

## Browser wake identity

V1 deliberately separates:

- `delivery_key`: deterministic, non-secret idempotency/deduplication identity;
- `delivery_id`: cryptographically random opaque correlation ID generated once
  for the delivery and carried in the exact browser wake.

The random `delivery_id` is not returned by bootstrap/status. `foreman_resume`
requires it in addition to connector authentication and current obligation/
generation/version fences. Deterministic scheduling metadata is never treated as
an unguessable possession secret.

## ChatGPT MCP capability boundary

As of the 2026-08-31 architecture review, OpenAI documents full custom MCP
modify/write actions for ChatGPT Business, Enterprise, and Edu beta surfaces;
consumer ChatGPT Pro custom MCP is read/fetch-only.

Command Governor does not work around that limitation by:

- treating assistant/browser settlement as ACK;
- labeling a mutation as read-only;
- bypassing product confirmations;
- weakening the explicit-ACK invariant.

Consumer Pro is therefore not a supported end-to-end V1 foreman target at this
snapshot. Candidate Business/Enterprise/Edu surfaces must pass the real mutation
and confirmation-behavior gate before support is claimed.

## ChatGPT Web adapter posture

The ChatGPT Web browser adapter is unofficial. Its security posture is:

- user signs in normally through the visible first-party browser UI;
- one dedicated local browser profile;
- exact conversation binding and generation fencing;
- per-message Command Governor app selection is proved before Send;
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
- current ChatGPT developer-mode/MCP documentation:
  <https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt>

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

Current Claude documentation says `PermissionRequest` can run in non-interactive
contexts. V1 may use it as a permission-decision signal, but exact durable
out-of-band pause/resume prefers a confirmed single-tool `PreToolUse` defer with a
stable tool-use fence. Multi-tool defer cannot be projected as a clean pause.

## Security testing policy

The acceptance suite injects sentinel secrets into prompt/cwd/tool-argument/tool-
result/transcript/provider-stream/browser/GitHub fields and byte-scans SQLite,
WAL, hook inbox, managed-run staging/receipts, logs, safe diagnostics, and crash
state for leakage. The only allowed provider-content exception is an explicitly
designated bounded final-result candidate/artifact when the test intentionally
places the sentinel in the final assistant result.

Delivery/restart tests also prove:

- accepted/ambiguous external writes are not replayed automatically;
- deterministic delivery metadata cannot reconstruct the random wake correlation
  ID;
- low-information bootstrap cannot claim current work;
- Stop-hook callback/veto races cannot fabricate completion;
- same-user file modes are not described as a worker sandbox.

See [`docs/testing.md`](docs/testing.md) for the exact test matrix.
