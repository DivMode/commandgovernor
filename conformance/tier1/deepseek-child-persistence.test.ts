import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
	dispatchAcceptedChildMessage,
	type AcceptedChildMessage,
	type ChildMessageTransport,
} from "../../governor/composition/child.ts";
import type { DurableEventSink, GovernorEventDraft } from "../../governor/composition/events.ts";
import { governorMessageId, governorSessionId } from "../../governor/composition/lifecycle.ts";

describe("DSH-DONOR: persistence failure after child send", () => {
	it("never asks the transport classifier to reinterpret a post-send storage failure as pre-effect", async () => {
		const committed: GovernorEventDraft[] = [];
		let appendCalls = 0;
		let classifierCalls = 0;
		const sink: DurableEventSink = {
			append: async (event) => {
				appendCalls += 1;
				if (appendCalls === 2) throw new Error("simulated durable-store failure");
				committed.push(event);
				return { seq: committed.length - 1, committedAt: `t-${committed.length}` };
			},
		};
		const message: AcceptedChildMessage = {
			taskId: "task-persist",
			parentSessionId: governorSessionId("parent-persist"),
			childSessionId: governorSessionId("child-persist"),
			messageId: governorMessageId("message-persist"),
			contentDigest: "sha256:persist",
			durable: { seq: 7, committedAt: "t-7" },
		};
		const transport: ChildMessageTransport = {
			provider: "acp",
			send: async () => ({ providerReceipt: "wire-returned" }),
			classifyFailure: () => {
				classifierCalls += 1;
				return { effect: "not-started", code: "would-be-dangerous-misclassification" };
			},
		};

		await assert.rejects(
			dispatchAcceptedChildMessage(sink, transport, message, "attempt-persist", new AbortController().signal),
			/simulated durable-store failure/,
		);
		assert.equal(classifierCalls, 0, "storage failures after send are outside the transport classifier's authority");
		assert.deepEqual(
			committed.map((event) => event.type),
			["child/message-dispatch-started"],
			"recovery sees only durable dispatch intent and therefore treats the effect as ambiguous",
		);
	});
});
