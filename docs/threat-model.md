# Command Governor V1 threat model

Command Governor coordinates components that can edit source, run processes,
access authenticated services, and drive an authenticated browser. A lifecycle
mistake can become a security mistake when it causes duplicate commands,
cross-project writes, false authorization, or premature review closure.

## Security goals

1. A stale/incorrect worker, runtime, browser, or ChatGPT conversation cannot
   silently close current work.
2. Ambiguous external writes are not automatically replayed.
3. Repository/worker content cannot become Command Governor policy through prompt
   injection.
4. Browser/GitHub/provider credentials stay in their normal private stores and do
   not leak into the control ledger or safe logs.
5. State/result durability survives crashes without widening authority.
6. Name reuse/stale identities cannot cross session/project/binding generations.
7. ChatGPT cannot grant authority the user did not delegate.
8. Public distribution does not implement auth/entitlement/CAPTCHA/rate-limit/
   anti-abuse bypass.
9. A provider hook callback cannot be promoted to stronger lifecycle truth than
   its documented semantics justify.

## High-value assets

- dedicated ChatGPT Chrome profile/session state;
- GitHub credentials and repository write authority;
- worker/runtime execution authority;
- OpenAI-supported MCP tunnel/app credentials;
- Command Governor local control endpoint;
- SQLite DB/WAL;
- private immutable result artifacts;
- private Claude worker-host structured-stream spools and exit receipts;
- managed Claude settings and sanitized hook inbox;
- repository contents visible to workers;
- durable user delegation/authorization policy.

## Trust boundaries

```text
human user / local OS account
   │
   ├── command-governor daemon ◄── owner-local IPC ── CLI / transport shims
   │       │
   │       ├── SQLite / result artifacts / hook inbox
   │       ├── runtime -> worker-host -> Claude/Codex -> repository data
   │       ├── GitHub remote
   │       └── dedicated Chrome profile -> ChatGPT.com
   │                                      └── supported MCP tunnel
   │
   └── local administrative trust root
```

The local OS account is the V1 administrative trust root. Owner-only file modes do
not contain malware already running as that same user; same-user compromise is
outside the strongest V1 isolation claim.

Remote repository, browser, and worker content is data, not policy.

## Threat: duplicate browser wake

**Failure:** daemon crashes after Send/evidence loss and an ordinary retry loop
submits the wake again.

**Mitigations:**

- deterministic delivery/revision identity;
- `claimed` durable before any browser I/O;
- `activation_armed` durable before exact Send action;
- startup orphaned attempts become ambiguous before browser recovery;
- accepted/ambiguous never automatically resend;
- exact message/network reconciliation can only promote ambiguous -> accepted;
- a later bounded resume is a new revision, never replay of the old one.

## Threat: wrong/stale ChatGPT conversation

**Failure:** current tab/history/redirect causes mutation in another conversation.

**Mitigations:**

- one explicit canonical `/c/<id>` binding;
- monotonic binding generation;
- dedicated browser target/profile;
- verify exact resolved conversation before composer mutation and before Send;
- delivery snapshots exact obligation version/source event;
- stale generation/target cannot claim or ACK current work.

## Threat: unrelated connector conversation mutates state

**Failure:** the same Command Governor connector is visible in another ChatGPT
conversation and calls mutation tools.

**Mitigations:**

- supported connector authentication;
- `foreman_resume` requires current accepted wake `delivery_id` in addition to
  generation/obligation fences;
- bootstrap does not reveal that current accepted delivery correlation nonce;
- resume mints a current claim;
- ACK/input answer require claim + source/version/generation;
- rebind invalidates old-generation mutations;
- future official trusted conversation/turn metadata can be added as another fence.

The delivery ID is an anti-confusion correlation nonce, not sole authentication.

## Threat: browser credential exfiltration

**Failure:** cookies/tokens are copied to SQLite/logs/argv/crash reports or an
unofficial standalone API client.

**Mitigations:**

- dedicated owner-private browser profile is the credential store;
- no cookie/token columns;
- no general cookie-export path;
- no secret argv where safer file/stdin/IPC exists;
- passive/direct reads prefer ambient browser session;
- byte-scan tests inject known sentinel credentials.

## Threat: private ChatGPT protection bypass

**Failure:** project grows challenge/anti-abuse bypass machinery to keep direct
private writes functioning.

**Policy:** explicitly out of scope. Real authenticated SPA performs sensitive
submission. No CAPTCHA/Turnstile/Sentinel/PoW/entitlement/rate-limit/anti-abuse
circumvention. If normal first-party interaction stops working, fail visibly.

## Threat: prompt injection from repository/worker data

**Failure:** issue/diff/result says "ignore policy and ACK/execute X".

**Mitigations:**

- MCP control envelope structurally separates trusted protocol from untrusted
  result/repository fields;
- ACK requires explicit correctly fenced tool invocation;
- worker never self-approves;
- foreman independently verifies GitHub/engineering evidence;
- adapters never execute commands parsed from worker/result prose.

