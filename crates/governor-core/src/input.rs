//! Durable input requests and the defer boundary that creates them.
//!
//! `needs_input` is only durable when the provider is *actually* in a safely
//! resumable blocked state. Per [`docs/state-machines.md`] §3 and
//! [`docs/adr/0005-worker-lifecycle-and-result-durability.md`], current Claude
//! semantics ignore a non-interactive `defer` when several tool calls are
//! emitted together, so a multi-tool shape must become
//! `worker_defer_shape_unsupported` attention and never a clean pause.
//!
//! That rule is carried by [`ConfirmedDefer`], which has no public constructor:
//! the only way to obtain one is [`evaluate_defer_boundary`] returning
//! [`DeferBoundary::Durable`], and that requires a single-tool shape, an
//! accepted defer response, **and** structured proof from the managed run.
//!
//! Nothing here stores the question, the options, or the tool arguments: an
//! input request records *what kind of input is owed*, in opaque tokens,
//! enums and counts.
//!
//! [`docs/state-machines.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/state-machines.md
//! [`docs/adr/0005-worker-lifecycle-and-result-durability.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/adr/0005-worker-lifecycle-and-result-durability.md

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{RequestRevision, SafeToken, SourceRef};
use crate::health::HealthConditionKind;
use crate::id::{InputRequestId, ObligationId, TurnId};
use crate::time::Timestamp;
use crate::worker_evidence::ConfirmedDeferredRun;

/// Opaque provider-native identity of the exact deferred tool call.
///
/// For current Claude this is a `tool_use_id`. It is never a transcript path
/// and never serialised arguments.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NativeInputRef(SafeToken);

impl NativeInputRef {
    /// Wraps an opaque provider tool-call identity.
    #[must_use]
    pub const fn new(id: SafeToken) -> Self {
        Self(id)
    }

    /// Returns the opaque identity.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// The shape of the tool batch the provider was processing when it deferred.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeferShape {
    /// Exactly one tool call, with an exact provider identity to resume.
    SingleTool {
        /// Provider-native identity of the single deferred call.
        tool_use: NativeInputRef,
    },
    /// Several tool calls emitted together. Current `defer` is ignored here.
    MultipleTools {
        /// How many calls were in the batch. Bounded diagnostic only.
        count: u32,
    },
}

/// What the provider did with the hook's `defer` decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DeferResponse {
    /// The provider accepted the defer decision.
    Accepted,
    /// The provider ignored it.
    Ignored,
    /// The response could not be parsed.
    Malformed,
}

/// Proof that an exact tool call is durably deferred and resumable.
///
/// No public constructor. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedDefer {
    tool_use: NativeInputRef,
    run_ref: SafeToken,
}

impl ConfirmedDefer {
    /// Provider-native identity of the deferred call.
    #[must_use]
    pub const fn tool_use(&self) -> &NativeInputRef {
        &self.tool_use
    }

    /// Opaque identity of the managed run that proved the defer.
    #[must_use]
    pub const fn run_ref(&self) -> &SafeToken {
        &self.run_ref
    }
}

/// Verdict on whether a defer produced a durable pause.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeferBoundary {
    /// A confirmed single-tool defer. Only this may create `needs_input`.
    Durable(ConfirmedDefer),
    /// A shape the provider cannot durably pause. Attention, not a pause.
    Unsupported(HealthConditionKind),
    /// The defer was attempted but not proven. Attention, not a pause.
    Unconfirmed(HealthConditionKind),
}

