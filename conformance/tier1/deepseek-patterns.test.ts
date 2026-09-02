import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { CapabilityAlreadyOwned, CapabilityRegistry } from "../../governor/composition/capabilities.ts";
import {
	UnsupportedChildCapability,
	admitChildMessage,
	assertChildProviderSupports,
	confirmChildMessageDelivery,
	dispatchAcceptedChildMessage,
	type AcceptedChildMessage,
	type ChildMessageTransport,
} from "../../governor/composition/child.ts";
import { ComponentExperienceError, validateComponentExperience } from "../../governor/composition/component.ts";
import {
	UnknownRequiredGovernorEvent,
	projectStoredEvents,
	type DurableEventRef,
	type DurableEventSink,
	type GovernorEvent,
	type GovernorEventDraft,
} from "../../governor/composition/events.ts";
import {
	assertActivationMatchesSession,
	governorActivationId,
	governorMessageId,
	governorSessionId,
	type DurableSessionRef,
	type LiveActivationRef,
} from "../../governor/composition/lifecycle.ts";
import {
	childMessageIsAutoRetryable,
	interruptedChildDispatches,
	pendingChildMessages,
	projectChildMailbox,
} from "../../governor/composition/mailbox.ts";
import { SandboxRequirementError, assertSandboxSatisfies } from "../../governor/composition/sandbox.ts";
import { WorkflowValidationError, validateWorkflow, type WorkflowDefinition } from "../../governor/composition/workflow.ts";

describe("DSH-DONOR: append-only event projection contract", () => {
	it("projects contiguous known events, skips only explicit ignorable unknowns, and refuses unknown required facts", () => {
		const known = {
			"known/add": (state: number, event: { readonly data: unknown }) => state + Number((event.data as { value: number }).value),
		};
		const events = [
			{ type: "known/add", seq: 0, committedAt: "2026-09-01T00:00:00Z", data: { value: 2 } },
			{ type: "future/metric", seq: 1, committedAt: "2026-09-01T00:00:01Z", data: {}, ignorable: true as const },
			{ type: "known/add", seq: 2, committedAt: "2026-09-01T00:00:02Z", data: { value: 3 } },
		];
		assert.equal(projectStoredEvents(events, 0, known), 5);

		assert.throws(
			() => projectStoredEvents([{ type: "future/authority", seq: 0, committedAt: "x", data: {} }], 0, known),
			UnknownRequiredGovernorEvent,
		);
		assert.throws(
			() => projectStoredEvents([{ type: "known/add", seq: 1, committedAt: "x", data: { value: 1 } }], 0, known),
			/non-contiguous/,
		);
	});
});

describe("DSH-DONOR: replaceable capability seams retain one Governor authority", () => {
	it("rejects silent duplicate owners and releases ownership explicitly", () => {
		const registry = new CapabilityRegistry();
		const dispose = registry.register({ name: "workflow-engine", owner: "governor-workflow", value: { version: 1 } });
		assert.equal(registry.require<{ version: number }>("workflow-engine").value.version, 1);
		assert.throws(
			() => registry.register({ name: "workflow-engine", owner: "other-plugin", value: {} }),
			CapabilityAlreadyOwned,
		);
		dispose();
		assert.equal(registry.get("workflow-engine"), undefined);
		assert.doesNotThrow(() => registry.register({ name: "workflow-engine", owner: "replacement", value: {} }));
	});
});

describe("DSH-DONOR: durable Session vs process-local Activation", () => {
	it("never treats an activation from another session as authority", () => {
		const session: DurableSessionRef = { sessionId: governorSessionId("session-a"), substrateSessionId: "/sessions/a.jsonl" };
		const correct: LiveActivationRef = {
			activationId: governorActivationId("activation-a1"),
			sessionId: session.sessionId,
			generation: "gen-7",
			processLocal: true,
		};
		assert.doesNotThrow(() => assertActivationMatchesSession(correct, session));
		assert.throws(
			() => assertActivationMatchesSession({ ...correct, sessionId: governorSessionId("session-b") }, session),
			/does not belong/,
		);
	});
});

