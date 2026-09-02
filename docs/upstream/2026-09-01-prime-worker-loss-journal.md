# Upstream proposal: do not journal a worker-transport loss as a definite result

Status: **filed** by the repository owner as
[PrimeIntellect-ai/prime-agent#1974](https://github.com/PrimeIntellect-ai/prime-agent/issues/1974).
The text below is what was proposed. Command Governor's own guard
(`governor/mutation/classify.ts` with the reviewed proof matrix in
`governor/mutation/proof.ts`) does not depend on this landing.

## Issue text

**Title:** Supervisor journals a worker socket loss as a definite command
failure; clients cannot tell it from a pre-effect rejection

**Version:** v0.8.1 (`514633727bf26d74f39f3119c2b0e31a5ceb2a9d`), macOS
15.7 arm64, Node 24.19.

**What happens.** `docs/daemon.md` says: "A received command without a
durable result is reported as uncertain and is not replayed." That holds
when the *supervisor* dies mid-command: retrying the same
`clientId + commandId` returns `errorInfo.code = command_result_uncertain`.
It does not hold when the *worker* dies mid-command. `DaemonWorkerClient`
rejects the in-flight request with `new Error("Daemon worker socket
closed")`; `DaemonSupervisor.handleClientCommand`'s catch path serialises
that into `failure(command.id, command.type, error, serializeDaemonError(error))`,
which carries no `errorInfo` because `serializeDaemonError` does not know
the error class, and then calls `commandJournal.recordResult(...)` because
the only exclusion is `isSupervisorGenerationStale(error)`. The untyped
failure becomes the durable result and is replayed on every retry.

**Why it matters.** A worker can die *after* it performed the external
effect and before it reported. Reproduced with
`execute_bash_and_wait` running `echo effect >> file; sleep 4` and a
`SIGKILL` of the worker pid once the line is on disk: the client is told
`success: false, error: "Daemon worker socket closed"`, indistinguishable
on the wire from a rejection that proves nothing ran. A client that
trusts the failure and retries under a new command id duplicates the
effect. Only the wording of `error` hints at what happened, and wording is
not a contract.

**Proposed change** (either is sufficient; both are better):

1. Leave the journal entry pending when the failure is a worker transport
   loss, so retries receive `command_result_uncertain` exactly as they do
   after supervisor loss. That is the documented semantics.
2. Type the loss: raise a dedicated error class from `DaemonWorkerClient`
   on `close`/`error` while requests are outstanding, and add an
   `errorInfo` code (for example `worker_connection_lost`) for it, so a
   client can classify the outcome structurally.

## Sketch of the patch

Against `packages/coding-agent/src/modes/daemon/` at v0.8.1. Not built or
tested against Prime's suite; offered as a starting point.

```diff
--- a/daemon-worker-client.ts
+++ b/daemon-worker-client.ts
@@
+export class DaemonWorkerConnectionLost extends Error {
+	readonly code = "worker_connection_lost" as const;
+	constructor(message: string) {
+		super(message);
+		this.name = "DaemonWorkerConnectionLost";
+	}
+}
+
+export function isDaemonWorkerConnectionLost(error: unknown): error is DaemonWorkerConnectionLost {
+	return error instanceof DaemonWorkerConnectionLost;
+}
@@
-		socket.on("error", (error) => this.notifyClosed(socket, error));
-		socket.on("close", () => this.notifyClosed(socket, new Error("Daemon worker socket closed")));
+		socket.on("error", (error) => this.notifyClosed(socket, new DaemonWorkerConnectionLost(`Daemon worker socket error: ${error.message}`)));
+		socket.on("close", () => this.notifyClosed(socket, new DaemonWorkerConnectionLost("Daemon worker socket closed")));

--- a/daemon-protocol.ts
+++ b/daemon-protocol.ts
@@ export type DaemonErrorInfo =
 	| { code: "session_already_active"; sessionPath: string; activeSessionId?: string }
-	| { code: "command_result_uncertain"; clientId: DaemonClientId; commandId: DaemonCommandId };
+	| { code: "command_result_uncertain"; clientId: DaemonClientId; commandId: DaemonCommandId }
+	| { code: "worker_connection_lost"; clientId?: DaemonClientId; commandId?: DaemonCommandId };

--- a/daemon-errors.ts
+++ b/daemon-errors.ts
@@ export function serializeDaemonError(error: unknown): DaemonErrorInfo | undefined {
+	if (isDaemonWorkerConnectionLost(error)) {
+		return { code: "worker_connection_lost" };
+	}

--- a/daemon-supervisor.ts
+++ b/daemon-supervisor.ts
@@ handleClientCommand catch path
 			let response = failure(command.id, command.type, error, serializeDaemonError(error));
-			if (journalIdentity && !isSupervisorGenerationStale(error)) {
+			// A worker transport loss proves nothing about the effect: leave the
+			// receipt pending so retries are reported uncertain, exactly as after
+			// supervisor loss, instead of journaling a definite failure.
+			if (journalIdentity && !isSupervisorGenerationStale(error) && !isDaemonWorkerConnectionLost(error)) {
```

With (1) alone, a retry after worker loss returns
`command_result_uncertain`, and existing clients already understand that
code. With (2) alone, the first response is typed and a client can decide.
Together, the first response is typed and the retry is uncertain.

## Test to add upstream

The Command Governor runtime test
`conformance/runtime/d2-worker-loss-uncertain.test.ts` is the reproducer;
it drives only the public daemon protocol (create with `sessionPath`,
attach, `execute_bash_and_wait`, `SIGKILL` on the reported worker pid,
same-identity retry) and can be ported to Prime's process tests verbatim.