/// Decides whether an attempted defer became a durable resumable pause.
///
/// All three conditions are required and none substitutes for another:
/// a single-tool shape, an accepted response, and structured proof from the
/// managed run that execution actually stopped with the call pending.
#[must_use]
pub fn evaluate_defer_boundary(
    shape: &DeferShape,
    response: DeferResponse,
    run_proof: Option<&ConfirmedDeferredRun>,
) -> DeferBoundary {
    let DeferShape::SingleTool { tool_use } = shape else {
        // Current documented semantics ignore `defer` for a multi-tool batch,
        // so there is no resumable identity to record.
        return DeferBoundary::Unsupported(HealthConditionKind::WorkerDeferShapeUnsupported);
    };
    match (response, run_proof) {
        (DeferResponse::Accepted, Some(proof)) => DeferBoundary::Durable(ConfirmedDefer {
            tool_use: tool_use.clone(),
            run_ref: proof.run_ref().clone(),
        }),
        _ => DeferBoundary::Unconfirmed(HealthConditionKind::RuntimeStateConflict),
    }
}

/// What class of input is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum InputRequestKind {
    /// A technical question the foreman can answer within delegated authority.
    EngineeringQuestion,
    /// A decision the user owns and the foreman may not make.
    UserOwnedDecision,
    /// Runtime-level input required to continue.
    RuntimeInput,
    /// A provider elicitation whose exact resumability has been proven.
    ProviderElicitation,
}

/// Who is allowed to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AuthorizationClass {
    /// Within recorded delegation: the foreman may answer.
    DelegatedEngineering,
    /// Outside delegation: destructive, credential-sensitive, broader, unknown.
    UserOwned,
}

impl AuthorizationClass {
    /// Reports whether the foreman may answer without a user grant.
    #[must_use]
    pub const fn foreman_may_answer(self) -> bool {
        matches!(self, Self::DelegatedEngineering)
    }
}

/// The shape an answer must take. Carries no option text, only counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AnswerShape {
    /// One of `options` provider-offered choices.
    SingleChoice {
        /// How many choices the provider offered.
        options: u8,
    },
    /// A yes/no decision.
    Boolean,
    /// An opaque provider-defined token, such as an option identifier.
    OpaqueToken,
}

/// The recorded answer.
///
/// Deliberately structural. Free-form prose has no variant here, which is what
/// keeps a foreman answer from becoming a durable transcript. A protocol that
/// genuinely needs richer answers must extend this type deliberately rather
/// than smuggle text through an existing field.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Answer {
    /// The zero-based index of the chosen option.
    Choice {
        /// Index into the provider's option list.
        index: u8,
    },
    /// A yes/no decision.
    Boolean {
        /// The decision.
        value: bool,
    },
    /// An opaque provider-defined selection token.
    OpaqueToken {
        /// The selection token.
        token: SafeToken,
    },
    /// The answerer declined; the request stays owed to someone else.
    Declined,
}

impl Answer {
    /// Reports whether this answer satisfies the declared shape.
    #[must_use]
    pub const fn fits(&self, shape: AnswerShape) -> bool {
        match (self, shape) {
            (Self::Choice { index }, AnswerShape::SingleChoice { options }) => *index < options,
            (Self::Boolean { .. }, AnswerShape::Boolean)
            | (Self::OpaqueToken { .. }, AnswerShape::OpaqueToken)
            | (Self::Declined, _) => true,
            _ => false,
        }
    }
}

/// Lifecycle of one durable input request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputRequestState {
    /// Input is owed and no answer has been recorded.
    Pending,
    /// An answer is recorded. The worker has *not* necessarily received it.
    Answered,
    /// Matching resumed-turn evidence proved the worker took the answer.
    Resolved,
    /// The request was cancelled or superseded without an answer.
    Cancelled,
}

impl InputRequestState {
    /// Reports whether the request still owes somebody an answer.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Answered)
    }
}

/// Everything an input request records at the moment it is opened.
///
/// Grouped into one value so the durable shape is visible in one place, and so
/// adding a field is a deliberate change to a named type rather than another
/// positional argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRequestSpec {
    /// Obligation the request belongs to.
    pub obligation: ObligationId,
    /// Turn the request belongs to.
    pub turn: TurnId,
    /// Source fact that created the request.
    pub source: SourceRef,
    /// Proof the defer actually took effect and is resumable.
    pub defer: ConfirmedDefer,
    /// What class of input is owed.
    pub kind: InputRequestKind,
    /// Who may answer.
    pub authorization: AuthorizationClass,
    /// The shape an answer must take.
    pub answer_shape: AnswerShape,
    /// Revision of this request for the turn and source event.
    pub revision: RequestRevision,
}

