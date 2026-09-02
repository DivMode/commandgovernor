import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { GovernorEvent } from "../../governor/composition/events.ts";
import { ChildMailboxProjectionError, projectChildMailbox } from "../../governor/composition/mailbox.ts";

function admission(): GovernorEvent {
	return {
		type: "child/message-admitted",
		seq: 0,
		committedAt: "t0",
		data: {
			taskId: "task-1",
			parentSessionId: "parent-1",
			childSessionId: "child-1",
			messageId: "message-1",
			contentDigest: "sha256:1",
		},
	};
}

function started(seq: number, attemptId: string): GovernorEvent {
	return {
		type: "child/message-dispatch-started",
		seq,
		committedAt: `t${seq}`,
		data: { messageId: "message-1", attemptId, provider: "acp" },
	};
}

describe("DSH-DONOR: child-mailbox replay fencing", () => {
	it("rejects a second dispatch after an uncertain effect", () => {
		assert.throws(
			() =>
				projectChildMailbox([
					admission(),
					started(1, "attempt-1"),
					{
						type: "child/message-dispatch-uncertain",
						seq: 2,
						committedAt: "t2",
						data: { messageId: "message-1", attemptId: "attempt-1", provider: "acp", code: "effect_unknown" },
					},
					started(3, "attempt-2"),
				]),
			ChildMailboxProjectionError,
		);
	});

	it("rejects a second dispatch after transport success but before target delivery confirmation", () => {
		assert.throws(
			() =>
				projectChildMailbox([
					admission(),
					started(1, "attempt-1"),
					{
						type: "child/message-dispatched",
						seq: 2,
						committedAt: "t2",
						data: { messageId: "message-1", attemptId: "attempt-1", provider: "acp", providerReceipt: "wire-ok" },
					},
					started(3, "attempt-2"),
				]),
			ChildMailboxProjectionError,
		);
	});

	it("allows a new attempt after an exact pre-effect rejection", () => {
		assert.doesNotThrow(() =>
			projectChildMailbox([
				admission(),
				started(1, "attempt-1"),
				{
					type: "child/message-dispatch-rejected",
					seq: 2,
					committedAt: "t2",
					data: { messageId: "message-1", attemptId: "attempt-1", provider: "acp", code: "not_opened" },
				},
				started(3, "attempt-2"),
			]),
		);
	});
});