describe("DSH-DONOR: provider-neutral child delegation with durable mailbox admission", () => {
	it("fails loud when a provider lacks a requested capability", () => {
		assert.throws(
			() => assertChildProviderSupports({ name: "acp", capabilities: ["structured-output"] }, { capabilities: ["persona"] }),
			UnsupportedChildCapability,
		);
	});

	it("does not return acceptance before the durable event commit resolves", async () => {
		let commit: (() => void) | undefined;
		const sink: DurableEventSink = {
			append: async <K extends GovernorEventDraft["type"]>(_event: GovernorEventDraft<K>): Promise<DurableEventRef> =>
				new Promise<DurableEventRef>((resolve) => {
					commit = () => resolve({ seq: 9, committedAt: "2026-09-01T10:00:00Z" });
				}),
		};
		const pending = admitChildMessage(sink, {
			taskId: "task-1",
			parentSessionId: governorSessionId("parent-1"),
			childSessionId: governorSessionId("child-1"),
			messageId: governorMessageId("message-1"),
			contentDigest: "sha256:abc",
		});
		let settled = false;
		void pending.then(() => {
			settled = true;
		});
		await Promise.resolve();
		assert.equal(settled, false, "process-local acceptance must not outrun durable admission");
		assert.ok(commit !== undefined);
		commit();
		const accepted = await pending;
		assert.equal(accepted.durable.seq, 9);
	});

	it("journals dispatch before transport I/O and does not confuse transport return with target delivery", async () => {
		const events: GovernorEventDraft[] = [];
		const sink: DurableEventSink = {
			append: async (event) => {
				events.push(event);
				return { seq: events.length - 1, committedAt: `t-${events.length}` };
			},
		};
		const accepted: AcceptedChildMessage = {
			taskId: "task-2",
			parentSessionId: governorSessionId("parent-2"),
			childSessionId: governorSessionId("child-2"),
			messageId: governorMessageId("message-2"),
			contentDigest: "sha256:def",
			durable: { seq: 4, committedAt: "t-4" },
		};
		const transport: ChildMessageTransport = {
			provider: "deepseek-harness-acp",
			send: async (message) => {
				assert.equal(events[0]?.type, "child/message-dispatch-started", "journal intent exists before transport send runs");
				assert.equal(message.durable.seq, 4);
				return { providerReceipt: "receipt-1" };
			},
		};
		const outcome = await dispatchAcceptedChildMessage(sink, transport, accepted, "attempt-1", new AbortController().signal);
		assert.equal(outcome.state, "dispatched");
		assert.deepEqual(events, [
			{
				type: "child/message-dispatch-started",
				data: { messageId: "message-2", attemptId: "attempt-1", provider: "deepseek-harness-acp" },
			},
			{
				type: "child/message-dispatched",
				data: { messageId: "message-2", attemptId: "attempt-1", provider: "deepseek-harness-acp", providerReceipt: "receipt-1" },
			},
		]);

		const confirmation = await confirmChildMessageDelivery(sink, accepted, "deepseek-harness-acp", "target-session-seq:44");
		assert.equal(confirmation.seq, 2);
		assert.equal(events[2]?.type, "child/message-delivery-confirmed");
	});

	it("classifies an unproved transport failure as uncertain and never retryable-by-default", async () => {
		const events: GovernorEventDraft[] = [];
		const sink: DurableEventSink = {
			append: async (event) => {
				events.push(event);
				return { seq: events.length - 1, committedAt: `t-${events.length}` };
			},
		};
		const accepted: AcceptedChildMessage = {
			taskId: "task-u",
			parentSessionId: governorSessionId("parent-u"),
			childSessionId: governorSessionId("child-u"),
			messageId: governorMessageId("message-u"),
			contentDigest: "sha256:u",
			durable: { seq: 1, committedAt: "t-1" },
		};
		const outcome = await dispatchAcceptedChildMessage(
			sink,
			{ provider: "acp", send: async () => { throw new Error("connection lost"); } },
			accepted,
			"attempt-u",
			new AbortController().signal,
		);
		assert.equal(outcome.state, "uncertain");
		assert.equal(events[1]?.type, "child/message-dispatch-uncertain");
	});

	it("permits retry only when the transport positively proves failure before the effect", async () => {
		const events: GovernorEventDraft[] = [];
		const sink: DurableEventSink = {
			append: async (event) => {
				events.push(event);
				return { seq: events.length - 1, committedAt: `t-${events.length}` };
			},
		};
		const accepted: AcceptedChildMessage = {
			taskId: "task-r",
			parentSessionId: governorSessionId("parent-r"),
			childSessionId: governorSessionId("child-r"),
			messageId: governorMessageId("message-r"),
			contentDigest: "sha256:r",
			durable: { seq: 1, committedAt: "t-1" },
		};
		const outcome = await dispatchAcceptedChildMessage(
			sink,
			{
				provider: "acp",
				send: async () => { throw new Error("not connected"); },
				classifyFailure: () => ({ effect: "not-started", code: "connection_not_opened" }),
			},
			accepted,
			"attempt-r",
			new AbortController().signal,
		);
		assert.equal(outcome.state, "rejected");
		assert.equal(events[1]?.type, "child/message-dispatch-rejected");
	});
});