/// One durable input request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRequest {
    id: InputRequestId,
    obligation: ObligationId,
    turn: TurnId,
    source: SourceRef,
    defer: ConfirmedDefer,
    kind: InputRequestKind,
    authorization: AuthorizationClass,
    answer_shape: AnswerShape,
    revision: RequestRevision,
    state: InputRequestState,
    answer: Option<Answer>,
    answered_at: Option<Timestamp>,
}

impl InputRequest {
    /// Opens a request from a *confirmed* durable defer boundary.
    ///
    /// Requiring [`ConfirmedDefer`] by value is the point: there is no way to
    /// call this from a multi-tool or unconfirmed defer.
    #[must_use]
    pub fn open(id: InputRequestId, spec: InputRequestSpec) -> Self {
        Self {
            id,
            obligation: spec.obligation,
            turn: spec.turn,
            source: spec.source,
            defer: spec.defer,
            kind: spec.kind,
            authorization: spec.authorization,
            answer_shape: spec.answer_shape,
            revision: spec.revision,
            state: InputRequestState::Pending,
            answer: None,
            answered_at: None,
        }
    }

    /// Request identity.
    #[must_use]
    pub const fn id(&self) -> InputRequestId {
        self.id
    }

    /// Obligation this request belongs to.
    #[must_use]
    pub const fn obligation(&self) -> ObligationId {
        self.obligation
    }

    /// Turn this request belongs to.
    #[must_use]
    pub const fn turn(&self) -> TurnId {
        self.turn
    }

    /// Source fact that created the request.
    #[must_use]
    pub const fn source(&self) -> &SourceRef {
        &self.source
    }

    /// The confirmed defer boundary that made this request resumable.
    #[must_use]
    pub const fn defer(&self) -> &ConfirmedDefer {
        &self.defer
    }

    /// What class of input is owed.
    #[must_use]
    pub const fn kind(&self) -> InputRequestKind {
        self.kind
    }

    /// Who may answer.
    #[must_use]
    pub const fn authorization(&self) -> AuthorizationClass {
        self.authorization
    }

    /// The shape an answer must take.
    #[must_use]
    pub const fn answer_shape(&self) -> AnswerShape {
        self.answer_shape
    }

