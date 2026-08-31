# Command Governor

Command Governor is a local-first control plane for reliable AI coding-agent
orchestration.

[commandgovernor.com](https://commandgovernor.com)

> **Status:** architecture and repository scaffolding. There is no executable
> implementation yet.

## Why Command Governor

AI workers can outlive the browser turn or process that started them. A worker
may finish after its foreman disconnects, block on input nobody notices, or be
duplicated because prompt delivery was ambiguous. Terminal state alone cannot
answer whether a task is running, finished, consumed, or still owed attention.

Command Governor is intended to make those obligations durable. It will keep
the authoritative lifecycle record locally, recover after crashes, and ensure
that completed worker results are not silently lost or orphaned.

## Operating model

- ChatGPT Web/Pro can act as the human-visible foreman and reviewer of record.
- Claude Code, Codex, and future coding agents can act as workers.
- GitHub remains the durable source of engineering truth for issues, commits,
  reviews, and pull requests.
- Session runtimes manage terminals and processes beneath the control plane.
- Command Governor owns durable orchestration state: tasks, sessions, events,
  results, obligations, delivery attempts, and recovery decisions.

These roles are adapters, not permanent vendor constraints. The architecture is
designed to support multiple conversational providers, coding agents, source
hosts, and session runtimes over time.

## Intended guarantees

- **Durable lifecycle truth.** Process labels such as `working` are observations,
  not the authoritative task state.
- **No lost worker results.** Completion and result-consumption obligations
  survive foreman disconnects and application restarts.
- **No blind duplicate dispatch.** Every dispatch has a durable identity and a
  recorded outcome or reconciliation requirement.
- **At-most-once browser delivery.** A delivery is bound to one verified ChatGPT
  conversation. An ambiguous submission is quarantined for reconciliation and
  is never retried automatically.
- **Crash recovery.** Durable state is replayed on restart; expired leases and
  incomplete obligations are reconciled before new work is issued.
- **Explicit human attention.** Blocked workers, ambiguous deliveries, and
  policy decisions become visible obligations rather than hidden terminal state.

At-most-once delivery deliberately prefers a visible unresolved obligation over
a duplicate browser submission. It does not claim exactly-once behavior from a
browser interface that offers no transactional idempotency primitive.

## Architecture

The initial design separates four concerns:

1. A durable local state store and append-only event history.
2. A lifecycle engine that derives task state and creates obligations.
3. Adapters for foremen, workers, source control, and session runtimes.
4. Recovery and reconciliation loops that resume safely after interruption.

See [Architecture](docs/architecture.md), the
[roadmap](docs/roadmap.md), and
[ADR 0001](docs/adr/0001-command-governor-architecture.md).

## Project principles

- Local-first by default; remote integrations are explicit.
- Durable records over inferred UI or process state.
- Idempotent commands where supported; reconciliation where they are not.
- Least privilege and narrow, auditable adapter boundaries.
- Human review remains explicit for consequential engineering actions.
- Provider-neutral domain models and lifecycle rules.

## Contributing

Command Governor is public and intended for broad use. The project is currently
design-first: proposals should establish lifecycle invariants and failure
behavior before implementation begins.

Read [CONTRIBUTING.md](CONTRIBUTING.md),
[SECURITY.md](SECURITY.md), and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before contributing.

## License

Command Governor is licensed under the [MIT License](LICENSE).
