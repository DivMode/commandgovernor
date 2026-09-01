//! The fake browser boundary.
//!
//! # What makes it evidence
//!
//! `docs/testing.md` DEL-003 and DEL-005 are not assertions a test writes after
//! the fact; they are properties of the *adapter boundary*. So this fake looks
//! for itself, through its own read-only connection, and **panics** rather than
//! acting:
//!
//! - every method requires the store to already show a live attempt —
//!   `claimed` or `activation_armed` — for the delivery it was handed;
//! - [`FakeBrowser::send`] additionally requires `activation_armed`.
//!
//! Reading through a second connection is what makes "the store shows" mean
//! *committed*, not merely written inside a transaction the writer still holds.
//! A daemon that reordered the claim transaction after navigation, or armed the
//! fence after Send, would take the panic on the first cell of the matrix.
//!
//! Every physical submission is recorded, so "zero further sends" and "at most
//! one submitted message for one revision" are counts rather than inferences.
//!
//! # What it is not
//!
//! It is not evidence about Chrome. Live conformance is Gate B in
//! `docs/testing.md`, and nothing in this crate may be read as standing in for
//! it.

use governor_core::delivery::{DeliveryId, WeakBrowserSignal};
use governor_core::fence::AttemptNo;
use governor_core::foreman_turn::ProviderMessageRef;
use governor_core::id::ObligationId;
use governor_core::outbound::{AmbiguityReason, FailureClass};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _};
use std::path::Path;

use crate::scenario::token;

/// One physical submission the fake actually performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalSend {
    /// Hex form of the wake's random correlation ID.
    pub delivery: String,
    /// Attempt that submitted.
    pub attempt: AttemptNo,
    /// The wake text that went into the composer.
    pub payload: String,
}

/// What the simulated page does when Send is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SendBehaviour {
    /// The message is submitted and exact evidence comes back.
    #[default]
    Submit,
    /// The activation call is refused synchronously, before any submission.
    RefuseActivation,
    /// The observation channel is lost while the submit is in flight.
    LoseObservation,
    /// Something happened, but only a weak UI signal was observed.
    WeakSignalOnly(WeakBrowserSignal),
}

/// The page the fake is looking at.
#[derive(Debug, Clone)]
pub struct BrowserWorld {
    /// Canonical conversation the binding names.
    pub bound_conversation: String,
    /// Canonical conversation actually resolved right now.
    pub resolved_conversation: String,
    /// Whether the Command Governor app is selected for this exact message.
    pub app_selected: bool,
    /// Whether the composer can be staged.
    pub composer_ready: bool,
    /// Whether the delivery target resolves at all.
    pub target_present: bool,
    /// What Send will do.
    pub send_behaviour: SendBehaviour,
}

impl BrowserWorld {
    /// A page in the state a successful wake needs.
    #[must_use]
    pub fn healthy(conversation: &str) -> Self {
        Self {
            bound_conversation: conversation.to_owned(),
            resolved_conversation: conversation.to_owned(),
            app_selected: true,
            composer_ready: true,
            target_present: true,
            send_behaviour: SendBehaviour::Submit,
        }
    }

    /// The proven pre-submit failure this page would report while staging.
    #[must_use]
    pub fn staging_failure(&self) -> Option<FailureClass> {
        if !self.target_present {
            return Some(FailureClass::TargetNotFound);
        }
        if self.resolved_conversation != self.bound_conversation {
            return Some(FailureClass::WrongConversation);
        }
        if !self.app_selected {
            return Some(FailureClass::AppNotSelected);
        }
        if !self.composer_ready {
            return Some(FailureClass::ComposerNotReady);
        }
        None
    }
}

/// What the fake observed after invoking Send.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SendOutcome {
    /// Exact conversation and provider user-message identity.
    Accepted(ProviderMessageRef),
    /// The activation itself was refused; nothing was submitted.
    RefusedBeforeSubmit(FailureClass),
    /// A submission may have happened and the outcome was lost.
    Lost(AmbiguityReason),
    /// Only a signal that can never prove acceptance was observed.
    WeakOnly(WeakBrowserSignal),
}

impl SendOutcome {
    /// Reports whether a physical message may have been submitted.
    #[must_use]
    pub const fn may_have_submitted(&self) -> bool {
        !matches!(self, Self::RefusedBeforeSubmit(_))
    }
}

/// A browser that refuses to act before the store says it may.
#[derive(Debug)]
pub struct FakeBrowser {
    conn: Connection,
    world: BrowserWorld,
    sends: Vec<PhysicalSend>,
    calls: Vec<&'static str>,
    next_message: u32,
}

impl FakeBrowser {
    /// Attaches to a state root's database and a simulated page.
    ///
    /// # Panics
    ///
    /// Panics when the database cannot be opened for reading.
    #[must_use]
    pub fn attach(database: &Path, world: BrowserWorld) -> Self {
        Self {
            conn: Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("the fake browser's own read-only connection"),
            world,
            sends: Vec::new(),
            calls: Vec::new(),
            next_message: 0,
        }
    }

    /// The page the fake is looking at.
    #[must_use]
    pub const fn world(&self) -> &BrowserWorld {
        &self.world
    }

    /// Changes the page under the fake, as a displacement would.
    pub fn displace(&mut self, world: BrowserWorld) {
        self.world = world;
    }

    /// Every physical submission, in order.
    #[must_use]
    pub fn sends(&self) -> &[PhysicalSend] {
        &self.sends
    }

