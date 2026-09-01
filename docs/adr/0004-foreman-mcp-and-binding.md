# ADR 0004: Stable foreman MCP ABI, exact binding generation, explicit ACK

- **Status:** Proposed, gated by a write-capable ChatGPT workspace surface
- **Date:** 2026-08-31

## Context

A browser wake only causes a ChatGPT turn. It does not prove the foreman fetched
the worker result, reviewed it, answered input, or acknowledged the obligation.
The control plane therefore needs a durable authenticated tool path back from the
foreman.

Long-lived ChatGPT conversations/apps can retain tool schemas, and current app
updates require explicit refresh/action enablement. The public ABI must be small
and stable.

Published OpenAI developer-mode guidance reviewed earlier on 2026-08-31 described
a plan-level split that suggested consumer Pro custom MCP should be treated as
read/fetch-only while Business/Enterprise/Edu had broader modify/write support.
ADR 0006 supersedes using that documentation as a categorical support decision: a
live test on the exact target ChatGPT Pro account/app/surface successfully performed
state-changing Tandem MCP actions and verified the mutation by read-back.

The architecture therefore treats published plan documentation as compatibility
evidence and the live capability probe as the support authority for the exact
bound surface. A write-capable ACK still cannot be faked as a read.

## Decision

Use the official Rust MCP SDK (`rmcp`) and publish one stable V1 connector ABI:

```text
command-governor-foreman/v1
```

Initial tools:

- `foreman_bootstrap`
- `foreman_resume`
- `foreman_ack`
- `foreman_answer_input`

`foreman_bootstrap` is read-only. The other three are truthful state-changing
operations: resume creates a claim, ACK closes a processed obligation, and input
answer records a decision/schedules a separate worker-resume delivery.

Breaking public tool semantics require a new connector ABI/explicit refresh, not
an invisible mutation under an old conversation.

## Exact binding and wake correlation

One active ChatGPT foreman conversation is stored with monotonic
`binding_generation`. Every mutation requires the current generation plus
obligation/source/claim fences.

MCP does not currently supply a documented trustworthy ChatGPT conversation ID as
the mutation principal. Therefore browser delivery uses two different identities:

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)

delivery_id = CSPRNG(>=192 bits)
```

`delivery_key` is a deterministic, non-secret idempotency/deduplication key. It
never authorizes a mutation.

`delivery_id` is a random opaque correlation ID generated once when the durable
delivery is created. The browser wake contains it; bootstrap/status never return
it. `foreman_resume` requires the exact current accepted `delivery_id`, and the
daemon verifies that it belongs to the obligation/current generation/current
version. Resume then mints a claim ID; ACK/input mutation requires that claim.

The random delivery ID is an anti-confusion possession fence, not a replacement
for connector authentication. A caller that knows only an obligation ID,
generation, and revision cannot derive it.

## Bootstrap confidentiality

`foreman_bootstrap` is intentionally low-information because the connector may be
visible from another conversation in the same authenticated workspace. It may
return health, counts, urgency classes, compatibility and active binding
generation, but it does not disclose repository/project refs, result contents,
worker/session refs, or the current accepted delivery ID required for mutation.
The actual bound wake carries the opaque IDs needed for `foreman_resume`.

## ACK semantics

Normal work closes only through explicit `foreman_ack` with:

- exact obligation ID/version;
- source event fence;
- current binding generation;
- current claim ID;
- valid semantic disposition.

Browser accepted != ChatGPT physically settled != ACK.

A stale/expired/different disposition ACK cannot rewrite terminal state. An exact
repeat of an already committed identical ACK may return idempotent success.

## Reachability

Use the currently supported OpenAI Secure MCP Tunnel/connectivity path rather than
publishing an unauthenticated local MCP listener. A stdio shim or loopback endpoint
may adapt the tunnel to the daemon, but it owns no orchestration state.

## Capability gate

Support is **capability-based, not plan-name-based**, as accepted by ADR 0006.
`command-governor chatgpt bind` must feature-test the exact bound
account/app/surface with a harmless synthetic state mutation and read-back. The
probe must also prove stale binding-generation rejection and characterize current
confirmation behavior.

The support record is fenced by `capability_epoch`. Plan/workspace/model labels are
recorded as diagnostics but are not sufficient to approve or reject the foreman
loop. Revalidate after connector/app recreation or refresh, account/workspace/plan
changes, relevant ChatGPT product changes, MCP ABI changes, or repeated action
rejection that suggests capability drift.

Keep tool-mount/runtime failures distinct from actual write denial. At minimum:

- `app_tools_not_mounted`
- `write_action_unavailable`
- `write_action_rejected`
- `confirmation_required`
- `connector_unreachable`
- `connector_abi_mismatch`

In particular:

- assistant settlement cannot substitute for ACK;
- browser DOM events cannot substitute for ACK;
- a read-only tool is not mislabeled to mutate state;
- a product confirmation that the model cannot legitimately complete unattended
  is not bypassed;
- a previously successful capability epoch does not authorize silent fallback if
  writes later stop working.

## Alternatives

### Treat assistant completion as ACK

Rejected because a ChatGPT response can finish without calling Command Governor,
without reviewing the result, or after a connector/tool failure.

### Deterministic delivery ID as possession token

Rejected. A deterministic unkeyed value derived from obligation/generation/revision
is suitable as an idempotency key but is not an unguessable possession fence when
those inputs are observable or enumerable. V1 therefore separates deterministic
`delivery_key` from random `delivery_id`.

### One MCP tool per internal feature

Rejected because schema caching/refresh makes tool-list churn an operational
compatibility hazard.

### Generic arbitrary `action` dispatcher

Rejected for V1 because it weakens per-tool safety semantics and creates a broad
command surface. The four stable tools have versioned additive result fields.

### Browser-only ACK

Rejected. The browser is delivery/observation transport and should not infer the
foreman's semantic review decision from UI text.

## Consequences

The V1 support matrix is dynamic and evidence-based rather than hard-coded from
plan names. The target Pro surface demonstrated state-changing Tandem MCP on
2026-08-31, but Command Governor still must run its own exact synthetic mutation,
stale-generation, confirmation, and ABI preflight before binding that surface.

A later capability failure leaves obligations open and marks the surface
unsupported for the current capability epoch. That is preferable to claiming a
durable review loop that cannot actually close obligations correctly.