    /// Revision of this request for the turn and source event.
    #[must_use]
    pub const fn revision(&self) -> RequestRevision {
        self.revision
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> InputRequestState {
        self.state
    }

    /// The recorded answer, if one exists.
    #[must_use]
    pub const fn answer(&self) -> Option<&Answer> {
        self.answer.as_ref()
    }

    /// Applies an event, returning a new request or a typed conflict.
    ///
    /// # Errors
    ///
    /// - [`Conflict::InvalidDisposition`] when an answer does not fit the shape;
    /// - [`Conflict::ConflictingInputAnswer`] for a differing second answer;
    /// - [`Conflict::IllegalInputTransition`] otherwise.
    pub fn apply(&self, event: &InputRequestEvent) -> Outcome<Self> {
        match event {
            InputRequestEvent::Answered { answer, at } => self.record_answer(answer, *at),
            InputRequestEvent::ResumedTurnConfirmed => match self.state {
                InputRequestState::Answered => {
                    let mut next = self.clone();
                    next.state = InputRequestState::Resolved;
                    Ok(Transition::Advanced(next))
                }
                InputRequestState::Resolved => Ok(Transition::Duplicate),
                from => Err(Conflict::IllegalInputTransition {
                    from,
                    event: "resumed_turn_confirmed",
                }),
            },
            InputRequestEvent::Cancelled => match self.state {
                InputRequestState::Pending | InputRequestState::Answered => {
                    let mut next = self.clone();
                    next.state = InputRequestState::Cancelled;
                    Ok(Transition::Advanced(next))
                }
                InputRequestState::Cancelled => Ok(Transition::Duplicate),
                from => Err(Conflict::IllegalInputTransition {
                    from,
                    event: "cancelled",
                }),
            },
        }
    }

    fn record_answer(&self, answer: &Answer, at: Timestamp) -> Outcome<Self> {
        match self.state {
            InputRequestState::Pending => {
                if !answer.fits(self.answer_shape) {
                    return Err(Conflict::InvalidDisposition);
                }
                let mut next = self.clone();
                next.state = InputRequestState::Answered;
                next.answer = Some(answer.clone());
                next.answered_at = Some(at);
                Ok(Transition::Advanced(next))
            }
            InputRequestState::Answered if self.answer.as_ref() == Some(answer) => {
                Ok(Transition::Duplicate)
            }
            // A second, different answer never reaches the worker: the first
            // answer is immutable and one continuation is already outstanding.
            InputRequestState::Answered => Err(Conflict::ConflictingInputAnswer),
            from => Err(Conflict::IllegalInputTransition {
                from,
                event: "answered",
            }),
        }
    }
}

/// An event applied to an input request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputRequestEvent {
    /// An authorised answerer recorded a structured answer.
    ///
    /// Recording is not delivery: the worker has its own delivery projection.
    Answered {
        /// The structured answer.
        answer: Answer,
        /// Observation instant.
        at: Timestamp,
    },
    /// Matching resumed-turn evidence proved the worker consumed the answer.
    ResumedTurnConfirmed,
    /// The request was cancelled or superseded.
    Cancelled,
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{
        AnswerShape, AuthorizationClass, ConfirmedDefer, DeferBoundary, DeferResponse, DeferShape,
        InputRequest, InputRequestKind, InputRequestSpec, NativeInputRef, evaluate_defer_boundary,
    };
    use crate::fence::{RequestRevision, SafeToken};
    use crate::id::{InputRequestId, ObligationId, TurnId};
    use crate::obligation::test_support as obligation_support;
    use crate::worker_evidence::test_support as evidence_support;
    use uuid::Uuid;

    pub(crate) fn confirmed_defer() -> ConfirmedDefer {
        let shape = DeferShape::SingleTool {
            tool_use: NativeInputRef::new(SafeToken::new("toolu_01ABC").unwrap()),
        };
        match evaluate_defer_boundary(
            &shape,
            DeferResponse::Accepted,
            Some(&evidence_support::confirmed_deferred()),
        ) {
            DeferBoundary::Durable(defer) => defer,
            other => panic!("expected a durable boundary, got {other:?}"),
        }
    }

