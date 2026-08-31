# Command Governor

Command Governor is a local-first durable control plane for reliable AI/software-engineering workers.

[commandgovernor.com](https://commandgovernor.com)

> **Status:** reviewed architecture and safety contracts. There is no executable
> implementation yet. The next allowed step is a small pure-Rust kernel/store/
> testkit scaffold after the architecture PR is accepted; live service adapters
> remain gated.

## What it solves

AI workers can outlive the browser turn or process that started them. A worker may
finish after its foreman disconnects, block on input nobody notices, or be
duplicated because prompt delivery was ambiguous. A terminal runtime can also be
wrong: a worker can have a confirmed result/input boundary while the process layer
still says `working`.

Command Governor makes those coordination debts durable.

> Delegated work does not disappear because ChatGPT, a browser, a terminal
> runtime, a worker, or Command Governor restarts. Worker completion creates a
> durable obligation. Only explicit foreman processing plus a fenced ACK closes
> normal processed work.

Browser delivery is not ACK. A settled ChatGPT assistant turn is not ACK. Runtime
idle is not ACK. A Claude `Stop` hook callback alone is not completion either.

## V1 operating model

- **ChatGPT Web foreman:** planning and independent review of record on a
  write-capable supported workspace surface.
- **Claude Code:** primary implementation worker initially; Codex/others later.
- **GitHub:** durable engineering source of truth.
- **Herdr or another runtime:** process/session transport.
- **Command Governor:** authoritative orchestration truth.

V1 is **Rust daemon + CLI**. There is no application GUI and no phone/email/Slack/
Telegram/ntfy completion-notification subsystem. The system wakes the bound
ChatGPT foreman itself.

### Current ChatGPT plan constraint

As of the 2026-08-31 architecture review, OpenAI documents full custom MCP
modify/write support for **ChatGPT Business, Enterprise, and Edu** beta surfaces;
consumer **ChatGPT Pro custom MCP is read/fetch-only**. Because Command Governor's
`foreman_resume`, `foreman_ack`, and `foreman_answer_input` are real mutations,
consumer Pro is not currently an end-to-end V1 foreman target.

Business currently offers the **Pro model option powered by GPT-5.6 Sol Pro**, so
the intended high-capability Pro-model foreman can still be tested on a
write-capable workspace surface without weakening the ACK invariant.

## Intended V1 architecture

```text
command-governor daemon
  ├── event / obligation state machines
  ├── single-writer SQLite (`rusqlite`)
  ├── private immutable result artifact store
  ├── Claude / Codex worker adapters
  ├── Herdr / runtime adapters
  ├── Rust MCP (`rmcp`) + supported tunnel path
  ├── dedicated headed Chrome profile + CDP
  ├── isolated `governor-chatgpt-web` adapter
  └── GitHub integration

command-governor CLI -> owner-local daemon IPC

Herdr/session runtime
  -> command-governor worker-host claude <opaque-turn-id>
       ├── parses `claude -p` structured output online
       ├── persists sanitized run/exit receipts only
       └── persists one bounded complete final-result candidate
```

The worker-host is a transport shim, **not another orchestration daemon**. It owns
no obligations or review state; it exists so a Claude final structured result can
survive the authoritative daemon restarting. It does **not** persist a raw copy of
the complete provider stream: intermediate tool-use/tool-result records are
processed in memory and discarded.

For managed Claude V1, a Stop-hook callback is only `stop_candidate` evidence:
current Claude hooks can block stopping and matching hooks can run in parallel.
Successful completion is proven by the final structured programmatic result plus
the matching child-process completion, then the bounded result artifact is made
durable before `completed_unprocessed` is published.

Current Claude documentation also says `PermissionRequest` can run in
non-interactive contexts. Exact durable out-of-band pause/resume prefers a
confirmed **single-tool** `PreToolUse` defer, because current multi-tool defer is
ignored and `PermissionRequest` lacks the same exact `tool_use_id` fence.

## ChatGPT transport

The ChatGPT write path is browser-backed: the real authenticated ChatGPT SPA
performs sensitive submission. Rust CDP/Network evidence provides stronger
observation/reconciliation. Command Governor does not reimplement Sentinel,
Turnstile, proof-of-work, CAPTCHA, entitlement checks, or rate-limit/anti-abuse
bypasses.

Every wake targets one exact `/c/<id>` binding generation and one exact obligation
version/source fact.

Browser wake identity deliberately separates dedupe from possession correlation:

```text
delivery_key = deterministic hash(obligation, binding generation, revision)
delivery_id  = random CSPRNG value (>=192 bits)
```

The deterministic key prevents duplicate scheduling. The random delivery ID is
placed in the exact bound browser wake, omitted from bootstrap/status, and required
by `foreman_resume` in addition to connector authentication and all durable fences.

Browser delivery uses at-most-once ambiguity semantics:

```text
pending -> claimed -> accepted | failed | ambiguous
```

`claimed` is durable before any browser I/O. A second durable ambiguity fence is
armed immediately before exact Send. On restart, an orphaned attempt is
quarantined as ambiguous before browser recovery. Accepted or ambiguous deliveries
are never automatically resent.

A settled-but-unACKed foreman turn may later receive a bounded **new delivery
revision** for the same obligation; the old delivery is never replayed.

## Stable foreman MCP

The proposed V1 connector ABI uses the official Rust MCP SDK and exposes only:

- `foreman_bootstrap`
- `foreman_resume`
- `foreman_ack`
- `foreman_answer_input`

Bootstrap is intentionally low-information because MCP does not currently provide
a documented trustworthy ChatGPT conversation principal. It reports aggregate
health/attention only and never leaks the accepted random wake correlation ID.

`foreman_resume`, ACK, and input answer are truthful mutations. ACK is the normal
closure operation; browser/assistant state cannot substitute for it.

## Current architecture gates

Three live gates remain deliberately unresolved before end-to-end support can be
claimed:

1. **Gate A — ChatGPT MCP mutation capability.** A candidate Business/
   Enterprise/Edu workspace must prove state-changing Command Governor actions and
   confirmation behavior on the actual account/surface. Consumer Pro is currently
   excluded by published product capability.
2. **Gate B — authenticated headed Chrome/CDP.** Exact binding, per-message app
   selection, 10/10 unique wakes, strong accepted evidence, crash-at-Send
   ambiguity/no replay, restart, random-correlation fencing, and generation
   fencing must pass. Headless is a separate experiment.
3. **Gate C — Claude managed execution.** A pinned real Claude invocation must
   prove structured final-result/exit semantics, no raw stream persistence,
   actual settings/hook-source behavior, controlled parallel Stop-hook veto
   without false completion, single-tool defer/resume, multi-tool defer failure,
   non-interactive `PermissionRequest`, daemon-offline final-result recovery,
   stale-Herdr reconciliation, and forbidden-data non-persistence.

If a platform gate fails, the adapter/surface is marked unsupported. The durable
obligation, at-most-once, and explicit-ACK invariants are not weakened.

## Local security boundary

The local OS user is the V1 administrative trust root. Owner-only file permissions
protect against other OS principals and accidental exposure, but they do **not**
sandbox a deliberately malicious Claude/tool process already running as the same
user. Command Governor minimizes worker-visible state paths and validates all
imported staging data; strong hostile-worker containment is a future separate-user
or sandbox/broker feature, not a V1 claim.

## Architecture documentation

Start here:

- [V1 architecture](docs/architecture.md)
- [Independent architecture review](docs/reviews/2026-08-31-architecture-review.md)
- [Technology research snapshot (2026-08-31)](docs/research/2026-08-31-technology-review.md)
- [Data model](docs/data-model.md)
- [State machines](docs/state-machines.md)
- [ChatGPT browser transport/live spike](docs/browser-transport.md)
- [MCP contract](docs/mcp-contract.md)
- [Worker lifecycle/input/watchdog](docs/worker-lifecycle.md)
- [Threat model](docs/threat-model.md)
- [Acceptance tests](docs/testing.md)
- [Roadmap](docs/roadmap.md)

ADRs:

- [0001 — durable orchestration control plane](docs/adr/0001-command-governor-architecture.md)
- [0002 — Rust daemon + `rusqlite`](docs/adr/0002-rust-daemon-and-sqlite.md)
- [0003 — ChatGPT browser-backed hybrid](docs/adr/0003-chatgpt-browser-hybrid.md)
- [0004 — foreman MCP + exact binding/ACK](docs/adr/0004-foreman-mcp-and-binding.md)
- [0005 — structured Claude lifecycle + result durability](docs/adr/0005-worker-lifecycle-and-result-durability.md)

## Security and unofficial ChatGPT Web support

The ChatGPT Web adapter is unofficial and may break as the service changes.
Command Governor uses normal user authentication in a dedicated local browser,
does not claim OpenAI endorsement, and deliberately avoids challenge/auth/
entitlement/rate-limit circumvention.

See [SECURITY.md](SECURITY.md) and the [threat model](docs/threat-model.md).

## Open-source provenance

Command Governor is an independent implementation. Architecture research includes:

- Tandem — MIT
- codex-chatgpt-web — MIT
- CCCC — Apache-2.0

Exact reviewed revisions and attribution policy are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). No third-party implementation
source is currently vendored/copied into this repository.

## Contributing

The project is design-first and test-first: proposals should state lifecycle
invariants, external-I/O ambiguity behavior, security/data boundaries, and the
failure-injection tests that prove the change.

Read [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

Command Governor is licensed under the [MIT License](LICENSE).
