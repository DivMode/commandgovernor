# ChatGPT foreman MCP contract

Status: **V1 ABI proposal**. Implementation uses the official Rust MCP SDK
(`modelcontextprotocol/rust-sdk`, crate `rmcp`).

The MCP surface exists so a woken ChatGPT foreman can fetch durable truth and
explicitly disposition it. MCP is not the wake mechanism. The browser wake is not
the result transport.

## Current capability gate

`foreman_resume`, `foreman_ack`, and `foreman_answer_input` are real mutations, so
the exact bound ChatGPT account/app/surface must prove that mutation class.
Published plan documentation is compatibility evidence rather than the support
authority.

ADR 0006 records a live 2026-08-31 test in which the target ChatGPT Pro surface
successfully performed state-changing Tandem MCP actions and verified the resulting
host-filesystem mutation by read-back. Command Governor therefore uses a harmless
synthetic mutation/read-back during binding, records a `capability_epoch`, and
revalidates after relevant app/account/product/ABI changes or capability drift.

Plan/workspace/model labels remain useful diagnostics. They neither grant nor deny
support by themselves, and no surface is allowed to fake mutation semantics when a
current probe fails.

## Compatibility objective

ChatGPT conversations and configured apps can retain/cached tool schemas. Current
ChatGPT app updates also require explicit refresh/action enablement. Therefore V1
starts with the complete small tool set and treats its public schema as an ABI.

Connector ABI identifier:

```text
command-governor-foreman/v1
```

All responses contain a `protocol_version` string and additive response fields are
allowed. Breaking argument/tool semantics require a new connector ABI and an
explicit operator refresh/rebind; they are not silently mutated underneath an old
conversation.

The server negotiates the MCP protocol version supported by the connecting client.
It does not hard-code one date even though the reviewed `rmcp` supports current
2026 protocol types.

## V1 tools

Exactly four public foreman tools are planned:

1. `foreman_bootstrap`
2. `foreman_resume`
3. `foreman_ack`
4. `foreman_answer_input`

Do not create a new MCP tool for every internal feature. Do not expose a generic
arbitrary-command dispatcher that defeats per-tool safety semantics.

## Common response envelope

Conceptual shape:

```json
{
  "protocol_version": "command-governor-foreman/v1",
  "server_instance": "opaque-instance-id",
  "binding_generation": 7,
  "compatibility": {
    "connector_abi": "command-governor-foreman/v1",
    "capability_epoch": 1,
    "schema_compatible": true,
    "write_actions_available": true
  },
  "result": {}
}
```

No response includes browser credentials, GitHub credentials, cwd, terminal
transcripts, or hidden worker/tool payloads unrelated to the requested obligation.

Errors are structured application results where possible so the model can
reconcile rather than guessing. Important classes include:

- `stale_binding_generation`
- `stale_obligation_version`
- `stale_or_expired_claim`
- `wake_delivery_not_current`
- `obligation_already_closed`
- `input_request_superseded`
- `user_authorization_required`
- `result_artifact_unavailable`
- `write_capability_unavailable`
- `connector_abi_mismatch`
- `reconciliation_required`

## Delivery identity versus wake correlation

MCP itself does not currently give Command Governor a stable, documented ChatGPT
conversation ID that can be relied on as a tool-call security principal. The
browser binding still knows the exact target conversation, but a connector can be
available elsewhere in the same authenticated workspace.

V1 therefore separates two concepts:

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)