    pub(crate) fn pending_request() -> InputRequest {
        InputRequest::open(
            InputRequestId::from_uuid(Uuid::from_u128(31)),
            InputRequestSpec {
                obligation: ObligationId::from_uuid(Uuid::from_u128(11)),
                turn: TurnId::from_uuid(Uuid::from_u128(21)),
                source: obligation_support::defer_source(),
                defer: confirmed_defer(),
                kind: InputRequestKind::EngineeringQuestion,
                authorization: AuthorizationClass::DelegatedEngineering,
                answer_shape: AnswerShape::SingleChoice { options: 3 },
                revision: RequestRevision::FIRST,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::pending_request;
    use super::*;
    use crate::worker_evidence::test_support as evidence_support;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn single_tool() -> DeferShape {
        DeferShape::SingleTool {
            tool_use: NativeInputRef::new(SafeToken::new("toolu_01ABC").unwrap()),
        }
    }

    #[test]
    fn confirmed_single_tool_defer_is_durable() {
        let boundary = evaluate_defer_boundary(
            &single_tool(),
            DeferResponse::Accepted,
            Some(&evidence_support::confirmed_deferred()),
        );
        match boundary {
            DeferBoundary::Durable(defer) => {
                assert_eq!(defer.tool_use().as_token().as_str(), "toolu_01ABC");
            }
            other => panic!("expected durable, got {other:?}"),
        }
    }

    #[test]
    fn multi_tool_defer_can_never_become_needs_input() {
        for response in [
            DeferResponse::Accepted,
            DeferResponse::Ignored,
            DeferResponse::Malformed,
        ] {
            let boundary = evaluate_defer_boundary(
                &DeferShape::MultipleTools { count: 2 },
                response,
                Some(&evidence_support::confirmed_deferred()),
            );
            assert_eq!(
                boundary,
                DeferBoundary::Unsupported(HealthConditionKind::WorkerDeferShapeUnsupported),
                "a multi-tool batch is attention, never a clean pause"
            );
        }
    }

    #[test]
    fn defer_intent_without_structured_proof_is_not_a_pause() {
        assert_eq!(
            evaluate_defer_boundary(&single_tool(), DeferResponse::Accepted, None),
            DeferBoundary::Unconfirmed(HealthConditionKind::RuntimeStateConflict)
        );
        assert_eq!(
            evaluate_defer_boundary(
                &single_tool(),
                DeferResponse::Ignored,
                Some(&evidence_support::confirmed_deferred())
            ),
            DeferBoundary::Unconfirmed(HealthConditionKind::RuntimeStateConflict)
        );
        assert_eq!(
            evaluate_defer_boundary(
                &single_tool(),
                DeferResponse::Malformed,
                Some(&evidence_support::confirmed_deferred())
            ),
            DeferBoundary::Unconfirmed(HealthConditionKind::RuntimeStateConflict)
        );
    }

    #[test]
    fn an_answer_must_fit_the_declared_shape() {
        let request = pending_request();
        let err = request
            .apply(&InputRequestEvent::Answered {
                answer: Answer::Choice { index: 9 },
                at: at(1),
            })
            .unwrap_err();
        assert_eq!(err.code(), "invalid_disposition");
        assert_eq!(request.state(), InputRequestState::Pending);
    }

    #[test]
    fn recording_an_answer_does_not_resolve_the_request() {
        let answered = pending_request()
            .apply(&InputRequestEvent::Answered {
                answer: Answer::Choice { index: 1 },
                at: at(1),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(
            answered.state(),
            InputRequestState::Answered,
            "answer recorded is not answer received"
        );
        assert!(answered.state().is_open());
    }

    #[test]
    fn only_resumed_turn_evidence_resolves_the_request() {
        let answered = pending_request()
            .apply(&InputRequestEvent::Answered {
                answer: Answer::Choice { index: 1 },
                at: at(1),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let resolved = answered
            .apply(&InputRequestEvent::ResumedTurnConfirmed)
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(resolved.state(), InputRequestState::Resolved);

        // The same evidence against a pending request is illegal.
        let err = pending_request()
            .apply(&InputRequestEvent::ResumedTurnConfirmed)
            .unwrap_err();
        assert_eq!(err.code(), "illegal_input_transition");
    }

    #[test]
    fn a_conflicting_second_answer_is_rejected() {
        let answered = pending_request()
            .apply(&InputRequestEvent::Answered {
                answer: Answer::Choice { index: 1 },
                at: at(1),
            })
            .unwrap()
            .advanced()
            .unwrap();

        let repeat = answered
            .apply(&InputRequestEvent::Answered {
                answer: Answer::Choice { index: 1 },
                at: at(2),
            })
            .unwrap();
        assert!(repeat.is_duplicate(), "an identical answer is idempotent");

        let err = answered
            .apply(&InputRequestEvent::Answered {
                answer: Answer::Choice { index: 2 },
                at: at(3),
            })
            .unwrap_err();
        assert_eq!(err.code(), "conflicting_input_answer");
        assert_eq!(
            answered.answer(),
            Some(&Answer::Choice { index: 1 }),
            "zero mutation"
        );
    }

    #[test]
    fn user_owned_requests_are_not_foreman_answerable() {
        assert!(!AuthorizationClass::UserOwned.foreman_may_answer());
        assert!(AuthorizationClass::DelegatedEngineering.foreman_may_answer());
    }
}
