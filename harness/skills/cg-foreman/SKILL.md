---
name: cg-foreman
description: How to report to, and take a verdict from, the ChatGPT foreman in its exact existing thread through the pi-gpt tools (gpt_get_conversation, gpt_chat), with delivery-id correlation, stale-revision rejection and no resend on an ambiguous send. Use when asked to hand work to the foreman, request a review verdict, read the foreman's reply, or reconcile a send that may or may not have landed.
---

# The ChatGPT foreman loop

The foreman is a ChatGPT Web thread the user created in the browser. It is
the reviewer of record and the merge authority; this skill only moves a
report into that thread and a verdict back out. The transport is the pinned
`pi-gpt` package (`pins/pins.json` `foreman-transport`), which uses the
user's Codex login (`~/.codex/auth.json`) against ChatGPT's backend. There
is no Command Governor code in this loop; the rules below are the product.

## Find the thread; never ask for it

`gpt_list_chats` only lists chats this package started. The foreman thread
was started in the browser, so read it from the account instead: the
conversation list is `gpt_get_conversation` on an id you already hold, or
the project's own list when the user works in a ChatGPT project. Record the
exact id in the task's evidence (`task_evidence`) the first time it is
learned. A thread id in evidence is the only acceptable source afterwards;
a remembered title is not.

## Before every send: read, then bind

1. `gpt_get_conversation(conversation_id)` and take the **last message on
   the active branch**. That is the message you reply to. If it is not the
   foreman turn you expected (the user may have posted in the browser),
   stop and say so; do not send under a moved leaf.
2. Bind the send to the exact work: the head SHA of the PR or the commit,
   never "the branch".

## The envelope

The first lines of every message to the foreman are literal, one per line:

```text
CG-D: CG-D-<id>
CG-TASK: <what this answers: the foreman message id and date, or the issue>
CG-REV: <PR number> head <40-hex SHA> on base <branch>
CG-REPLY-CONTRACT: first line of the reply must be exactly "CG-D: CG-D-<id>"; then "VERDICT: APPROVE" or "VERDICT: REQUEST_CHANGES" for that head SHA; if REQUEST_CHANGES, a numbered list of exact changes; review no other head.
```

`<id>` is random and **must contain letters** (base32 of at least ten
random bytes). `pi-gpt` redacts any run of ten or more digits as a phone
number on readback, so an all-digit id would be destroyed by the reader.

Write the envelope into the task's evidence **before** `gpt_chat` runs, with
the conversation id and the leaf message id you bound to. That record is
what survives a crash mid-send.

## Send

`gpt_chat({ conversation_id, prompt, model, thinking_effort })` with the
thread's own model (read `default_model_slug` from the conversation) and
`temporary: false`. Assert that the returned `conversation_id` equals the
one requested; `pi-gpt` does not, and a mismatch means the reply landed in
another thread and must be treated as **not delivered**.

Thinking models answer asynchronously: the call may return empty text after
its five-minute poll. That is not a failure. Go to readback.

## Readback and correlation

`gpt_get_conversation(conversation_id)` again. Walk the active branch, find
your own message (the one carrying your `CG-D` line), and take the first
**assistant** message after it whose status is finished. Then:

- **Echo.** The reply's first line must be `CG-D: <your id>`. A reply
  without the echo is not a verdict; ask the foreman to restate with the id.
- **Revision.** The verdict names a head SHA. If it is not the SHA you sent,
  record the reply as **rejected: stale revision** and do not act on it.
- **Placement.** If your message is not on the active branch (the user
  edited or branched in the browser), the reply is not for the current
  work. Record it and re-read; do not send again on your own.

Affect the work exactly once per verdict: one evidence entry, one PR
comment or one set of changes, keyed by the delivery id. A second reply
carrying the same id changes nothing.

## Ambiguous send: never resend

If `gpt_chat` failed, timed out, or the process died between the evidence
record and the returned text, the send is **ambiguous**. Resolve it by
reading, not by sending: `gpt_get_conversation` and search the active
branch for your `CG-D` line.

- Found: the send landed; continue with readback.
- Not found after two reads a minute apart: the send did not land; you may
  send once more with a **new** delivery id and a note that the previous id
  was never delivered.

Two messages carrying the same delivery id in one thread is the failure
this rule exists to prevent; the foreman would answer both.

## The verdict is the acceptance record

A correlated reply carrying `VERDICT: APPROVE` for the exact head SHA is the
independent acceptance record: it is written by a model the implementer does
not run, in a thread the implementer can append to but cannot author as the
foreman. Record it in the task's evidence with the reply's message id, then
merge on GitHub bound to that same head. A GitHub review is not the record on
a single-identity repository: the ruleset requires no approvals and the
foreman's GitHub connector is the same user as the author. `REQUEST_CHANGES`
means make exactly the numbered changes, push, and send a new envelope with a
new delivery id for the new head; never reuse an id across revisions.