describe("DSH-DONOR: queued-minus-confirmed child mailbox projection", () => {
	const admitted: GovernorEvent = {
		type: "child/message-admitted",
		seq: 0,
		committedAt: "t0",
		data: {
			taskId: "task-mail",
			parentSessionId: "parent-mail",
			childSessionId: "child-mail",
			messageId: "message-mail",
			contentDigest: "sha256:mail",
		},
	};

	it("keeps a dispatched message pending until target delivery is confirmed", () => {
		const events: GovernorEvent[] = [
			admitted,
			{
				type: "child/message-dispatch-started",
				seq: 1,
				committedAt: "t1",
				data: { messageId: "message-mail", attemptId: "attempt-mail", provider: "acp" },
			},
			{
				type: "child/message-dispatched",
				seq: 2,
				committedAt: "t2",
				data: { messageId: "message-mail", attemptId: "attempt-mail", provider: "acp", providerReceipt: "wire-ok" },
			},
		];
		const beforeConfirmation = projectChildMailbox(events);
		assert.equal(beforeConfirmation.get("message-mail")?.state, "dispatched");
		assert.equal(pendingChildMessages(beforeConfirmation).length, 1);
		assert.equal(childMessageIsAutoRetryable(beforeConfirmation.get("message-mail")!), false);

		const confirmed = projectChildMailbox([
			...events,
			{
				type: "child/message-delivery-confirmed",
				seq: 3,
				committedAt: "t3",
				data: { messageId: "message-mail", provider: "acp", confirmation: "target-seq-7" },
			},
		]);
		assert.equal(confirmed.get("message-mail")?.state, "confirmed");
		assert.equal(pendingChildMessages(confirmed).length, 0);
	});

	it("exposes a crash after dispatch-start as interrupted uncertainty instead of retryable queued work", () => {
		const mailbox = projectChildMailbox([
			admitted,
			{
				type: "child/message-dispatch-started",
				seq: 1,
				committedAt: "t1",
				data: { messageId: "message-mail", attemptId: "attempt-crash", provider: "acp" },
			},
		]);
		const item = mailbox.get("message-mail")!;
		assert.equal(item.state, "dispatching");
		assert.equal(interruptedChildDispatches(mailbox).length, 1);
		assert.equal(childMessageIsAutoRetryable(item), false);
	});

	it("returns to retryable queued state only after a proven pre-effect rejection", () => {
		const mailbox = projectChildMailbox([
			admitted,
			{
				type: "child/message-dispatch-started",
				seq: 1,
				committedAt: "t1",
				data: { messageId: "message-mail", attemptId: "attempt-reject", provider: "acp" },
			},
			{
				type: "child/message-dispatch-rejected",
				seq: 2,
				committedAt: "t2",
				data: { messageId: "message-mail", attemptId: "attempt-reject", provider: "acp", code: "connection_not_opened" },
			},
		]);
		const item = mailbox.get("message-mail")!;
		assert.equal(item.state, "queued");
		assert.equal(childMessageIsAutoRetryable(item), true);
	});

	it("never marks an uncertain attempt auto-retryable", () => {
		const mailbox = projectChildMailbox([
			admitted,
			{
				type: "child/message-dispatch-started",
				seq: 1,
				committedAt: "t1",
				data: { messageId: "message-mail", attemptId: "attempt-uncertain", provider: "acp" },
			},
			{
				type: "child/message-dispatch-uncertain",
				seq: 2,
				committedAt: "t2",
				data: { messageId: "message-mail", attemptId: "attempt-uncertain", provider: "acp", code: "transport_effect_unknown" },
			},
		]);
		const item = mailbox.get("message-mail")!;
		assert.equal(item.state, "uncertain");
		assert.equal(childMessageIsAutoRetryable(item), false);
	});
});

