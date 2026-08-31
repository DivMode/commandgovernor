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

Current OpenAI published product behavior also matters. As of this review,
consumer **ChatGPT Pro custom MCP is read/fetch-only** in developer mode; full
custom MCP modify/write actions are currently documented for Business,
Enterprise, and Edu beta surfaces. Business currently exposes the GPT-5.6 Sol Pro
model, so a Business workspace can satisfy the desired Pro-model foreman role if
the live action/confirmation gate passes. A write-capable ACK cannot be faked as a
read.

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

Consumer ChatGPT Pro is **not currently an end-to-end V1 foreman target** because
its custom MCP surface is documented read/fetch-only. The Rust kernel does not
weaken its invariant to accommodate that limitation.

For Business/Enterprise/Edu candidate surfaces, `chatgpt bind` must feature-test
the real account/workspace with a synthetic safe mutation and must characterize
confirmation behavior. If state-changing MCP actions are unavailable, blocked, or
require an interaction incompatible with the intended unattended loop, binding
records the exact unsupported capability state.

In particular:

- assistant settlement cannot substitute for ACK;
- browser DOM events cannot substitute for ACK;
- a read-only tool is not mislabeled to mutate state;
- a product confirmation that the model cannot legitimately complete unattended
  is not bypassed.

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

The V1 architecture currently supports fewer ChatGPT plan/surface combinations
than desired. A Business/Enterprise/Edu workspace with a supported Pro model is a
candidate; consumer Pro is not. That is preferable to claiming a durable review
loop that cannot actually close obligations correctly.
