# Does Command Governor need any custom production code? — proof and cleanup, 2026-09-04

Status: **executed.** Every claim below was measured on this machine on
2026-09-04 in disposable roots against the exact revisions named, with
credential-free scripted models. Raw logs, scripts and per-assertion results
are preserved with the session that produced them and summarised here; the
repository keeps the reproducers that became conformance tests.

## The answer

**No.** Command Governor requires zero custom production code.

The product is:

```text
Prime Agent v0.9.1 (81ae3cb34d27d38ee37f9e205a1e73694993b344), verified assets
  + pi-tasks 0.2.5             durable task/evidence contract
  + @gotgenes/pi-subagents 21.4.0   delegation runtime and role files
  + pi-pr-review 1.17.10       GitHub review lane, reviewed-head binding
  + @commandgovernor/harness   skills, prompts, role files, project settings
  + pins/                      manifest, checksums, install root, bootstrap
  + conformance/               black-box tests through stock Prime clients
```

Everything that executes is Prime or a package Prime loads. The repository
ships two shell scripts (bootstrap and the test runner) and test code.

Every custom component that existed on `main` at `902814c` ends in exactly
one disposition:

| Component (before) | LOC | Disposition | Replaced by |
| --- | ---: | --- | --- |
| `governor/governor.ts` (external daemon client, reopen loop) | 674 | **DELETE** | stock `prime-agent -r <sessionFile>`; Prime's per-path lease and `openingWorkers` convergence |
| `governor/session/registry.ts`, `paths.ts` (D1/D8 registry, incarnation fence, path policy) | 691 | **DELETE** | Prime `sessionId`/`activeSessionId`, generation-fenced cursors, canonical session paths on every stock surface |
| `governor/mutation/{ledger,classify,proof,digest}.ts` (D2 ledger and classifier) | 1,248 | **DELETE** | Prime worker recovery marker (`prime-agent.worker_recovery`, "not replayed"); no stock client re-issues a mutation |
| `governor/fs/*`, `governor/process/*` (durable FS, process identity) | 668 | **DELETE** | existed only to support the stores above |
| `governor/prime/*` (protocol slice, daemon client, env allowlist, substrate reader, client identity) | 1,140 | **DELETE** | Prime's own clients; pin facts asserted from the manifest by the conformance suite |
| `harness/extensions/cg-foreman/transport.ts` (types-only transport stub) | 277 | **DELETE** | correlation rules are the `cg-foreman` skill over the pinned `pi-gpt` transport (§6) |
| `harness/authorities.json`, `harness/agents/role.schema.json` | 189 | **DELETE** (merged) | `pins/pins.json` `concerns[]`; roles in `@gotgenes/pi-subagents` agent-file format |
| `crates/*` (frozen Rust oracle) | 49,142 | **DELETE** | `docs/research/2026-09-01-rust-invariant-catalog.md` and Git history (PR #24) |
| `harness/agents/*.md`, `prompts/`, `skills/` | 495 (prose) | **USE EXISTING** format | Agent Skills, Prime prompt templates, pi-subagents agent files |
| `pins/`, `scripts/bootstrap.sh` | 184 (shell) | distribution metadata | unchanged in kind; re-pinned to 0.9.1 |
| `conformance/` | 5,534 (29 files) | rewritten | black-box suite through stock clients (see §12) |

No `PLUGIN` and no `TEMP WORKAROUND` survived. The one place where a plugin
was proposed during the proof (a local acceptance record, §5) was rejected
because the property it claimed to enforce is not enforceable by any local
code on this substrate, and the one place where custom code would be needed
(tool gating, §7) cannot be written by an extension at all.

## 1. Method

Each product requirement was run through the path a user actually runs —
the stock `prime-agent` clients (interactive TUI on a pty, `-r`, `list --json`,
`send`, `schedule`, `-p`, `--mode json`, `--mode rpc`) against an isolated
supervisor — never through the deleted raw daemon client. Packages were
installed with `prime-agent package install --local` into scratch projects
and observed to register through Prime's own loader and the mock model's
`tools` array, because Prime discards extension load failures silently in
headless modes (§8, item B). Every safety assertion sits next to a negative
control that shows the measurement can fail.

Inputs, all read at their current revisions on 2026-09-04:

- Prime Agent `v0.9.1` (installed, protocol 7, schema revision 25) and
  `v0.8.1` (the previous pin) for the D2 confirmation; Prime `main`
  `5c2750bd` for the "still unfixed" checks.
- Upstream Pi `v0.85.0`.
- The nine packages named by ADR 0010 §17 plus the alternatives found by a
  catalogue sweep (pi.dev, npm `pi-package` keyword, GitHub).
- DeepSeek Harness `master` `d347e703` (0.1.3-alpha.1) against the alpha.5
  baseline in ADR 0010.

## 2. D1 — resident-root recovery (stock path): NO custom code

The bake-off finding reproduces on the product path: after `SIGKILL` of a
resident worker, `prime-agent list` reports `failed` and then drops the row;
the supervisor never relaunches a resident root on its own
(`daemon-supervisor.js`: no `transientCreateCommand` → `lifecycle = "failed"`,
`lastError = "Waiting for a client with fresh runtime context"`).

What a user does next decides everything, and the stock client handles it:

| Assertion | Observed on 0.9.1 |
| --- | --- |
| `prime-agent -r <sessionFile>` reopens the dead root | same `sessionId`, same `sessionFile`, new `activeSessionId`, exactly one live row |
| transcript integrity | pre-crash turn present, file only grew (1,642 → 2,445 bytes), exactly one `prime-agent.worker_recovery` entry |
| two simultaneous `-r` on the same file | one worker, one `list --all` record, `clients: 2`, transcript only grew |
| `SIGKILL` the supervisor under a live worker | replacement supervisor on the same socket (new `supervisorGeneration`), same worker pid adopted, same `activeSessionId`, TUI keeps working |
| `SIGKILL` worker and supervisor | `-r` revives the same `sessionId`; one live root; no interrupted effect repeated |

`prime-agent attach <agent>` never reopens a dead resident root (three
measured messages; upstream item F2). That is a documentation gap, not a
missing capability: the custom registry, incarnation fence and reopen loop
existed to do what `-r` and the per-path lease already do
(`reclaimStaleWorkerRegistration`, `openingWorkers`/`joinOpeningWorker` in
0.9.1's `create` path).

## 3. D2 — no duplicate external effect after worker loss: NO custom code

The effect is a real model tool call: the scripted model asks Prime's
`ipython` tool to append one line to a file and sleep; the worker is
`SIGKILL`ed only after the line is on disk; every model request and tool
execution is logged to JSONL.

| Stock surface | Effect lines | Model calls | Tool calls |
| --- | ---: | ---: | ---: |
| interactive TUI, model tool call, then `-r` reopen | 1 | 1 | 1 |
| negative control: same prompt sent twice | **2** | 2 | 2 |
| `prime-agent send <agent> "…"` | 1 | unchanged after recovery | 1 |
| `prime-agent schedule add <agent> "in 1m" -- "…"` | 1 | unchanged | 1 |
| `prime-agent -p` (client-owned) | 1 | 1 | 1 |
| `--mode rpc` `{"type":"bash"}` (`execute_bash_and_wait` on the wire) | 1 | — | 1 response |

Prime writes its own model-visible `prime-agent.worker_recovery` entry with
`details.operations: ["tool_execution_start"]` and the text that the work
"was not replayed". The supervisor's mutation journal was empty for the whole
TUI experiment: on the product path the effect is a model tool call executed
inside the worker loop, and `prompt` returns at admission, so the command
result the D2 ledger classified is decoupled from the effect.

Does stock Prime ever re-issue a mutating command? The code answer, cited
from 0.9.1 `dist/`: the only automatic re-issue is a byte-identical resend
of the same `clientId + commandId` after reconnect
(`daemon-client.js:177-180, 277-289, 340-350`), answered from the journal;
no client in `dist/` reads `command_result_uncertain`, and no client mints a
new command id. The D2 ledger guarded against a client that does not exist.

The one product-path exposure is a reporting defect: the RPC client is told
`success:false, error:"Daemon worker socket closed"` with no `errorInfo`,
and the supervisor journals it as definite (identical on 0.8.1 and 0.9.1,
still present on `main`; discussion #1978 remains unanswered). It misreports;
it duplicates nothing. Recorded under `docs/upstream/`.

## 4. D8 — explicit persistent session path: NO custom code

18/18 assertions: TUI, `-p`, `--mode json` and `--mode rpc` each leave
exactly one `<sessionDir>/<sessionId>.jsonl` containing the turn; Prime's
own client sends `createCommand.sessionPath`; `--no-session` is the only
pathless surface; `list --json` names a `sessionFile` that exists; after
`SIGKILL` of worker and supervisor, `-r` reopens with unchanged `sessionId`
and `sessionFile`. 0.9.0 additionally "made session path detection
consistent across direct and daemon commands".

## 5. Independent acceptance — `worker finished != work accepted`: USE EXISTING

Measured on Prime 0.9.1 (130 assertions, 0 failures):

| Package | Loads on Prime | Finding |
| --- | --- | --- |
| `pi-squad` 0.20.6 | **no** | the only package that implements the rule as code (children have no `squad_review` tool); breaks on Prime's `@earendil-works/pi-ai/compat` alias, on `Theme not initialized` headlessly, and ends tasks only on `agent_settled`, which Prime never emits (task hangs; scheduler fails after five retries blaming the provider); its reviewer role's `--tools read,bash` refuses to start on Prime |
| `pi-subagents` 0.65.0 | **no** | same alias break (loads after rewriting five specifiers) |
| `pi-tasks` 0.2.5 | yes | completion blocked in its reducer (evidence, criteria, blockers, plan steps); `force_with_reason` is a recorded bypass (warning, confidence capped) available to the producing agent |
| `pi-pr-review` 1.17.10 | yes | frozen reviewed-head SHA, staleness re-check at publish, stale `APPROVE` refused by default, idempotent publication ids |
| `pi-governance-pipeline` 1.0.16 | yes | wants to own `AGENTS.md`/`SOUL.md`/`SYSTEM.md`/`MEMORY.md`; shell half hardcodes `pi` |
| `@gotgenes/pi-subagents` 21.4.0 | yes | the only delegation package loading unpatched; agent files with locked fields and complete tool allowlists; terminal state from its own event bus |
| Prime native | — | `--autonomous-gate "<cmd>"` is a host-owned gate the model cannot pass by claiming success (identical model script, opposite outcomes); `goal.complete()` is prompt-only; `rlm.run` negotiates only `name`/`model`/`thinking`, so a read-only reviewer child is not expressible |

Two substrate facts govern the disposition. Prime's default tool set is
`{ ipython }`, so every role runs "the kernel" or nothing. And an
environment variable is not a role boundary: a later invocation inherits any
variable it does not set from the shared daemon supervisor. Both were
measured, and both defeat any *local* attempt to make "the implementer
cannot write the acceptance record" true: a file the reviewer writes is a
file the implementer (same OS user, same kernel) can write; a tool an
extension registers for "reviewer" sessions is callable from any session
that claims the name. The bake-off's proposed 128-line `cg_accept` plugin
keyed its refusal on `CG_ROLE=reviewer` and described itself as "a check on
the agent loop, not a security boundary". It was not built.

The record an implementer genuinely cannot write is external. Per ADR 0008
invariant 9 and this machine's orchestration policy, GitHub is the
engineering source of truth and the ChatGPT foreman is the reviewer of
record and merge authority. An earlier revision of this proof named the
GitHub pull-request review as that record, on the grounds that GitHub
refuses an author's self-approval. The foreman measured that claim on
2026-09-05 and it failed: on this repository the `protect-main` ruleset
requires zero approving reviews, and the foreman's GitHub connector is the
same GitHub user as the PR author, so GitHub refused *its* review too. One
GitHub identity cannot be a boundary against itself. Therefore:

- the **acceptance record** is the foreman's correlated reply in its own
  ChatGPT thread: a message that echoes the delivery id and names the head
  SHA, written by a model the implementer does not run, in a thread the
  implementer can append to but not author as the foreman (§6). GitHub merge
  happens only after that verdict, and is bound to the same head SHA;
- **stale acceptance** is refused by the correlation rule (a verdict naming
  another head is recorded as rejected) and by `pi-pr-review`'s reviewed-head
  binding (`allowStaleApprovals` defaults false) for GitHub-side reviews;
- **candidate completion stays review-blocked** because the run may be
  wired with `--autonomous-gate` to a command that checks for the correlated
  verdict, and the model cannot satisfy that gate by talking;
- **restart** changes nothing: the gate reads external state;
- **evidence** is the `pi-tasks` ledger inside the session, advisory by
  design, with its bypass recorded rather than hidden.

That is `USE EXISTING` plus prompts and configuration. The role files
(`harness/agents/`) carry the independence rule in prose in
`@gotgenes/pi-subagents`' format; the `cg-review` prompt carries the review
procedure.

## 6. ChatGPT foreman loop — `pi-gpt`, measured live on the exact thread

| Candidate | Result on Prime 0.9.1 |
| --- | --- |
| `pi-gpt` 0.4.3 (MIT) | **adopted.** Loads unmodified. Message ids and a true active-branch read. Continues any conversation id with no provenance gate. Drives `chatgpt.com/backend-api` with the user's Codex login token and solves the provider's sentinel/proof-of-work/turnstile checks on the send path. Declared repository returns 404, so the npm tarball (integrity in `pins/pins.json`) is the only source. Zero durability across a send; the delivery id in the body plus readback is the reconciliation. Two shipped defects carried as compatibility risks: no assertion that the returned `conversation_id` equals the requested one; a swallowed `leafMessageId` failure sends with a fabricated parent. |
| `pi-oracle` 0.7.20 (MIT) | **does not load**, silently: imports `CONFIG_DIR_NAME`, `ProjectTrustStore`, `hasTrustRequiringProjectResources` as runtime values Prime does not export; branches on `ctx.mode`, which Prime lacks; touches `ctx.ui.theme` while Prime reports `hasUI === true` headlessly, which throws and kills the daemon worker (a Prime bug). With a 21-line feature-detection patch plus one compatibility module it loads unmodified, registers all five tools, records the exact `chatgpt.com/c/<id>`, and its job record and `lifecycleEvents` survive `SIGKILL` of the detached worker. Patch and issue drafts: `docs/upstream/2026-09-04-pi-oracle-*`. Remains the browser-backed alternative once upstream lands the patch. |
| `@cobuild/review-gpt` 0.5.145 | implements the same envelope ideas as a standalone browser CLI (not a Pi extension); `UNLICENSED`. Not adopted. |

An earlier revision rejected `pi-gpt` under ADR 0008 §8 without running the
probe the transport review had recommended. The user, as the account owner,
then explicitly accepted the terms and account-suspension risk and asked for
the best-working transport; ADR 0008 §8 carries that amendment. Judged on
capability alone, `pi-gpt` is the only candidate that loads on the pinned
Prime today, and the only one with message identity.

The live round trip ran on 2026-09-05 (UTC) against the real account, through
`pi-gpt` 0.4.3's own modules, unmodified, using the Codex login token, into
the foreman thread the user created in the browser
(`https://chatgpt.com/c/6a97b52c-90e0-83ea-9dfd-56fdba1c1855`, ChatGPT project
"commandgovernor", 1,803 nodes before the send):

| Step | Observed |
| --- | --- |
| read the thread | current leaf `1de4fce8…` = the foreman's handoff message; bound to it |
| durable pre-send record | delivery id `CG-D-47B3FJU5QW2EG43V`, conversation id, parent id, our message id `e46474dc…`, written before the send |
| send (sentinel + proof-of-work + turnstile inside the package) | 07:52:57Z; the message is in the thread at 07:53:00Z with our id and parent = bound leaf; `complete()` returned the requested conversation id |
| readback | the foreman ran 40 tool calls against GitHub, then answered at 07:56:54Z (message `6f1e1dec…`); first line `CG-D: CG-D-47B3FJU5QW2EG43V`, `VERDICT: REQUEST_CHANGES`, three numbered items, bound to head `d76e307…` |
| effect | exactly one: the three items became this revision of PR #24 |

Python `urllib` with the same token got a Cloudflare challenge page; Node's
`fetch` with the package's header set did not. The read leg needs no
security-control tokens; the send leg does, and the package solved them.
The foreman's own reply was the first measurement of item 1 below: it tried
to submit a review on GitHub and was refused because it is the same GitHub
user as the author, which is why the thread verdict, not the GitHub review,
is the acceptance record (§5).

Two follow-ups the same day, both kept in this repository rather than waited
on. First, the package is **vendored**: the npm tarball is committed under
`pins/packages/` (sha512 = the pin's `integrity`), `scripts/bootstrap.sh`
extracts it and applies the committed patch `pins/patches/pi-gpt-0.4.3-foreman-guards.patch`,
and Prime installs it by path. The patch closes the two shipped defects in
`gpt_chat` (a drifted reply now fails instead of being reported as the
requested thread's; an unreadable leaf now fails before sending instead of
sending under a fabricated parent); TRN-003 runs the patched tool through
the extension's own entry point against the mock and shows both failures
plus the passing control. Second, the transport was verified **inside a
Prime worker**, not only from a script: with the scripted model issuing the
tool calls and the real Codex login, Prime 0.9.1 executed
`gpt_get_conversation` on the foreman thread (the APPROVE reply came back
with message ids) and `gpt_chat` into a temporary chat (ChatGPT echoed the
probe token, conversation id returned). That is the opt-in live lane
`conformance/runtime/live-chatgpt.test.ts` (LIVE-001…003), the only test in
the repository that can fail when the provider changes.

The correlation rules are the `cg-foreman` skill (`harness/skills/cg-foreman`),
not code: the `CG-D` / `CG-TASK` / `CG-REV` / `CG-REPLY-CONTRACT` envelope,
the reply must echo the delivery id, a reply naming another head is recorded
as rejected, a send is bound to the thread's current leaf, and an ambiguous
send is classified by reading the thread and never resent. The delivery id
must contain letters because `pi-gpt`'s readback redaction replaces any run
of ten or more digits with `<PHONE>`. The credential-free conformance file
`conformance/runtime/foreman-transport.test.ts` (TRN-000…003) protects the
three package facts the rules stand on (exact-thread binding, one request
per send, the repository's patch) against a re-vendor; the rules themselves
are not re-encoded as tests; §12.

## 7. Tool gating and user-owned decisions — an upstream gap no plugin can close

Prime 0.9.1 has no permission or approval system. The highest-adoption Pi
permission package, `@gotgenes/pi-permission-system` 31.1.0, fails to load
on Prime (`getPackageDir` is defined in Prime's `config.js` but not
exported; `ctx.isProjectTrusted` absent) — silently, in every headless
mode, with the violating call executing ungated. Patched to load, its
deny/allow/ask/`gate_error` semantics are correct, and it still cannot see
the product path: Prime's only tool is the REPL, `bash()` spawns inside the
kernel (`rlm/bash.py:652`, `:183`) with no host round trip, and the
`tool_call` event sees one opaque `{code}`. Policy `bash: {"*": "deny"}`
produced no permission entry and the target directory was deleted.

A Command Governor extension has exactly the same interception point, so
this is not a `PLUGIN` candidate. It is recorded as the substrate's open
security limitation (threat model) with the upstream ask (a host hook for
`rlm.bash`, the same bridge shape Prime has for `rlm.run`) and the interim
control (OS containment of the kernel process).

## 8. Ecosystem refresh — what changed since 2026-09-02

- **Prime 0.9.0/0.9.1** (2026-09-01): agent roster with push updates
  (`recovering`/`failed`/`lastHeardFromAt`), exactly-once fix for remote
  agent messages, direct session transport, consistent session-path
  detection, worker identity/instance ids, many REPL fixes; extension and
  package API byte-identical to 0.8.1; schema revision 22 → 25 (26 on
  `main`). Command Governor re-pinned to 0.9.1 with verified assets.
- **Still missing in Prime at `main`:** `agent_settled` (Pi has had it since
  0.80.4); the worker-loss journal fix; any permission system; an ACP client;
  the `./hooks` package export (declared, path absent).
- **Upstream Pi 0.85.0** added durable tool execution
  (`effect_pending` + `replay: "never"`, "does not assert that the external
  effect failed") — Command Governor's D2 semantics, upstream, but wired
  only into Pi's experimental `mini` path and absent from Prime's fork base.
- **Package compatibility is the governing fact:** no package in the
  5,337-item catalogue carries a Prime compatibility statement; Prime's jiti
  alias maps `@earendil-works/pi-ai/*` subpaths to a file path, breaking
  every package that imports `pi-ai/compat`; four "settled"-class packages
  are inert on Prime. Admission is therefore by observed load, not by claim.

Full detail, with citations: the session's `research-ecosystem.md`
(841 lines) and `research-deepseek-vs-prime.md` (1,538 lines).

## 9. DeepSeek Harness donor ideas — eight of ten already owned

| Idea (ADR 0010 §) | Verdict | Owner |
| --- | --- | --- |
| capability seams, one owner per concern (§6) | USE EXISTING + config | Prime package precedence and `+path`/`-path` filtering; `pins.json` `concerns[]` |
| append-only facts + projections (§7) | USE EXISTING | Prime session JSONL + custom entries; `pi-tasks` events |
| durable Session vs Activation (§8) | USE EXISTING | `sessionId` vs `activeSessionId`, supervisor and event generations, start-id-hardened lease |
| fail-loud capability negotiation (§9) | USE EXISTING (mechanism) | `rlm.run` rejects unknown kwargs before any side effect; only `name`/`model`/`thinking` negotiable (upstream gap for per-child tool policy) |
| durable team mailbox (§10) | gap, not a CG requirement | Prime `agent_message` receipts are process-local; `pi-squad`'s durable mailbox does not load on Prime; the task-level obligation lives in GitHub/`pi-tasks` |
| bounded workflows (§11) | USE EXISTING / not a requirement | RLM depth default 2, `waitForRlmQuiescence`, orphan reaper; no CG workflow engine |
| PTC / `run_code` (§12) | not a requirement | Prime is already PTC-shaped (one tool, typed returns, intermediates out of context); the missing property (sub-calls re-entering a gated pipeline) is §7's gap |
| fail-closed approvals (§13) | package, not Prime | `tool_call` block short-circuits and cannot be widened; the package does not load (§7) |
| credential references (§14) | USE EXISTING | `AuthStorage` + per-call resolution + serialized OAuth refresh; `$VAR` interpolation differs from Pi 0.85 (upstream) |
| component/token-cost accounting (§15) | USE EXISTING (cost) + config (metadata) | per-message `Usage.cost`, own-vs-total via context tree; component metadata in `concerns[]` |

DeepSeek `master` since alpha.5 reverses the donor review's §8 (it now ships a
session-format migration chain), adds a durable `assistant/attempt` event,
and ships its cross-process write lease; none of it changes the substrate
decision. Prime cannot drive DeepSeek's ACP server (Prime has no ACP client).
Zero DeepSeek ideas require a Command Governor subsystem.

## 10. PR #24 audit

PR #24 (`cleanup/composition-test-shrink`, one commit `66f0a25`) was stacked on
the already-merged `docs/adr-0010-composition-boundary`; retargeting to
`main` changed nothing in its diff.

- Deleting the 110-file Rust workspace, `Cargo.*`, `deny.toml`,
  `rust-toolchain.toml` and the Rust CI jobs: **correct** (ADR 0010 §1; the
  invariant catalogue is preserved).
- Deleting nine TypeScript test files that only proved `governor/*`
  internals: **correct**, and moot once `governor/*` is gone.
- Retaining D1/D2/D8 as `TEMP WORKAROUND` "pending the package-path
  reproducer": **superseded** by §2–§4; those workarounds are deleted here
  with their remaining tests.
- Its document rewrites described a "temporary D2 compatibility layer";
  rewritten again for the final state.
- **More was deletable:** six Rust-era design documents (3,400 lines) moved
  to `docs/history/`; the transport stub, role schema and authorities file
  removed; `conformance/` reduced to black-box tests.

The PR was retargeted onto `main` and finished on its branch.

## 11. What survives, and why each line exists

| Path | Kind | Existence reason |
| --- | --- | --- |
| `pins/pins.json`, `pins/SHA256SUMS`, `pins/prime-0.9.1/*` | manifest | exact substrate and package pins; the only place a version string lives |
| `scripts/bootstrap.sh` | shell | two-authority asset verification before npm; refuses drift |
| `scripts/conformance.sh` | shell | test runner and process sweep |
| `harness/package.json`, `harness/settings.project.json` | config | Prime package manifest; the project settings that install the pinned packages |
| `harness/agents/*.md` | config (prose) | role definitions in the delegation package's format |
| `harness/skills/cg-conformance/SKILL.md`, `harness/prompts/cg-review.md` | Agent Skills / prompt | procedures, progressively disclosed |
| `conformance/**` | test code | black-box product invariants and distribution facts |
| `docs/**` | documents | ADRs, research, upstream records, history |

## 12. Before / after

| Measure | `main` @ 902814c | this change |
| --- | ---: | ---: |
| custom production TypeScript (`governor/*`, `harness/extensions`) | 4,723 | **0** |
| Rust source + tests (`crates/`) | 49,142 (110 files) | 0 |
| shell (`scripts/bootstrap.sh`, `scripts/conformance.sh`) | 251 | 276 |
| harness configuration and prose (roles, skill, prompt, manifest, settings) | 495 | 398 |
| conformance TypeScript/Python | 5,534 in 29 test files (+6 lib) | 4,433 in 10 test files (+10 lib); 87 tests, 15 suites, ~3 min; plus the opt-in live lane (3) |
| tracked files | 238 | 91 |
| Prime pin | 0.8.1 | 0.9.1 |
| pinned packages | 0 | 3 |
| assigned concerns whose owner is custom code | 8 of 10 | 0 of 12 |

Which external capability replaced each deleted subsystem:

| Deleted subsystem | Replaced by |
| --- | --- |
| session registry, incarnation fence, recovery lease, reopen loop | Prime per-path session lease, `openingWorkers` convergence, stock `prime-agent -r <sessionFile>`, supervisor replacement by a live worker |
| mutation ledger, outcome classifier, proof matrix, command digest | Prime worker-recovery journal and transcript marker; the fact that no stock client re-issues a mutation |
| durable filesystem helpers, process-identity probe | nothing needed; they served the two stores above |
| daemon client, protocol slice, version gate | Prime's own clients; `daemon_hello` compared with the manifest by the conformance suite |
| environment allowlist | not needed; Command Governor runs no process of its own |
| foreman transport stub | the `cg-foreman` skill over the pinned `pi-gpt`; TRN-000…003 in the suite |
| role schema and its validator | `@gotgenes/pi-subagents` agent-file format, observed through the package by LOAD-001 |
| authorities inventory | `pins/pins.json` `concerns[]`, checked by OWN-001 |
| Rust oracle | `docs/research/2026-09-01-rust-invariant-catalog.md`, Git history, and the black-box suite for the semantics that survived |

## 13. Open items that only the user or upstream can close

1. **Browser-backed transport alternative:** `pi-oracle` waits on its
   compatibility patch upstream, plus `zstd` and `agent-browser` in the Nix
   configuration and one `/oracle-auth`. Not needed for the product: the
   authenticated round trip ran on `pi-gpt` (§6).
2. **Filing upstream:** the Prime gaps in
   `docs/upstream/2026-09-04-prime-extension-and-daemon-gaps.md` and the
   pi-oracle issue/patch, through Prime's Discussions gate and pi-oracle's
   issue tracker (outward-facing; not done autonomously).
3. **Tool gating:** no control exists on the substrate; OS containment of
   the kernel process is the user's choice until Prime exposes a kernel
   hook.
4. **Real-model runs:** no provider credentials were available; every
   proof used scripted models, which is the right instrument for package
   and substrate mechanics and the wrong one for model judgement.