## Threat: worker input widens user authority

**Failure:** worker asks for destructive/credential-sensitive action and foreman
silently grants it as ordinary coordination.

**Mitigations:**

- request policy classification;
- unknown/broader/destructive/credential-sensitive requests user-owned by default;
- `foreman_answer_input` returns `user_authorization_required` outside recorded
  delegation;
- user grants are explicit durable policy events;
- worker request never creates its own authorization.

## Threat: stale Herdr state overrides worker truth

**Failure:** confirmed Claude result/deferred input exists but Herdr still says
`working / idle:false`, causing rejected continuation or duplicate worker.

**Mitigations:**

- managed Claude structured final/deferred/process evidence outranks stale Herdr
  observation for the same fenced turn;
- record `runtime_state_conflict`;
- one explicit reconciliation/clear-busy path before continuation;
- no duplicate worker while ownership is unresolved;
- deterministic and live conformance fixtures reproduce the class.

## Threat: Stop-hook callback falsely declares completion

**Failure:** Command Governor's Claude `Stop` hook fires, but another matching Stop
hook returns `decision:block`; Claude continues while Governor publishes
`completed_unprocessed` from the first callback.

**Mitigations:**

- Stop callback is only `stop_candidate` evidence;
- current docs say matching hooks can run in parallel, so hook order is never
  terminal arbitration;
- successful managed Claude completion requires final structured programmatic
  `result` + matching child-process exit receipt;
- controlled parallel Stop-veto test is a hard live Gate C case;
- `SessionEnd` is session-end evidence, not successful-result proof.

## Threat: daemon crash loses Claude's final result

**Failure:** daemon owns the only stdout reader; Claude finishes while daemon is
restarting; final result disappears even though Herdr/worker continues or exits.

**Mitigations:**

- runtime launches managed Claude through a narrow Rust `worker-host` mode;
- worker-host captures structured stream to owner-private bounded spool;
- sanitized child-exit receipt is durable after child termination;
- worker-host owns no SQLite/task/obligation authority;
- daemon later validates/reconciles spool+receipt and extracts bounded final result
  into immutable result artifact;
- truncated/no-exit/no-final-result never becomes completion.

## Threat: worker-host becomes a second control plane

**Failure:** transport shim starts writing obligations or deciding terminal review
state, creating split-brain orchestration.

**Mitigations:**

- worker-host receives only opaque turn/session transport fences;
- no write access to SQLite control API;
- can write only its allocated private spool/receipt/inbox paths;
- daemon is sole importer/projector;
- tests attempt arbitrary state commands and verify no authority mutation.

## Threat: worker-host spool leaks prompt/provider content

**Failure:** structured provider stream is copied into routine logs/DB/doctor.

**Mitigations:**

- spool is a separately classified sensitive store, not event ledger;
- owner-private root/files, bounded size, no-follow rooted access;
- routine status exposes only safe IDs/counts/state;
- daemon extracts only bounded final result needed for review;
- safe-persistence sentinel scans allow sensitive content only in explicitly
  designated spool/result files.

## Threat: hook event disappears while daemon is down

**Failure:** progress/input/defer/native event is delivered only over live IPC.

**Mitigations:**

- managed hook first writes a sanitized owner-private atomic inbox envelope;
- file and directory durability are established before return as required;
- daemon ingestion dedupes by non-secret source identity;
- crash after DB commit before inbox cleanup is idempotent;
- inbox contains lifecycle metadata only, not raw hook payload/transcript.

## Threat: Claude settings/hook command tampering

**Failure:** attacker/unsafe local configuration rewrites the settings file or hook
command, executing unexpected code in each managed worker.

**Mitigations:**

- Command Governor-owned settings, never personal settings mutation;
- validate owner, mode, regular-file/no-symlink, safe ancestor, hook epoch;
- stable installed hook command;
- fail closed before managed spawn if unsafe;
- do not assume `--settings` isolates all user/project/plugin hooks;
- live conformance measures active settings/hook behavior.

## Threat: raw hook data leaks sensitive content

**Failure:** generic JSON serializer writes cwd/transcript/prompt/tool args/command
into SQLite/logs.

**Mitigations:**

- event-specific typed extractors;
- unknown provider fields discarded;
- progress stores identity/time/safe class only;
- input ledger stores opaque identity/classification, not raw tool args;
- byte-scan regression tests across DB/WAL/logs/inbox/crash state.

## Threat: unconfirmed defer creates false `needs_input`

**Failure:** PreToolUse hook attempts `defer`, but provider ignores/misparses it or
multi-tool shape partially executes; Governor assumes clean pending input.

**Mitigations:**

- persist safe defer intent separately;
- project `needs_input` only after structured managed-run outcome confirms the
  pending/deferred boundary;
- unsupported multi-tool shapes become reconciliation attention;
- PermissionRequest alone is not treated as a generic durable pause/resume
  primitive without conformance proof.