    /// How many physical submissions exist for one wake revision.
    #[must_use]
    pub fn sends_for(&self, delivery: &DeliveryId) -> usize {
        let hex = delivery.expose_hex();
        self.sends
            .iter()
            .filter(|send| send.delivery == hex)
            .count()
    }

    /// Every browser method that was invoked, in order.
    #[must_use]
    pub fn calls(&self) -> &[&'static str] {
        &self.calls
    }

    /// Asserts the browser was never touched at all.
    ///
    /// # Panics
    ///
    /// Panics naming the calls that did happen.
    pub fn assert_untouched(&self, context: &str) {
        assert!(
            self.calls.is_empty(),
            "{context}: the browser was reached: {:?}",
            self.calls
        );
    }

    /// Navigates to the bound surface.
    ///
    /// # Panics
    ///
    /// Panics when the store does not already show a live attempt: invariant
    /// 10, `claimed` is durable before *any* browser I/O.
    pub fn navigate(&mut self, delivery: &DeliveryId, attempt: AttemptNo) {
        self.require_live("navigate", delivery, attempt);
    }

    /// Selects the Command Governor app for this exact message.
    ///
    /// # Panics
    ///
    /// As [`FakeBrowser::navigate`].
    pub fn select_app(&mut self, delivery: &DeliveryId, attempt: AttemptNo) {
        self.require_live("select_app", delivery, attempt);
    }

    /// Stages the wake text in the composer, without submitting it.
    ///
    /// # Errors
    ///
    /// Returns the proven pre-submit [`FailureClass`] this page would report.
    ///
    /// # Panics
    ///
    /// As [`FakeBrowser::navigate`].
    pub fn stage_composer(
        &mut self,
        delivery: &DeliveryId,
        attempt: AttemptNo,
    ) -> Result<(), FailureClass> {
        self.require_live("stage_composer", delivery, attempt);
        self.world.staging_failure().map_or(Ok(()), Err)
    }

    /// Invokes the exact Send activation.
    ///
    /// # Panics
    ///
    /// Panics unless the store already shows this attempt `activation_armed`:
    /// invariant 11, the ambiguity fence is durable *before* the exact Send.
    pub fn send(
        &mut self,
        delivery: &DeliveryId,
        attempt: AttemptNo,
        obligation: ObligationId,
    ) -> SendOutcome {
        self.calls.push("send");
        let state = self.attempt_state(delivery, attempt);
        assert_eq!(
            state.as_deref(),
            Some("activation_armed"),
            "DEL-005: Send was invoked while the store showed {state:?}; the \
             ambiguity fence must be durable before the exact submit"
        );

        let outcome = match self.world.send_behaviour {
            SendBehaviour::RefuseActivation => {
                return SendOutcome::RefusedBeforeSubmit(FailureClass::ActivationRefused);
            }
            SendBehaviour::Submit => {
                self.next_message += 1;
                SendOutcome::Accepted(ProviderMessageRef::new(token(&format!(
                    "msg-{}",
                    self.next_message
                ))))
            }
            SendBehaviour::LoseObservation => SendOutcome::Lost(AmbiguityReason::ObservationLost),
            SendBehaviour::WeakSignalOnly(signal) => SendOutcome::WeakOnly(signal),
        };

        // Everything but a synchronous refusal may have put a message on the
        // wire, so it is recorded as a physical send regardless of what the
        // adapter was able to observe.
        self.sends.push(PhysicalSend {
            delivery: delivery.expose_hex(),
            attempt,
            payload: WakePayload::render(obligation, delivery).into_text(),
        });
        outcome
    }

    fn require_live(&mut self, call: &'static str, delivery: &DeliveryId, attempt: AttemptNo) {
        self.calls.push(call);
        let state = self.attempt_state(delivery, attempt);
        assert!(
            matches!(state.as_deref(), Some("claimed" | "activation_armed")),
            "DEL-003: `{call}` was invoked while the store showed {state:?}; \
             an attempt must be durably claimed before any browser I/O"
        );
    }

    fn attempt_state(&self, delivery: &DeliveryId, attempt: AttemptNo) -> Option<String> {
        self.conn
            .query_row(
                "SELECT state FROM delivery_attempts
                  WHERE delivery_id = ?1 AND attempt_no = ?2",
                rusqlite::params![delivery.expose_hex(), i64::from(attempt.get())],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .expect("reading the attempt state")
    }
}

/// The wake text one revision carries.
///
/// The shape is fixed by `docs/browser-transport.md`, "Delivery identity and
/// wake payload": a protocol marker, the opaque obligation ID, the random
/// correlation ID, and a static instruction. Nothing else — no task, project,
/// worker, result or prompt content — has anywhere to go, because this
/// constructor takes nothing else.
///
/// This is the *test* renderer. The production one belongs to the browser
/// adapter, which Phase 1 does not build; SEC-005 is therefore proven against
/// the documented protocol shape rather than against a live composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePayload(String);

impl WakePayload {
    /// Renders the wake for one obligation and correlation ID.
    #[must_use]
    pub fn render(obligation: ObligationId, delivery: &DeliveryId) -> Self {
        Self(format!(
            "[command-governor wake v1] obligation={obligation} delivery={}. \
             Use the Command Governor app now. Resume this obligation, reconcile \
             the owning worker/result, perform the required review/action, then \
             ACK only after processing.",
            delivery.expose_hex()
        ))
    }

    /// The rendered text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the payload for its text.
    #[must_use]
    pub fn into_text(self) -> String {
        self.0
    }
}
