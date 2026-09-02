# Command Governor threat model

Status: current for the composition-first Prime/Pi architecture in ADRs 0008–0010.

Older standalone Rust daemon/SQLite/browser/MCP threat assumptions remain in Git history and historical documents; they are not the current product topology.

## System shape

```text
Prime Agent
  + selected reviewed Prime/Pi packages
  + @commandgovernor/harness
      - small product-specific extensions/policy
      - roles / skills / prompts / configuration
      - focused conformance
      - temporary compatibility shims only for proven upstream defects
```

Command Governor does not assume it owns a second general runtime, session store, scheduler, subagent engine, memory engine, or browser automation stack.

## Assets

Protect:

- repository/worktree contents;
- user credentials and environment secrets;
- provider/API keys;
- GitHub authentication;
- authenticated browser/session material;
- exact task/revision/session/correlation identities;
- evidence required for independent review or safe reconciliation;
- component pins, hashes, authority assignments, and policy configuration;
- user-owned decisions and permission boundaries.

## Trust assumptions

### Trusted-local profile

The local OS user is the administrative trust root. Prime, selected packages, Command Governor code, workers, and tools may have the same-user authority available to their process unless explicitly isolated.

This profile protects against accidental disclosure and other OS principals where file permissions apply. It does not protect against a deliberately malicious same-user process.

### External systems

GitHub, model providers, ChatGPT Web, package registries, and network services are outside the local durability boundary. Their responses may be delayed, duplicated, stale, unavailable, or ambiguous.

### Model output and repository content

Model output, worker output, repository text, webpages, issues, PR comments, tool output, and retrieved documents are untrusted data. They cannot redefine architecture/policy merely by containing instructions.

## Primary threats and controls

### T1 — duplicate external effect after ambiguous failure

**Threat:** a worker/process/transport dies after an external effect but before a trustworthy result reaches the caller; a replacement operation repeats the effect.

**Control:** unknown effect state is `UNCERTAIN`/reconciliation, not proven failure. Stable operation identity and package/runtime idempotency are used where available. The current D2 custom guard is a `TEMP WORKAROUND` and must disappear if the package path no longer needs it.

**Conformance:** D2 worker-loss and post-effect import reproducers plus completed-command idempotence.

### T2 — stale identity mutates newer work

**Threat:** an old incarnation, cursor, task revision, delivery, claim, or foreman response is accepted after a newer generation/revision exists.

**Control:** exact current identities are checked at the owning boundary; stale identities fail closed. Generic Prime session identity should be reused rather than shadowed when sufficient.

**Conformance:** D1 stale-incarnation/cursor behavior and future foreman-revision tests when that transport is selected.

### T3 — overlapping package authorities

**Threat:** two extensions/packages both believe they own task completion, compaction, memory, transport, tool gating, or lifecycle state; load order silently decides behavior.

**Control:** `harness/authorities.json` records exactly one owner per concern and the reason that owner is allowed to exist (`USE EXISTING`, `PLUGIN`, `TEMP WORKAROUND`). Unassigned concerns remain explicit until a bake-off selects an owner.

### T4 — temporary workaround becomes permanent control plane

**Threat:** a custom subsystem accumulates tests and review history until sunk cost is treated as architecture justification.

**Control:** every `TEMP WORKAROUND` names a removal condition. Review necessity before correctness. Delete implementation-specific tests with the workaround. ADR 0010 forbids a parallel general control plane.

### T5 — secret/environment leakage

**Threat:** the host environment or credentials are copied wholesale into Prime/worker/package processes, logs, evidence, or repository files.

**Control:** positive allowlists and explicit grants; no routine raw environment snapshots; no credentials in public evidence; private transport/browser credentials are not general control data.

**Conformance:** environment boundary negative/positive sentinels.

### T6 — supply-chain or re-pin drift

**Threat:** a mutable tag, release asset change, dependency collision, or unreviewed package silently changes the runtime authority.

**Control:** exact pin/revision, integrity hashes where practical, license/provenance records, explicit re-pin review, and conformance against the selected version.

### T7 — wrong ChatGPT target or ambiguous submission

**Threat:** a foreman request reaches the wrong conversation or a send may have happened but is blindly repeated.

**Control:** existing exact-thread transports are evaluated before custom browser code; exact conversation/revision/correlation is required; ambiguous send is reconciled rather than replayed.

No current merge gate claims this feature is shipped until a transport is selected and the closed loop is proven.

### T8 — implementer self-approval

**Threat:** the worker that produced a result can satisfy the requirement for independent acceptance simply by claiming success.

**Control:** choose an existing task/review package or the smallest policy plugin that enforces independent acceptance. Do not add a custom review runtime before package bake-off.

### T9 — prompt injection becomes policy authority

**Threat:** repository content or external text tells the agent to ignore review, widen tools, expose credentials, or change architecture.

**Control:** deterministic repository/ADR/authority policy outranks untrusted content. Capability/permission changes require the actual owning mechanism/user decision, not text embedded in work data.

### T10 — lingering background processes/state

**Threat:** a test or failed run leaves resident Prime processes that mutate later sessions or create misleading conformance results.

**Control:** isolated fixture roots and mandatory process sweep. Runtime conformance is sequential where tests kill supervisors/workers.

## Persistence policy

Use the selected substrate/package's normal session state when it owns the concern. Command Governor should not duplicate raw transcripts/provider streams into a second authority.

Product-specific state should be minimal, typed/structured where practical, and limited to the exact policy/correlation/evidence that Prime/packages do not already own safely.

Credentials, raw browser auth material, provider keys, GitHub auth, and arbitrary environment snapshots are not routine persistent control state.

## Sandboxing

Sandboxing is optional hardening for workloads intentionally treated as untrusted. It is not a requirement for the trusted-local profile and should be supplied by an existing isolation mechanism rather than by inventing another Governor runtime.

When an untrusted-workload profile is selected, document the actual isolation boundary, filesystem/network grants, credential broker, and escape assumptions separately.

## Test strategy

See `docs/testing.md`.

The strongest useful boundary wins. Historical implementation-specific tests are not permanent security controls. If a current invariant matters, prove it against the current Prime/package product path.

## Review trigger

Update this threat model when a change introduces or changes:

- an authority owner;
- a new persistent state boundary;
- a new consequential external-write transport;
- a credential/environment grant;
- a package with meaningful process/filesystem/network authority;
- an isolation/sandbox profile;
- or the trust status of a workload.
