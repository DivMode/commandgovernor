# Command Governor

Command Governor is a local-first durable control plane for reliable AI/software-engineering workers.

[commandgovernor.com](https://commandgovernor.com)

> **Status:** architecture and safety contracts. There is no executable
> implementation yet. The project is intentionally not scaffolding the Rust
> workspace until the reviewed V1 architecture is accepted.

## What it solves

AI workers can outlive the browser turn or process that started them. A worker
may finish after its foreman disconnects, block on input nobody notices, or be
duplicated because prompt delivery was ambiguous. A terminal runtime can also be
wrong: Claude may have stopped or need input while a process layer still says
`working`.

Command Governor makes those coordination debts durable.

The core invariant is:

> Delegated work does not disappear because ChatGPT, a browser, a terminal
> runtime, a worker, or Command Governor restarts. Worker completion creates a
> durable obligation. Only explicit foreman processing plus a fenced ACK closes
> normal processed work.

Browser delivery is not ACK. A completed ChatGPT assistant turn is not ACK.
Runtime idle is not ACK.

## V1 operating model

- **ChatGPT Web / ChatGPT foreman:** planning and independent review of record.
- **Claude Code:** primary implementation worker initially; Codex and other agents
  are additional workers later.
- **GitHub:** durable engineering source of truth.
- **Herdr or another runtime:** terminal/process/session layer.
- **Command Governor:** authoritative orchestration truth.

V1 is **Rust daemon + CLI**. There is no application GUI and no phone/email/Slack/
Telegram/ntfy completion-notification subsystem. The system wakes the bound
ChatGPT foreman itself.

## Intended V1 architecture

```text
command-governor daemon
  ├── event / obligation state machines
  ├── single-writer SQLite (`rusqlite`)
  ├── private result artifact store
  ├── Claude / Codex worker adapters
  ├── Herdr / runtime adapters
  ├── Rust MCP (`rmcp`) + supported tunnel path
  ├── dedicated headed Chrome profile + CDP
  ├── isolated `governor-chatgpt-web` adapter
  └── GitHub integration

command-governor CLI -> owner-local daemon IPC
```

The ChatGPT write path is browser-backed: the real authenticated ChatGPT SPA
performs sensitive submission. Rust CDP/Network evidence is used for stronger
observation/reconciliation. Command Governor does not reimplement Sentinel,
Turnstile, proof-of-work, CAPTCHA, entitlement checks, or rate-limit/anti-abuse
bypasses.

## Delivery safety

Browser wakes use deterministic identities and at-most-once ambiguity semantics:

```text
pending -> claimed -> accepted | failed | ambiguous
```

`claimed` is durable before browser I/O. A second durable ambiguity fence is
armed immediately before the exact Send action. If Command Governor restarts with
an orphaned attempt, it becomes `ambiguous` before browser recovery. Accepted or
ambiguous deliveries are never automatically resent.

A settled-but-unACKed foreman turn may eventually receive a bounded **new delivery
revision** for the same obligation; the old delivery is never replayed.

## Current architecture gates

Two live gates remain intentionally unresolved before end-to-end support can be
claimed:

1. **MCP mutation capability:** the exact target ChatGPT account/surface must prove
   state-changing `foreman_ack` / input actions are available. Current published
   ChatGPT plan capabilities mean Pro cannot be assumed to support this today.
2. **Authenticated browser spike:** headed Chrome + CDP must pass exact binding,
   per-message app selection, 10/10 unique wakes, strong accepted evidence,
   crash-at-Send ambiguity, no replay, restart, and rebind-generation tests.

The architecture does not weaken its ACK or duplicate-delivery rules if a platform
gate fails; that combination is marked unsupported until the capability exists.

## Architecture documentation

Start here:

- [V1 architecture](docs/architecture.md)
- [Technology research snapshot (2026-08-31)](docs/research/2026-08-31-technology-review.md)
- [Data model](docs/data-model.md)
- [State machines](docs/state-machines.md)
- [ChatGPT browser transport and live spike](docs/browser-transport.md)
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
- [0005 — native worker lifecycle + result durability](docs/adr/0005-worker-lifecycle-and-result-durability.md)

## Security and unofficial ChatGPT Web support

The ChatGPT Web adapter is unofficial and may break as the service changes.
Command Governor uses normal user authentication in a dedicated local browser,
does not claim OpenAI endorsement, and deliberately avoids challenge/auth/
entitlement/rate-limit circumvention.

See [SECURITY.md](SECURITY.md) and the [threat model](docs/threat-model.md),
including current Terms/compatibility risk.

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
