/**
 * cg-foreman — the foreman transport interface.
 *
 * TYPES ONLY. There is no implementation in this file and none anywhere else
 * yet; the transport itself is Gate P4 and the durable store behind it is
 * Phase B. What this file fixes now are the decisions that become expensive to
 * change once rows exist: the envelope shape, the delivery-id encoding, and the
 * fact that an ambiguous send is a result rather than an exception.
 *
 * Two candidate transports were evaluated (see
 * docs/research/2026-09-01-chatgpt-transport-review.md). Neither satisfies the
 * gate alone, and their failure modes are opposite: one has message identity
 * and no durability, the other has durability and only positional correlation.
 * The interface below is deliberately the intersection they can both implement
 * — bind, send, read-since — so that neither one's shape leaks into the ledger.
 *
 * Where durable state lives
 * -------------------------
 * The foreman ledger is an **extension-owned durable sidecar**, not the Pi
 * session JSONL. Pi's `pi.appendEntry` is session-scoped: it does not survive
 * as a single authority across `session_before_fork` or
 * `session_before_switch`, and Pi documents no background processes for
 * extensions. Use `appendEntry` as an in-session mirror for visibility only.
 * The sidecar is the authority. It is not implemented here.
 *
 * The reply leg must be pollable from a fresh process
 * ---------------------------------------------------
 * Phase A must not assume send-and-reply within one Pi turn. One candidate is
 * synchronous-in-turn, the other is a detached worker with a best-effort
 * wake-up, and Pi forbids background processes inside an extension. So
 * `readSince` is defined against a durable cursor and takes a `Binding` it can
 * reconstruct from stored data — never a live handle.
 */

// ---------------------------------------------------------------------------
// Delivery identity
// ---------------------------------------------------------------------------

/**
 * Every delivery id MUST contain at least one ASCII letter, and MUST NOT
 * contain a run of ten or more characters drawn only from digits, spaces,
 * parentheses and hyphens.
 *
 * This is not style. One candidate transport runs a redaction pass over text it
 * reads back, and that pass replaces any sufficiently long digit/space/paren/
 * hyphen run with a `<PHONE>` placeholder. A purely numeric delivery id is
 * therefore *destroyed on readback*, which silently breaks the one correlation
 * primitive the whole protocol rests on. The constraint is recorded here
 * because every stored ledger row and every sent envelope carries the id, so it
 * cannot be revisited cheaply later.
 *
 * The recommended encoding is `CG-D-` followed by Crockford base32 of at least
 * 128 bits of entropy, which guarantees letters in practice — but the checker,
 * not the convention, is what the conformance suite asserts.
 */
export const DELIVERY_ID_PREFIX = "CG-D-";

/** The redaction window that motivates {@link DELIVERY_ID_PREFIX}. */
export const REDACTION_HAZARD_RUN_LENGTH = 10;

export type DeliveryId = string & { readonly __brand: "DeliveryId" };

/** A task identity. Stable across revisions of the same task. */
export type TaskId = string & { readonly __brand: "TaskId" };

/**
 * A monotonically increasing revision of a task. A reply naming a revision
 * older than the current one is stale and MUST be recorded as rejected rather
 * than silently dropped, so that "the foreman answered and we ignored it"
 * stays auditable.
 */
export type TaskRevision = number & { readonly __brand: "TaskRevision" };

// ---------------------------------------------------------------------------
// Binding
// ---------------------------------------------------------------------------

/**
 * A reference to the foreman conversation, as the user supplies it: either a
 * full `https://chatgpt.com/c/<id>` URL or the bare id. Normalisation to the
 * exact conversation id is the transport's job, and the caller MUST assert the
 * returned id equals the requested one — both candidate transports have a
 * silent fallback that returns a *different* conversation without erroring.
 */
export interface ConversationRef {
	readonly raw: string;
}

/**
 * A generation-fenced binding to exactly one foreman conversation.
 *
 * `generation` exists so that a rebind invalidates in-flight work bound to the
 * previous generation. ADR 0004's "exactly one active binding" singleton is
 * deliberately NOT assumed here: a Pi harness may address several
 * conversations, so the fence is per-binding and the singleton question is left
 * open rather than inherited.
 *
 * Every field is plain data. A binding must be reconstructible from the durable
 * sidecar in a process that has never spoken to the transport.
 */
export interface Binding {
	readonly conversationId: string;
	readonly generation: number;
	readonly boundAt: string;
	readonly transport: TransportId;
}

export type TransportId = string & { readonly __brand: "TransportId" };

// ---------------------------------------------------------------------------
// Envelopes
// ---------------------------------------------------------------------------

export type ForemanEventKind =
	| "work_completed"
	| "work_blocked"
	| "review_requested"
	| "question";

