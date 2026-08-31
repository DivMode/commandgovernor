# ADR 0006: Empirical ChatGPT MCP capability gate

- **Status:** Accepted; supersedes plan-name capability assumptions in earlier V1 docs
- **Date:** 2026-08-31

## Context

Command Governor requires real state-changing MCP operations for the foreman loop:
`foreman_resume`, `foreman_ack`, and `foreman_answer_input` cannot be simulated by
browser state or mislabeled as read-only operations.

During the initial architecture review, OpenAI's published developer-mode guidance
was interpreted as a hard plan boundary: consumer ChatGPT Pro was documented as
read/fetch-only, while full modify/write custom MCP was described for Business,
Enterprise, and Edu. That led the draft architecture to classify consumer Pro as
categorically unsupported.

A live capability test on 2026-08-31 disproved that categorical assumption for the
actual target account/surface.

Using a fresh ChatGPT conversation with the private Tandem app explicitly attached,
ChatGPT successfully performed these state-changing MCP operations:

1. `list_sessions` on device `local`;
2. `open_session` for disposable session `tandem-pro-mcp-write-proof`;
3. `send_to_session` with an instruction that caused Claude Code to create and
   overwrite a host filesystem file;
4. Claude read the file back and returned exactly `MCP WRITE VERIFIED`;
5. Tandem returned the turn as `done`.

Observed identifiers from the disposable proof:

- Tandem session: `local:tandem-pro-mcp-write-proof`
- Herdr session: `tandem-134a8abf462b`
- disposable cwd: `/Users/peter/Developer/tandem-proof-sandbox`

There was no plan restriction, read-only error, confirmation-policy rejection, or
permission failure on either `open_session` or `send_to_session`.

This is direct evidence that the tested ChatGPT Pro account/surface can currently
perform state-changing custom MCP actions despite the published plan matrix.

## Decision

**Command Governor support is capability-based, not plan-name-based.**

`command-governor chatgpt bind` must execute a harmless synthetic mutation against
the actual bound ChatGPT account/app/surface. The runtime support decision is based
on that observed result.

A plan label such as `Pro`, `Business`, `Enterprise`, or `Edu` is diagnostic
metadata only. It is never sufficient by itself to approve or reject the durable
foreman loop.

Gate A therefore requires proof of the actual operations Command Governor needs:

1. the connector/app is mounted for the message;
2. a synthetic state-changing tool is callable;
3. the mutation reaches the MCP server and commits durable state;
4. the result can be read back and correlated to the exact mutation;
5. stale binding generation is rejected;
6. confirmation/permission behavior does not make unattended correctness
   impossible under the configured policy;
7. subsequent tool-schema/app refresh behavior is understood for the bound
   surface.

If the mutation succeeds, the surface is write-capable for that tested capability
epoch regardless of the public plan table. If it fails because writes are truly
unavailable, the surface is unsupported until a later successful preflight.

## Documentation discrepancy policy

Published OpenAI documentation remains important compatibility evidence and should
be recorded with date/source. It does not override stronger direct capability
evidence from the exact account/surface Command Governor will use.

Conversely, one successful test is not a permanent entitlement guarantee. Product
behavior can change. Capability is therefore versioned/fenced by a
`capability_epoch` and revalidated after relevant changes such as:

- app/connector recreation or refresh;
- account/workspace/plan change;
- ChatGPT product update that changes action availability;
- MCP ABI change;
- repeated action rejection indicating capability drift.

A previously successful capability probe does not authorize silent fallback if
writes later stop working. Obligations remain open and `doctor` reports the
capability failure.

## Tool-mount failures are distinct from write denial

A ChatGPT turn where the app is selected but its tools are not mounted is not
classified as `write_capability_unavailable` unless an actual write invocation is
rejected for capability reasons.

Keep at least these failure classes separate:

- `app_tools_not_mounted`
- `write_action_unavailable`
- `write_action_rejected`
- `confirmation_required`
- `connector_unreachable`
- `connector_abi_mismatch`

This distinction matters because a tool-mount/runtime problem says nothing about
whether the account is entitled to perform the write once the app is actually
available.

## Security consequence

This ADR does **not** weaken explicit ACK or any other safety invariant.

- browser submission is still not ACK;
- physical ChatGPT turn settlement is still not ACK;
- a read-only tool is never allowed to mutate state;
- every mutation remains fenced by binding generation, obligation/source/version,
  claim, and the accepted random wake correlation ID;
- the synthetic Gate A test never touches a real engineering obligation.

## Superseded statements

Any earlier Command Governor V1 document that states or implies
"consumer ChatGPT Pro is categorically unsupported because its plan is
read/fetch-only" is superseded by this ADR.

The correct statement is:

> Current published product documentation may describe plan-level availability,
> but Command Governor determines support by a harmless live mutation probe on the
> exact bound account/app/surface. The target Pro account demonstrated successful
> state-changing Tandem MCP actions on 2026-08-31.

Those older documents should be mechanically cleaned before the architecture PR is
merged so the prose is fully consistent with this accepted decision.
