# ADR 0004: Stable foreman MCP ABI, exact binding generation, explicit ACK

- **Status:** Proposed, gated by ChatGPT capability preflight
- **Date:** 2026-08-31

## Context

A browser wake only causes a ChatGPT turn. It does not prove the foreman fetched
the worker result, reviewed it, answered input, or acknowledged the obligation.
The control plane therefore needs a durable authenticated tool path back from the
foreman.

Long-lived ChatGPT conversations/apps can retain tool schemas, and current app
updates require explicit refresh/action enablement. The public ABI must be small
and stable.

Current OpenAI published product behavior also matters: as of this review, custom
MCP on Pro cannot be assumed to expose arbitrary state-changing actions, while
full MCP actions are documented for other plan/beta combinations. A write-capable
ACK cannot be faked as a read.

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

## Exact binding

One active ChatGPT foreman conversation is stored with monotonic
`binding_generation`. Every mutation requires the current generation plus
obligation/source/claim fences.

Because MCP does not currently supply a documented trustworthy ChatGPT
conversation ID as the mutation principal, `foreman_resume` also requires the
opaque `delivery_id` from the accepted current-generation browser wake. Bootstrap
never discloses that accepted delivery ID to an unrelated/stale connector turn.
Resume then mints a claim ID; ACK/input mutation requires that claim.

The delivery ID is an anti-confusion correlation nonce, not a replacement for
connector authentication.

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

`chatgpt bind` must feature-test the real account/surface with a synthetic safe
mutation. If state-changing MCP actions are unavailable, binding records
`write_capability_unavailable` and V1 automatic foreman processing is unsupported
on that surface.

The invariant is not weakened. In particular:

- assistant settlement cannot substitute for ACK;
- browser DOM events cannot substitute for ACK;
- a read-only tool is not mislabeled to mutate state.

## Alternatives

### Treat assistant completion as ACK

Rejected because a ChatGPT response can finish without calling Command Governor,
without reviewing the result, or after a connector/tool failure.

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

The V1 architecture may temporarily support fewer ChatGPT plan/surface
combinations than desired. That is preferable to claiming a durable review loop
that cannot actually close obligations correctly.
