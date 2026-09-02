# Security Policy

## Supported versions

Command Governor is pre-release and does not yet publish supported versions. Security reports about the design, repository, pinned substrate, package composition, or implementation are welcome.

The current security model is in [`docs/threat-model.md`](docs/threat-model.md). Older pre-ADR-0008 Rust/browser/MCP documents are historical provenance, not the current product topology.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or include sensitive logs, credentials, prompts, conversation contents, repository source, browser profile data, or session data in a public report.

Use GitHub private vulnerability reporting for this repository:

<https://github.com/DivMode/commandgovernor/security/advisories/new>

Include the affected component/version/commit, impact, a minimal safe reproduction when possible, and any suggested mitigation.

## Current trust model

Command Governor is a local-first Prime/Pi harness distribution.

The local OS user is the administrative trust root for the trusted-local profile. Prime, selected packages, Command Governor extensions, workers, and tools may normally execute with that user's authority unless a separate isolation profile is explicitly used.

Owner-only files are privacy against other OS principals and accidental exposure; they are **not** a hostile same-user sandbox.

Sandboxing remains optional hardening for intentionally untrusted repositories, skills, packages, or tools. The core trusted-local product does not claim same-user containment.

## Security boundaries that matter now

### Component and supply-chain integrity

- Prime and third-party packages must be pinned to an exact version/revision with license/provenance recorded.
- The bootstrap path verifies the selected Prime release assets against committed integrity metadata.
- One component owns each authority concern; overlapping generic lifecycle/task/memory/transport owners are rejected rather than resolved implicitly by load order.
- A package is not trusted merely because it is popular or already used by another Pi harness.

### Environment and credential boundary

Command Governor must not forward the host environment wholesale to Prime/worker paths.

Use a positive allowlist for variables that are intentionally granted. Credentials, browser session material, GitHub auth, provider keys, and unrelated environment values stay outside worker/package reach unless explicitly required by the selected capability.

### External-effect ambiguity

A lost transport or worker after an external mutation cannot be treated as proof that the effect did not occur.

The current Prime worker-loss defect is guarded by a temporary D2 compatibility layer while the package-shaped product path is tested. Unknown effect state fails closed; it is not blindly repeated under a fresh identity.

When the package path proves the custom D2 owner unnecessary, both the workaround and its implementation-specific tests must be removed.

### Session/revision identity

Stale session incarnation, cursor, task revision, delivery identity, or foreman correlation must not mutate newer work as though it were current.

Prime should own generic session identity/recovery where its public package/runtime surfaces satisfy the requirement. Custom D1 state remains temporary until the package-path reproducer proves whether it is needed.

### ChatGPT foreman transport

Any ChatGPT Web integration is unofficial unless the relevant provider exposes an official supported API for the use case.

The product requirement is exact-target correlation and fail-closed handling of ambiguous submission. Existing Pi-native transports are evaluated before bespoke browser automation. Command Governor does not define CAPTCHA, anti-abuse, entitlement, or rate-limit bypass as a product feature.

### Prompt injection and untrusted content

Repository text, worker output, tool output, webpages, and external messages are data, not policy authority.

Untrusted content cannot silently widen capabilities, change component ownership, satisfy independent review, or convert a user-owned decision into an automatic action.

## Sensitive data policy

Do not intentionally persist or publish credentials, browser cookies/tokens, raw private transport headers/bodies, provider keys, GitHub auth, or arbitrary environment snapshots as routine Command Governor control data or CI evidence.

The product should prefer references, bounded result/evidence artifacts, and the minimum state required for reliable coordination. A selected package's normal session/transcript storage is governed by that package/Prime boundary; Command Governor should not create a second raw transcript/provider-stream store merely for orchestration.

## Security testing

The active merge-gating suite is intentionally product-oriented; see [`docs/testing.md`](docs/testing.md).

Current security-relevant conformance includes:

- exact substrate pin/integrity checks;
- one-authority/disposition checks;
- positive environment allowlist behavior;
- D1 stale-incarnation/recovery behavior while that workaround exists;
- D2 worker-loss uncertainty/no-duplicate behavior while that workaround exists;
- explicit persistent session paths;
- completed-command idempotence and process-safe session ownership;
- zero unexpected Prime fixture processes after tests.

Do not restore the retired standalone Rust test universe merely to preserve old security assertions. When a historical invariant still matters, prove it at the current Prime/package boundary.

## Non-claims

Command Governor does not currently claim:

- hostile same-user worker containment;
- official OpenAI/ChatGPT Web automation support;
- that an experimental package is safe merely because it passed one bake-off;
- that a model-generated memory/summary is authority for exact lifecycle, permission, or user-owned facts.