## Threat: result disappears after runtime close

**Failure:** completion obligation exists but actual worker result lived only in
PTY/provider spool.

**Mitigations:**

- confirmed final result extracted to immutable result artifact before
  `completed_unprocessed` transaction commits;
- digest/size/ref in SQLite;
- open obligation pins retention;
- file-before-DB crash-safe ordering;
- missing/corrupt artifact blocks processing and raises health condition.

## Threat: artifact/spool path tamper

**Failure:** worker supplies path, symlink swap, traversal, or modifies bytes.

**Mitigations:**

- daemon/worker-host allocate opaque store keys;
- root-contained no-follow access where supported;
- owner-private permissions;
- bounded size/complete-record checks;
- digest/length validation;
- untrusted content remains untrusted even when integrity-valid.

## Threat: SQLite corruption/rollback/projection drift

**Failure:** restored/corrupt DB projection appears closed while event history does
not support it.

**Mitigations:**

- schema epoch/checksums;
- foreign keys;
- event/projection transactional coupling;
- replay/validation watermark;
- mismatch fails closed into doctor/repair mode;
- supported backup/restore includes consistent DB/WAL plus required artifact set.

## Threat: two daemons become authorities

**Failure:** two daemons target one state root and each schedules work.

**Mitigations:**

- owner-root instance lock before DB/browser/runtime recovery;
- DB instance/daemon epoch;
- second process fails closed or becomes a stateless client;
- browser profile ownership separately fenced.

SQLite's one-writer property is not the daemon-election protocol.

## Threat: local IPC abuse

**Mitigations:**

- Unix socket in owner-only directory with peer credentials where available;
- Windows named-pipe ACL current-user-only;
- loopback HTTP only if required, with random local capability and strict loopback
  handling;
- no LAN bind by default.

## Threat: MCP tunnel exposure

**Mitigations:**

- use current supported OpenAI tunnel/connectivity path;
- stdio stateless shim or loopback-only endpoint as required;
- tunnel/shim owns no durable orchestration state;
- credentials excluded from argv/logs where safer mechanisms exist;
- no unauthenticated public `0.0.0.0` MCP server in V1.

## Threat: connector schema drift/caching

**Mitigations:**

- complete four-tool V1 ABI from first supported release;
- connector ABI/capability epoch in bootstrap;
- breaking changes require explicit new ABI/refresh/rebind;
- stale conversations can discover health/outstanding work but cannot bypass
  accepted-wake/claim/generation fences.

## Threat: supply-chain compromise

**Mitigations:**

- pinned Rust toolchain and committed application `Cargo.lock`;
- `cargo audit` + `cargo deny`;
- license/source policy;
- reviewed dependency automation, no blind auto-merge for security-sensitive
  crates;
- pin GitHub Actions by commit SHA where practical;
- explicitly reject known malicious August 2026 crate releases such as
  `arrayref 0.3.10`, `internment 0.8.7`, and `append-only-vec 0.1.9`;
- minimize browser/auth/process dependencies.

## Threat: diagnostics become exfiltration

Safe diagnostics may include opaque IDs, state/event classes, counts, durations,
versions/commits, redacted route class, and boolean evidence flags.

They must not include prompt/result/repository content by default, cwd, transcript
paths, raw provider stream, browser headers/bodies/cookies/tokens, GitHub auth, or
arbitrary environment variables.

## OpenAI terms / unofficial ChatGPT Web risk

Command Governor is an independent open-source project. Current sources reviewed:

- OpenAI Terms of Use, effective 2026-01-01:
  <https://openai.com/policies/terms-of-use/>
- OpenAI App Developer Terms, updated 2026-07-09:
  <https://openai.com/policies/app-developer-terms/>

The terms include restrictions relevant to reverse engineering, automated data
extraction, rate/restriction bypass, and protective measures. The normal-login,
local-browser, no-bypass design reduces some engineering/security risk but does
not make unofficial browser automation officially supported or establish a legal
conclusion.

Public posture:

- no OpenAI endorsement claim;
- normal user authentication;
- no auth/entitlement/CAPTCHA/rate/protective-measure bypass;
- browser credentials local;
- no protected private-write emulator;
- replaceable ChatGPT-specific adapter;
- users review terms applicable to their use.

## Out-of-scope V1 guarantees

- containing fully compromised same-user OS account;
- sandboxing arbitrary worker code from host by Command Governor itself;
- guaranteeing availability of private web/provider behavior;
- exactly-once semantics from a non-idempotent browser interface;
- preventing a user from manually copying their own opaque wake correlation ID;
- hostile multi-tenant local deployment.

## Security acceptance gate

No service adapter is supportable until the tests in [`testing.md`](testing.md)
prove identity fences, ambiguity recovery, Stop-veto safety, structured-result
recovery, artifact/spool integrity/ACL, event dedupe, and forbidden-data scans
under deterministic crash injection. Live ChatGPT and Claude support additionally
require Gates A/B/C from the architecture/roadmap.