/**
 * What Command Governor sends to the foreman.
 *
 * Serialization constraint, carried from the transport research: the rendered
 * form must be **scrape- and truncation-tolerant**. One candidate reads replies
 * from an accessibility snapshot rather than the source message; the other
 * truncates readback at 4000 characters. So the delivery id and the event kind
 * must appear early, each on its own line, and must survive whitespace
 * normalisation — they must not be buried inside JSON that a scraper may
 * mangle. `payload` is the bounded part that may be lossy; the header fields
 * are the part that may not.
 */
export interface ForemanEvent {
	readonly taskId: TaskId;
	readonly taskRevision: TaskRevision;
	readonly deliveryId: DeliveryId;
	readonly eventKind: ForemanEventKind;
	/** Bounded result or reference payload. Never authority for lifecycle. */
	readonly payload: string;
}

export type ForemanActionKind = "ACK" | "REVISE" | "DELEGATE" | "ASK_USER";

/**
 * What the foreman sends back.
 *
 * `prose` is the free-text half. A ChatGPT Web foreman replies in prose by
 * construction, so this field will exist whether or not the schema wants it —
 * and it is an open decision, flagged in
 * docs/pi-native/migration-notes.md, that its bounded size, classification,
 * retention and redaction contract must be settled **before** it is
 * implemented. It is typed here as optional and bounded-by-contract so nobody
 * implements it by accident.
 */
export interface ForemanAction {
	readonly taskId: TaskId;
	readonly taskRevision: TaskRevision;
	readonly deliveryId: DeliveryId;
	readonly action: ForemanActionKind;
	readonly prose?: string;
}

// ---------------------------------------------------------------------------
// Send results
// ---------------------------------------------------------------------------

/** The send was observed to land, with whatever identity the transport has. */
export interface Receipt {
	readonly kind: "receipt";
	readonly deliveryId: DeliveryId;
	/**
	 * Provider-side message identity, when the transport can supply one. A
	 * transport that cannot forces every send into {@link Ambiguous}, which is
	 * correct but has architectural consequences — it is an open question, not
	 * a detail.
	 */
	readonly messageId?: string;
	readonly observedAt: string;
}

/**
 * The send may or may not have landed, and the transport cannot tell.
 *
 * This is a first-class result, never an exception, because both candidate
 * transports can produce it and because the correct response is reconciliation
 * — read the thread back and look for the delivery id — not a retry. A
 * transport that models this as a thrown error invites the caller to treat it
 * as a failure and replay blindly, which is precisely the behaviour the
 * reliability contract forbids.
 */
export interface Ambiguous {
	readonly kind: "ambiguous";
	readonly deliveryId: DeliveryId;
	/** Why the outcome is unknown. Recorded, never used to infer an outcome. */
	readonly reason: string;
	readonly observedAt: string;
}

export type SendResult = Receipt | Ambiguous;

// ---------------------------------------------------------------------------
// Reading back
// ---------------------------------------------------------------------------

/**
 * A durable read position in a bound conversation. Plain data, so a fresh
 * process can resume reading without having sent anything.
 */
export interface ReadCursor {
	readonly conversationId: string;
	readonly generation: number;
	readonly lastMessageId?: string;
	readonly lastObservedAt?: string;
}

export interface ForemanReply {
	readonly messageId?: string;
	readonly createdAt?: string;
	/** Raw text as read. Parsing into a {@link ForemanAction} is a separate step. */
	readonly text: string;
}

export interface ReadSinceResult {
	readonly replies: readonly ForemanReply[];
	readonly cursor: ReadCursor;
}

// ---------------------------------------------------------------------------
// The transport
// ---------------------------------------------------------------------------

/**
 * The three operations both candidate transports can implement, and nothing
 * else. No transport-specific shape may leak through this interface into the
 * ledger.
 */
export interface ForemanTransport {
	readonly id: TransportId;

	/**
	 * Resolve a conversation reference to an exact, generation-fenced binding.
	 * Implementations MUST fail rather than silently binding a different
	 * conversation than the one requested.
	 */
	bind(ref: ConversationRef): Promise<Binding>;

	/**
	 * Send one event. Returns {@link Ambiguous} rather than throwing when the
	 * outcome is unknown.
	 *
	 * The caller's contract, which no transport can enforce: the
	 * `SEND_ATTEMPTED` ledger row must be durable **before** this call returns
	 * or throws. Otherwise reconciliation is impossible and every restart is a
	 * blind replay.
	 */
	send(binding: Binding, event: ForemanEvent): Promise<SendResult>;

	/**
	 * Read replies after a cursor. Must work from a fresh process that has
	 * never called {@link send}.
	 */
	readSince(binding: Binding, cursor: ReadCursor): Promise<ReadSinceResult>;
}

// ---------------------------------------------------------------------------
// Ledger states (recorded here so the transport and the store agree later)
// ---------------------------------------------------------------------------

/**
 * The lifecycle of one delivery in the durable sidecar. Named now so that the
 * Phase B store and the Gate P4 transport cannot invent two vocabularies.
 */
export type DeliveryState =
	| "PREPARED"
	| "SEND_ATTEMPTED"
	| "SEND_OBSERVED"
	| "REPLY_READ"
	| "DISPOSITIONED";
