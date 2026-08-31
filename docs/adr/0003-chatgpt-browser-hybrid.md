# ADR 0003: Browser-backed hybrid ChatGPT Web transport

- **Status:** Proposed, gated by live spike
- **Date:** 2026-08-31

## Context

Command Governor must wake one exact ChatGPT foreman conversation after durable
worker attention appears. Current ChatGPT Web submission is a protected private
browser flow whose details change. Independent current projects have found direct
server-side writes brittle even with valid authentication, while the first-party
SPA can perform the current authentication/challenge/submission behavior.

A DOM-only implementation, however, has weak evidence after Send. UI states such
as empty composer or Stop button do not prove which user message was accepted.

## Decision

V1 uses a **browser-backed hybrid**:

- headed system Google Chrome/Chromium;
- dedicated owner-private Command Governor profile;
- Rust CDP via `chromiumoxide` behind a replaceable browser trait;
- real ChatGPT SPA performs sensitive message submission;
- DOM is used for exact structural control: route/composer/app-selection/Send;
- CDP Network/Target/Page evidence is preferred for accepted submission and
  physical-turn observation;
- narrowly bounded authenticated reads/passive message-tree interpretation may be
  used for reconciliation;
- all unofficial ChatGPT behavior is isolated in `governor-chatgpt-web`.

Command Governor does **not** implement direct private ChatGPT message submission,
Sentinel, Turnstile, proof-of-work, CAPTCHA solving, entitlement bypass, or
rate-limit/anti-abuse circumvention.

## Exact binding

V1 has one active canonical `/c/<id>` foreman binding and monotonic binding
generation. The adapter never uses last-active/current tab/history as authority.
The exact resolved conversation is verified before composer mutation and again
immediately before Send.

New-chat binding, if added, commits only after a concrete final `/c/<id>` exists.

## App selection

Current ChatGPT app selection is message-scoped. Every wake must prove the Command
Governor app is selected/mentioned for that exact message before Send. A plain
prose instruction to use the app is insufficient if the first-party UI has not
made the connector available to the turn.

## Delivery semantics

Each obligation/binding-generation/revision has a deterministic delivery ID.

The store commits:

```text
pending -> claimed
```

before browser I/O.

Immediately before exact Send activation it commits internal:

```text
activation_armed
```

Then one physical Send action may be invoked.

Terminal delivery projection:

- `failed` — proven no submission; bounded retry is safe;
- `accepted` — strong semantic evidence proves intended user message was submitted
  to exact bound conversation;
- `ambiguous` — submission may have happened but cannot be proved.

Any previous-process `claimed`/`activation_armed` without terminal result is
converted to ambiguous during startup **before browser recovery**. Accepted or
ambiguous is never automatically resent.

This deliberately chooses at-most-once safety over guaranteed delivery. A crash
can produce zero sends and a durable ambiguity.

## Observation

Prefer correlated SPA-generated request/message-tree evidence. Whole private
requests, response bodies, headers, cookies, or protocol traces are not persisted
in safe state/logs. Private endpoint names/schema are implementation observations,
not stable API contracts.

Weak UI signals alone never produce accepted.

Physical assistant turn settlement is separately observed and may enable bounded
resume policy, but it does not ACK the obligation.

## Headed versus headless

Current public field evidence makes headed real Chrome the V1 support target.
`--headless=new` gets a separate conformance run and is experimental unless it
matches headed behavior without stealth/challenge bypass.

## Alternatives

### Fully private API client

Rejected for the write path: brittle private protocol, credential expansion, and
pressure to reproduce protective mechanisms.

### DOM-only browser automation

Rejected as the complete evidence plane. DOM remains necessary for structural
control but is insufficient for Send/reconciliation truth by itself.

### `headless_chrome` Rust crate

Retained as fallback driver. It is capable but a less natural first fit for the
Tokio-oriented daemon than `chromiumoxide`.

### Wry

Rejected for V1 correctness path: platform WebViews do not provide one uniform
Chrome/CDP target/profile/network contract and V1 has no GUI requirement.

### CEF

Deferred. Full embedded Chromium ownership adds large browser packaging, update,
security, and cross-platform burden before system Chrome has failed a real need.

## Gate

This ADR is not considered implementation-proven until the authenticated live
spike in `docs/browser-transport.md` passes. A duplicate Send is a gate failure.