describe("DSH-DONOR: bounded workflow IR", () => {
	const valid: WorkflowDefinition = {
		id: "review-pipeline",
		name: "review-pipeline",
		description: "Implement in parallel, then review in sequence.",
		root: {
			kind: "sequence",
			steps: [
				{
					kind: "parallel",
					branches: [
						{ kind: "delegate", role: "implementer", promptDigest: "sha256:a" },
						{ kind: "delegate", role: "researcher", promptDigest: "sha256:b" },
					],
				},
				{ kind: "phase", title: "independent-review", body: { kind: "delegate", role: "reviewer", promptDigest: "sha256:c" } },
			],
		},
	};

	it("counts bounded orchestration without executing arbitrary model-written code", () => {
		assert.deepEqual(validateWorkflow(valid), { nodes: 6, delegates: 3, maxDepth: 3, maxParallelWidth: 2 });
	});

	it("rejects resource explosions before an executor can start children", () => {
		assert.throws(() => validateWorkflow(valid, { maxDepth: 8, maxDelegates: 2, maxParallelWidth: 8, maxNodes: 128 }), WorkflowValidationError);
		assert.throws(() => validateWorkflow(valid, { maxDepth: 8, maxDelegates: 32, maxParallelWidth: 1, maxNodes: 128 }), WorkflowValidationError);
	});
});

describe("DSH-DONOR: sandbox is a capability report, not a magic secure bit", () => {
	it("accepts stronger boundaries and refuses partial/ambient boundaries when the caller requires more", () => {
		assert.doesNotThrow(() =>
			assertSandboxSatisfies(
				{ backend: "microvm", filesystem: "full", network: "isolated", process: "isolated", credentials: "none" },
				{ filesystem: "full", network: "restricted", process: "restricted", credentials: "brokered" },
			),
		);
		assert.throws(
			() =>
				assertSandboxSatisfies(
					{ backend: "seatbelt-files-only", filesystem: "full", network: "host", process: "host", credentials: "ambient" },
					{ filesystem: "full", network: "restricted", credentials: "brokered" },
				),
			SandboxRequirementError,
		);
	});
});

describe("DSH-DONOR: component admission records model/token/cache impact", () => {
	it("requires explicit token and cache effects even for a model-invisible component", () => {
		assert.doesNotThrow(() =>
			validateComponentExperience({
				component: "durable-child-mailbox",
				authority: "lifecycle",
				modelSurface: "none",
				tokenEffect: "No direct prompt tokens; only bounded child-result references may later be rendered.",
				cacheEffect: "No request-prefix change by itself.",
				executable: false,
			}),
		);
		assert.throws(
			() =>
				validateComponentExperience({
					component: "bad-component",
					authority: "advisory",
					modelSurface: "dynamic",
					tokenEffect: "",
					cacheEffect: "unknown",
					executable: false,
				}),
			ComponentExperienceError,
		);
	});

	it("does not allow a declared external-effect authority to pretend it is non-executable", () => {
		assert.throws(
			() =>
				validateComponentExperience({
					component: "external-writer",
					authority: "external-effect",
					modelSurface: "none",
					tokenEffect: "No model-visible content.",
					cacheEffect: "No request-prefix change.",
					executable: false,
				}),
			ComponentExperienceError,
		);
	});
});
