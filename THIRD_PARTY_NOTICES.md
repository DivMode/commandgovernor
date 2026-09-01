# Third-Party Notices and Provenance

Command Governor is licensed under MIT. This notice records architecture
inspiration/research separately from future compiled dependencies.

At the 2026-08-31 architecture phase, the repository contains no vendored or
copied third-party implementation source code. The project is independently
implementing the documented semantics described below in Rust.

## Architecture / protocol references

### Tandem

- Project: `Maxmedawar/tandem` and `DivMode/tandem`
- License: MIT
- Upstream revision re-verified: `a98bcafd2c40ae5473b85fe41183e4f391933799`
- DivMode main re-verified: `afc3192e9caaa1affb7c9ed97c6c66df0605c2ee`
- DivMode PR #6 head re-verified: `af568233e1aae2d4cc343b38ca0e2a1a248e7857`
- URL: <https://github.com/Maxmedawar/tandem>
- URL: <https://github.com/DivMode/tandem>

Concepts studied include runtime/Herdr adaptation, ownership/provenance, MCP
orchestration, native Claude lifecycle, stale-client behavior, completion
barriers, and turn/session fencing.

No Tandem author or maintainer is implied to endorse Command Governor.

### codex-chatgpt-web

- Project: `miuuyy/codex-chatgpt-web`
- License: MIT
- Initial architecture review revision: `d7675fc7767a8f19b908f3e5d0e357699d1d9fdf`
- Current main re-verified at review completion:
  `06637f97a68faaa636986dad7514c7e2b3449347`
- Architecture document blob at current main:
  `4367828fae8ad0a53e4adb0af19c1589640cb37c` (unchanged from the source
  actually inspected for the architectural findings)
- Release reviewed/re-verified: `v4.0.7`
- URL: <https://github.com/miuuyy/codex-chatgpt-web>

Concepts studied include exact browser-surface ownership, retained ChatGPT
conversation lifecycles, connector identity/schema compatibility, send/settlement
boundaries, reconnect without replay, and compaction handoff.

No codex-chatgpt-web author or maintainer is implied to endorse Command Governor.

### CCCC

- Project: `ChesterRa/CCCC`
- License: Apache-2.0
- Revision re-verified: `5f0b83242d09c88b1e2267d1056fc5bf64feb626`
- URL: <https://github.com/ChesterRa/CCCC>

Command Governor studied CCCC primarily as a protocol/semantics reference,
including append-only daemon authority and documented delivery states such as
`claimed`, `accepted`, `failed`, and `ambiguous`.

The current plan is to independently implement those safety semantics, not copy
CCCC source code. If Apache-2.0 source is copied or adapted later, the repository
must preserve all required license/NOTICE attribution and record the exact files,
revisions, and changes here before distribution.

No CCCC author or maintainer is implied to endorse Command Governor.


## Durable-orchestration implementation references — no code copied

### Salvor

- Project: `joseym/salvor`
- License: Apache-2.0
- Revision reviewed: `dd9eb49f6bf854dc1c96b1b1ad7accbc509807b0`
- URL: <https://github.com/joseym/salvor>

Concepts studied include pure event replay, explicit external-effect classes,
write-ahead tool intent, dangling-write reconciliation, idempotency-key handling,
and deterministic kill/failpoint tests. Command Governor will independently
implement the needed semantics and will not inherit Salvor's broader durable
provider/tool payload model.

### Prime Agent

- Project: `PrimeIntellect-ai/prime-agent`
- License: MIT
- Revision reviewed: `9f5edc192cfe3d4737205a2f551d2b6b6e34fe09`
- URL: <https://github.com/PrimeIntellect-ai/prime-agent>

Concepts studied include mutating-command identity, write-ahead command journaling,
completed-result replay, uncertain-result no-replay, generation-aware cursors,
supervisor/worker recovery, and process-safe session leases fenced against PID
reuse.

### Agent Orchestrator

- Project: `ralphkrauss/agent-orchestrator`
- License: MIT
- Revision reviewed: `8b2f3b967e90877c3abac07061dbb2b1e67d2035`
- URL: <https://github.com/ralphkrauss/agent-orchestrator>

Concepts studied include daemon-owned orchestration truth, short-lived structured
orchestrator/reviewer turns, durable notification reconciliation, explicit
notification ACK, request-ID idempotency, and thin MCP/IPC/CLI transports.

No author or maintainer of these projects is implied to endorse Command Governor.
No implementation source from them is currently vendored/copied. Any future source
incorporation requires file-level provenance and the applicable license/NOTICE
obligations before distribution.

## Additional research references — no code copied

These projects were studied to understand current ChatGPT Web browser/private-API
tradeoffs. They are not currently implementation dependencies or copied sources:

- OpenWeb (`imoonkey/openweb`), revision
  `a387b50c829d871839a613732e1b97bfa1946124`
- `Octo-Lex/ChatGPT-Web2API`, revision
  `497527dceabfa3f95961e23c291e618c5570f1ac`
- `stufently/gpt-web-gateway`, revision
  `efb01a32e9e4c7fbebb8acff204c8c2a448c476c`

Rust browser alternatives examined but not copied/depended on yet:

- `mattsse/chromiumoxide` main
  `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`
- `rust-headless-chrome/rust-headless-chrome` main/release
  `0a5c307a85debc450378a1f19e4dac1838d7b22d` (`1.0.22`)
- `tauri-apps/wry` dev
  `bb69d628a905d65042c71a95e85f6921ec9b3264`
- `tauri-apps/cef-rs` dev
  `a2e15ae659c4b3957883e34de879bd8b38360ce5`

See [`docs/research/2026-08-31-technology-review.md`](docs/research/2026-08-31-technology-review.md)
for the architecture evidence derived from these sources.

## Planned external dependencies

No Rust dependency has been added yet. The current proposal includes crates such
as Tokio, serde, thiserror, tracing, clap, `rmcp`, `rusqlite`, uuid, and a deliberate
time crate, plus a Rust CDP library. The official Rust MCP SDK was re-verified at
main `ad9832ec212baf526e1a69d73ee04cd8305ae331`, workspace version `3.1.4`;
that is research context, not a commitment to depend on an unreleased main SHA.

Exact dependency versions/licenses will be re-verified and recorded by
`Cargo.lock`/license policy at the first scaffold commit.

A dependency manifest and generated license report do not replace this provenance
record when source/patterns are materially copied or adapted.
