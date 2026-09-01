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

Published OpenAI plan documentation is recorded as compatibility evidence, but it
is not treated as an authorization oracle. ADR 0006 records a stronger empirical
fact for the actual target: on 2026-08-31 the target ChatGPT Pro
account/app/surface successfully performed state-changing Tandem MCP actions and
verified the resulting host-filesystem mutation.

Command Governor therefore binds support to a harmless synthetic mutation/read-back
on the exact account/app/surface and a fenced `capability_epoch`, not to the plan
label. Capability is revalidated after relevant connector/account/product/ABI
changes or repeated action rejection.

Command Governor does not react to capability loss by:

- treating assistant/browser settlement as ACK;
- labeling a mutation as read-only;
- bypassing product confirmations;
- weakening the explicit-ACK invariant.

A surface whose current probe cannot perform the required state-changing action is
unsupported for that epoch and its obligations remain open. Tool-mount failures,
write denial, confirmation requirements, connector reachability, and ABI mismatch
remain distinct diagnostics.

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

The acceptance suite carries a fourteen-sentinel corpus covering prompt, cwd,
raw tool arguments and results, shell command, transcript path, terminal
transcript, provider intermediate records, browser cookies/tokens/headers/
bodies, GitHub auth, and environment secrets. Two different things are proven
about it, and they are not interchangeable:

- **Ten classes are structurally unrepresentable.** They contain a space, a
  quote, a newline, a brace or a `/`, so `SafeToken`'s charset refuses them and
  no caller can present one to any API. That is proven by the charset itself
  (`governor-core` `fence`:
  `refuses_shapes_that_could_carry_forbidden_content`), by the corpus being
  checked against the charset rather than trusted (`governor-testkit`
  `sentinels`: `the_charset_claim_matches_reality`, re-asserted at the top of
  each SEC-001 acceptance test), and by the schema having nowhere to put them
  (`governor-store-sqlite` `store_privacy`:
  `the_schema_has_no_column_for_forbidden_content`, which pins the full column
  list, and `safe_metadata_never_holds_a_provider_shaped_document`). All ten are
  additionally byte-scanned for, which is a check that nothing *else* wrote
  them.
- **Four classes are token-shaped and are injected for real.** A session cookie,
  an `sk-proj` key, a `ghp_` token and an environment secret are strings the
  charset cannot distinguish from a legitimate opaque identity, so the weaker
  claim is the honest one and it is proven empirically. Each is pushed through
  a real public request field that would accept it — `display_name`,
  `worker_turn_ref`, `source_issue_ref`, and a wake's accepted message ref —
  driven through a full lifecycle, and then shown to have reached exactly the
  column it was written to and nothing else: not another table, not
  `safe_metadata_json`, not an artifact, not a log line, not CLI output, not an
  error string. See `governor-testkit` `sentinels::INJECTED` for the mapping,
  and `sec_acceptance`
  (`sec_001_injected_token_shaped_sentinels_reach_one_column_each`) plus the
  daemon suite's SEC-001 test for the two lifecycles that carry them.

The scanned surfaces are SQLite, WAL and SHM, every artifact, staging and
quarantine file, `logs/`, and every CLI stdout and stderr. The only allowed
provider-content exception is an explicitly designated bounded final-result
candidate/artifact when the test intentionally places the sentinel in the final
assistant result; its confinement to `artifacts/objects/` is asserted
separately.

Delivery/restart tests also prove:

- accepted/ambiguous external writes are not replayed automatically;
- deterministic delivery metadata cannot reconstruct the random wake correlation
  ID;
- low-information bootstrap cannot claim current work;
- Stop-hook callback/veto races cannot fabricate completion;
- same-user file modes are not described as a worker sandbox.

See [`docs/testing.md`](docs/testing.md) for the exact test matrix.