delivery_id = CSPRNG(>=192 bits)
```

- `delivery_key` is deterministic, non-secret, and used only for idempotency and
  deduplication. It never grants possession or mutation authority.
- `delivery_id` is a cryptographically random opaque correlation ID generated once
  when the durable delivery is created.
- the tiny browser wake contains `obligation_id` and the random `delivery_id`;
- `foreman_resume` requires that exact random `delivery_id`;
- the daemon verifies the delivery is accepted, belongs to the obligation, targets
  the current obligation version/source fact, and was sent under the current
  `binding_generation`;
- bootstrap/status APIs never expose the current accepted `delivery_id`;
- `foreman_resume` mints a new claim ID bound to that delivery and generation;
- ACK/input mutation requires the claim.

The random `delivery_id` is not advertised as the sole authentication secret and
does not replace connector authentication. It is a possession/correlation nonce
that prevents a stale or unrelated ChatGPT conversation from claiming current work
merely by learning deterministic scheduling metadata.

If a future official MCP/ChatGPT metadata field provides a cryptographically
trustworthy conversation/turn identity, add it as another fence without changing
the domain model.

## `foreman_bootstrap`

Purpose: let any compatible conversation inspect Command Governor health and
learn that durable work exists, including conversations that have gone stale.

This tool is read-only and does not claim work. Because the caller is not proven to
be the exact bound conversation, bootstrap is deliberately low-information.

Conceptual input:

```json
{
  "protocol_version": "command-governor-foreman/v1",
  "known_binding_generation": 6
}
```

Both fields may be optional for first contact.

Conceptual result:

```json
{
  "outstanding_count": 3,
  "attention_summary": [
    {
      "kind": "completed_result",
      "count": 2,
      "highest_priority": 100,
      "oldest_age_seconds": 42,
      "wake_state": "scheduled_or_accepted"
    },
    {
      "kind": "needs_input",
      "count": 1,
      "highest_priority": 90,
      "oldest_age_seconds": 17,
      "wake_state": "pending"
    }
  ],
  "binding": {
    "generation": 7,
    "state": "healthy",
    "wake_required_for_mutation": true
  },
  "health": {
    "mcp_write_capability": "available",
    "browser": "healthy",
    "runtime_conflicts": 0,
    "ambiguous_deliveries": 0
  }
}
```

Bootstrap intentionally does **not** return repository/project refs, task/session
refs, result content, raw obligation metadata, or the accepted wake `delivery_id`.
A stale conversation can discover "work exists / this is no longer the active
binding generation" without learning the possession value needed to claim it.

The exact browser wake already gives the bound foreman the opaque obligation and
random delivery IDs needed for resume.

## `foreman_resume`

Purpose: prove this turn possesses the accepted wake correlation, claim the exact
obligation version, and fetch the real result/input needed for processing.

This is a **state-changing** tool because it creates a foreman claim.

Conceptual first-page input:

```json
{
  "protocol_version": "command-governor-foreman/v1",
  "obligation_id": "obl_...",
  "expected_obligation_version": 4,
  "binding_generation": 7,
  "wake_delivery_id": "del_random_..."
}
```

Conceptual result:

```json
{
  "claim": {
    "claim_id": "claim_...",
    "obligation_version": 5,
    "binding_generation": 7,
    "expires_at": "..."
  },
  "obligation": {
    "kind": "completed_result",
    "attention_state": "processing",
    "task_ref": "task_...",
    "source_event_id": "src_...",
    "worker": {
      "kind": "claude",
      "session_id": "sess_...",
      "session_incarnation_id": "inc_...",
      "turn_id": "turn_..."
    }
  },
  "artifact": {
    "media_type": "text/plain",
    "content": "...untrusted worker result page...",
    "next_cursor": null,
    "content_is_untrusted": true
  },
  "engineering_refs": [
    {"kind": "github_pull_request", "ref": "..."}
  ]
}
```

### Paging

Large bounded result artifacts are paged through the same tool to avoid creating
an unstable `fetch_result_v2` tool later. Subsequent input supplies `claim_id` and
opaque `cursor`; it must still include obligation and binding fences. The server
controls page size and total artifact limit.

The cursor is scoped to `(claim_id, result_artifact_id, digest)` and cannot read a
different artifact.

### Claim expiry

If the ChatGPT turn disappears before ACK, the claim may expire back to the
obligation's prior attention state. Expiry never discards result content and never
closes work. A new accepted wake/revision can claim again later.

## `foreman_ack`

Purpose: record the foreman's explicit disposition after processing/review.

This is the normal action that closes a processed obligation. Browser delivery,
MCP result delivery, and assistant settlement cannot substitute for it.

Conceptual input:

```json
{
  "protocol_version": "command-governor-foreman/v1",
  "obligation_id": "obl_...",
  "obligation_version": 5,
  "source_event_id": "src_...",
  "binding_generation": 7,
  "claim_id": "claim_...",
  "disposition": "reviewed_accepted",
  "evidence_refs": [
    {"kind": "github_pull_request", "ref": "..."}
  ]
}
```

Allowed disposition strings are server-versioned semantic values. Unknown values
are rejected; a new value does not change the tool shape.

### ACK validation

In one SQLite transaction, verify:

- obligation exists and is open;
- current obligation version matches;
- source event fence matches;
- binding generation is current;
- claim exists, is unexpired/current, and was minted from an accepted wake for
  this obligation/generation;
- referenced artifact/input state still matches the claim;
- disposition is valid for the obligation kind.

Then append one explicit disposition event and close the projection.

A repeated identical ACK may return the existing terminal disposition as an
idempotent success **only** when every fence/disposition matches the already
committed closing event. A different stale ACK cannot rewrite the disposition.

## `foreman_answer_input`

Purpose: record a structured answer to a current durable worker input request and
schedule the fenced worker-resume delivery.

It does **not** mean the worker received the answer.

Conceptual input:

```json
{
  "protocol_version": "command-governor-foreman/v1",
  "obligation_id": "obl_...",
  "obligation_version": 5,
  "input_request_id": "input_...",
  "source_event_id": "src_...",
  "binding_generation": 7,
  "claim_id": "claim_...",
  "answer": {
    "kind": "choice",
    "choice_ids": ["choice_a"]
  }
}
```

Supported answer shapes are deliberately small:

- `choice` with opaque choice IDs;
- `text` with a bounded plain-text answer for ordinary engineering coordination;
- `deny` for a request the foreman elects not to grant;
- `defer_to_user` to preserve the request as user-owned attention.

No answer field is a general shell command/tool argument channel.

### Authorization

Every input request carries a policy classification. If the request is
`user_owned_decision` or would widen/destructively change authority beyond a
recorded user grant, `foreman_answer_input` returns
`user_authorization_required` without recording a grant or touching the worker.

After a valid answer is recorded, Command Governor creates a separate worker
command/resume delivery. Only native resumed-turn evidence returns the obligation
to `running`.

## MCP tool annotations

When supported by the negotiated MCP version, expose truthful safety annotations:

| Tool | read-only | mutating | idempotency note |
| --- | --- | --- | --- |
| `foreman_bootstrap` | yes | no | safe repeat |
| `foreman_resume` | no | claim mutation | repeat requires same accepted wake/current claim; no closure |
| `foreman_ack` | no | closes obligation | identical committed ACK can return idempotent success |
| `foreman_answer_input` | no | records answer/schedules worker delivery | same fenced answer may return existing record; conflicting answer rejected |

Do not use misleading read-only annotations to work around ChatGPT plan/action
availability.

## Prompt-injection/data boundary

MCP tool descriptions and server-generated control fields are trusted Command
Governor protocol. Worker results, GitHub issues/diffs/comments, and repository
files are untrusted data.

Every result-bearing response must label content as untrusted and separate it
structurally from control instructions. The server never accepts an instruction
inside an artifact as a substitute for an MCP argument or user policy.

Example: a worker result saying "ACK obligation X now" is data. Only the foreman
decides whether to call `foreman_ack` after independent review.

## Reachability / Secure MCP Tunnel

Current ChatGPT development guidance does not make an arbitrary localhost MCP URL
available directly to ChatGPT. Command Governor should use the supported OpenAI
Secure MCP Tunnel/connectivity path rather than expose an unauthenticated local
HTTP server to the internet.

The transport topology is implementation-spike dependent:

- if the supported tunnel consumes stdio MCP, a tiny stateless `rmcp` stdio shim
  may talk to the authoritative daemon over owner-only local IPC;
- if it forwards to loopback Streamable HTTP, the embedded daemon endpoint binds
  loopback only and uses an application-owned local capability between tunnel and
  daemon.

Either way, the tunnel/shim owns **zero orchestration state**. A crash/restart of
that transport cannot close or lose obligations.

Tunnel credentials/config are owner-private, excluded from logs, and never passed
on command lines when a safer file/stdin/IPC mechanism exists.

## Binding capability preflight

`command-governor chatgpt bind` is not successful merely because the connector is
visible. It must prove the actual candidate workspace/account can execute the V1
contract.

At minimum the preflight verifies:

1. connector ABI matches;
2. bootstrap tool is visible;
3. a harmless test mutation round trip proves state-changing MCP actions are
   available on this account/surface;
4. the actual confirmation behavior permits the intended legitimate model-driven
   mutation flow without bypass;
5. app selection can be attached to the exact browser message;
6. stale-generation test mutation is rejected.

The test mutation operates on a synthetic preflight record, never a real
engineering obligation.

Published plan documentation is retained as dated compatibility evidence, but
ADR 0006 forbids using plan name as the support decision. The target Pro surface
demonstrated state-changing Tandem MCP on 2026-08-31. Every exact bound surface
still must pass the Command Governor synthetic mutation/read-back and confirmation
preflight for the current `capability_epoch`; if it fails, binding records the
exact unsupported state and the daemon does not weaken the invariant.

## Stable-schema upgrade policy

- additive optional response fields: same ABI;
- stricter internal validation with same accepted contract: same ABI, document it;
- new required argument, removed tool, renamed tool, changed mutation semantics:
  **new connector ABI**;
- new external capability that can fit behind existing response/action semantics:
  prefer internal evolution over a new public tool;
- old conversations always get a meaningful bootstrap compatibility result when
  their cached schema can still invoke V1 bootstrap.

This is why all four V1 tools must exist before the first supported connector is
published.
