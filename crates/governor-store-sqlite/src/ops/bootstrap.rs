//! Registering the work structure, and committing a foreman binding.
//!
//! Neither is one of `docs/data-model.md`'s critical boundaries, but both are
//! prerequisites for them: an obligation has a task, a turn and an incarnation
//! before it can transition, and a wake has a binding generation before it can
//! be created. Both are transactions of the same shape as the rest.

use governor_core::binding::{
    BindingEvent, BrowserProfileRef, ConnectorAbi, ConversationRef, VerifiedBindingTarget,
    WriteCapabilityState,
};
use governor_core::fence::{
    BindingGeneration, EventSeq, IncarnationGeneration, SafeToken, TurnGeneration,
};
use governor_core::id::EventId;
use governor_core::id::{
    ForemanBindingId, ObligationId, ProjectId, SessionId, SessionIncarnationId, TaskId, TurnId,
};
use governor_core::obligation::ObligationKind;
use governor_core::time::Timestamp;
use rusqlite::params;

use crate::codec::{
    ActorClass, TurnLifecycle, encode_actor_class, encode_obligation_kind, encode_obligation_state,
    encode_turn_lifecycle, encode_write_capability, id_text, store_u64,
};
use crate::error::StoreResult;
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::ops::internal_source;
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// Source-host provenance for a project. Never repository content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSpec {
    /// Opaque source-host identity, such as `github.com`.
    pub source_host: SafeToken,
    /// Opaque host-native repository identity.
    pub source_repo_id: Option<SafeToken>,
    /// Opaque host-native repository display reference.
    pub source_repo_display: Option<SafeToken>,
}

/// The worker session a turn runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// Opaque runtime class, such as `herdr`.
    pub runtime_kind: SafeToken,
    /// Opaque worker class, such as `claude`.
    pub worker_kind: SafeToken,
    /// Display metadata. Never an identity fence.
    pub display_name: Option<SafeToken>,
    /// Opaque runtime instance reference for this incarnation.
    pub runtime_instance_ref: Option<SafeToken>,
    /// Opaque worker session reference for this incarnation.
    pub worker_session_ref: Option<SafeToken>,
}

/// Everything needed to open one delegated worker turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWorkerTurnRequest {
    /// Project provenance.
    pub project: ProjectSpec,
    /// Opaque source-host issue reference for the task.
    pub source_issue_ref: Option<SafeToken>,
    /// Session and incarnation description.
    pub session: SessionSpec,
    /// Opaque worker-native turn reference.
    pub worker_turn_ref: Option<SafeToken>,
    /// Scheduling priority. Ordering hint only, never a fence.
    pub priority: i64,
}

/// The identities one opened worker turn produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenedWorkerTurn {
    /// Registered project.
    pub project: ProjectId,
    /// Registered task.
    pub task: TaskId,
    /// Registered session.
    pub session: SessionId,
    /// Started session incarnation.
    pub incarnation: SessionIncarnationId,
    /// Started turn.
    pub turn: TurnId,
    /// Obligation created in `created`.
    pub obligation: ObligationId,
}

/// Registers a project, task, session, incarnation, turn and obligation.
pub(crate) struct OpenWorkerTurn {
    request: OpenWorkerTurnRequest,
    ids: OpenedWorkerTurn,
    events: [EventId; 6],
    now: Timestamp,
}

impl WriteOp for OpenWorkerTurn {
    type Request = OpenWorkerTurnRequest;
    type Committed = OpenedWorkerTurn;
    type Output = OpenedWorkerTurn;

    const NAME: &'static str = "open_worker_turn";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            ids: OpenedWorkerTurn {
                project: ports.next_id(),
                task: ports.next_id(),
                session: ports.next_id(),
                incarnation: ports.next_id(),
                turn: ports.next_id(),
                obligation: ports.next_id(),
            },
            events: [
                ports.next_id(),
                ports.next_id(),
                ports.next_id(),
                ports.next_id(),
                ports.next_id(),
                ports.next_id(),
            ],
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let ids = self.ids;
        let scope = EventScope {
            project: Some(ids.project),
            ..EventScope::default()
        };

        let project_seq = self.append(
            tx,
            0,
            EventKind::ProjectRegistered,
            scope.clone(),
            SafeMetadata::new(),
        )?;
        tx.conn().execute(
            "INSERT INTO projects (project_id, source_host, source_repo_id,
                                   source_repo_display, created_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id_text(ids.project),
                self.request.project.source_host.as_str(),
                self.request
                    .project
                    .source_repo_id
                    .as_ref()
                    .map(SafeToken::as_str),
                self.request
                    .project
                    .source_repo_display
                    .as_ref()
                    .map(SafeToken::as_str),
                event::store_seq(project_seq)?,
            ],
        )?;

        let task_seq = self.append(
            tx,
            1,
            EventKind::TaskRegistered,
            EventScope {
                task: Some(ids.task),
                ..scope.clone()
            },
            SafeMetadata::new(),
        )?;
        tx.conn().execute(
            "INSERT INTO tasks (task_id, project_id, source_issue_ref,
                                created_event_seq, latest_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![
                id_text(ids.task),
                id_text(ids.project),
                self.request
                    .source_issue_ref
                    .as_ref()
                    .map(SafeToken::as_str),
                event::store_seq(task_seq)?,
            ],
        )?;

        let session_seq = self.append(
            tx,
            2,
            EventKind::SessionRegistered,
            EventScope {
                session: Some(ids.session),
                ..scope.clone()
            },
            SafeMetadata::new(),
        )?;
        tx.conn().execute(
            "INSERT INTO sessions (session_id, project_id, runtime_kind, worker_kind,
                                   display_name, created_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id_text(ids.session),
                id_text(ids.project),
                self.request.session.runtime_kind.as_str(),
                self.request.session.worker_kind.as_str(),
                self.request
                    .session
                    .display_name
                    .as_ref()
                    .map(SafeToken::as_str),
                event::store_seq(session_seq)?,
            ],
        )?;

        let incarnation_seq = self.append(
            tx,
            3,
            EventKind::SessionIncarnationStarted,
            EventScope {
                session: Some(ids.session),
                incarnation: Some(ids.incarnation),
                ..scope.clone()
            },
            SafeMetadata::new().int("generation", 1),
        )?;
        tx.conn().execute(
            "INSERT INTO session_incarnations (session_incarnation_id, session_id, generation,
                                               runtime_instance_ref, worker_session_ref,
                                               started_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id_text(ids.incarnation),
                id_text(ids.session),
                store_u64(
                    IncarnationGeneration::FIRST.get(),
                    "session_incarnations",
                    "generation"
                )?,
                self.request
                    .session
                    .runtime_instance_ref
                    .as_ref()
                    .map(SafeToken::as_str),
                self.request
                    .session
                    .worker_session_ref
                    .as_ref()
                    .map(SafeToken::as_str),
                event::store_seq(incarnation_seq)?,
            ],
        )?;

        let turn_seq = self.append(
            tx,
            4,
            EventKind::TurnStarted,
            EventScope {
                task: Some(ids.task),
                session: Some(ids.session),
                incarnation: Some(ids.incarnation),
                turn: Some(ids.turn),
                ..scope.clone()
            },
            SafeMetadata::new().int("turn_generation", 1),
        )?;
        tx.conn().execute(
            "INSERT INTO turns (turn_id, task_id, session_incarnation_id, worker_turn_ref,
                                turn_generation, lifecycle_state, started_event_seq,
                                latest_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id_text(ids.turn),
                id_text(ids.task),
                id_text(ids.incarnation),
                self.request.worker_turn_ref.as_ref().map(SafeToken::as_str),
                store_u64(TurnGeneration::FIRST.get(), "turns", "turn_generation")?,
                encode_turn_lifecycle(TurnLifecycle::Running),
                event::store_seq(turn_seq)?,
            ],
        )?;

        let obligation_seq = self.append(
            tx,
            5,
            EventKind::ObligationCreated,
            EventScope {
                task: Some(ids.task),
                turn: Some(ids.turn),
                obligation: Some(ids.obligation),
                ..scope
            },
            SafeMetadata::new()
                .label(
                    "obligation_kind",
                    encode_obligation_kind(ObligationKind::WorkerTurn, "events")?,
                )
                .int("incarnation", 1),
        )?;
        tx.conn().execute(
            "INSERT INTO obligations (obligation_id, task_id, turn_id, obligation_kind, state,
                                      priority, created_event_seq, source_event_seq,
                                      current_version, incarnation_generation, latest_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1, 1, ?7)",
            params![
                id_text(ids.obligation),
                id_text(ids.task),
                id_text(ids.turn),
                encode_obligation_kind(ObligationKind::WorkerTurn, "obligations")?,
                encode_obligation_state(governor_core::obligation::ObligationState::Created),
                self.request.priority,
                event::store_seq(obligation_seq)?,
            ],
        )?;
        tx.conn().execute(
            "INSERT INTO obligation_events (obligation_id, obligation_version, event_seq,
                                            from_state, to_state, actor_class)
             VALUES (?1, 1, ?2, NULL, ?3, ?4)",
            params![
                id_text(ids.obligation),
                event::store_seq(obligation_seq)?,
                encode_obligation_state(governor_core::obligation::ObligationState::Created),
                encode_actor_class(ActorClass::Daemon),
            ],
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(ids)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

impl OpenWorkerTurn {
    fn append(
        &self,
        tx: &Tx<'_>,
        index: usize,
        kind: EventKind,
        scope: EventScope,
        metadata: SafeMetadata,
    ) -> StoreResult<EventSeq> {
        let subject = match kind {
            EventKind::ProjectRegistered => id_text(self.ids.project),
            EventKind::TaskRegistered => id_text(self.ids.task),
            EventKind::SessionRegistered => id_text(self.ids.session),
            EventKind::SessionIncarnationStarted => id_text(self.ids.incarnation),
            EventKind::TurnStarted => id_text(self.ids.turn),
            _ => id_text(self.ids.obligation),
        };
        let source = crate::ops::internal_source_text(&subject, kind.label())?;
        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.events[index],
                kind,
                source,
                observed_at: self.now,
                occurred_at: None,
                scope,
                metadata,
            },
        )?;
        Ok(appended.seq())
    }
}

// --- Foreman binding --------------------------------------------------------

/// A conversation the browser adapter has already verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindForemanRequest {
    /// Opaque provider label, such as `chatgpt`.
    pub provider: SafeToken,
    /// Exact resolved canonical conversation.
    pub conversation: ConversationRef,
    /// Opaque canonical conversation URL reference.
    pub conversation_url_ref: SafeToken,
    /// Opaque identity of the dedicated browser profile.
    ///
    /// A bare token rather than a [`BrowserProfileRef`]: `governor-core` exposes
    /// no accessor for the wrapper's contents, and the projection row needs the
    /// value. The wrapper is built from it here, so there is still exactly one
    /// source of truth for it.
    pub profile: SafeToken,
    /// Opaque connector ABI proven present on the surface.
    ///
    /// A bare token for the same reason as [`Self::profile`].
    pub connector_abi: SafeToken,
    /// Capability epoch observed during verification.
    pub capability_epoch: u64,
    /// Feature-tested write capability.
    pub write_capability: WriteCapabilityState,
}

/// The binding a commit produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundForeman {
    /// The new binding record.
    pub binding: ForemanBindingId,
    /// The generation it was issued at.
    pub generation: BindingGeneration,
}

/// Commits a verified binding, superseding every older generation.
pub(crate) struct BindForeman {
    request: BindForemanRequest,
    binding: ForemanBindingId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for BindForeman {
    type Request = BindForemanRequest;
    type Committed = BoundForeman;
    type Output = BoundForeman;

    const NAME: &'static str = "bind_foreman";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            binding: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let ledger = crate::load::bindings(tx)?;
        let next = ledger
            .apply(&BindingEvent::Bound {
                target: Box::new(VerifiedBindingTarget {
                    id: self.binding,
                    conversation: self.request.conversation.clone(),
                    profile: BrowserProfileRef::new(self.request.profile.clone()),
                    connector_abi: ConnectorAbi::new(self.request.connector_abi.clone()),
                    capability_epoch: self.request.capability_epoch,
                    write_capability: self.request.write_capability,
                }),
                at: self.now,
            })?
            .or_unchanged(ledger);
        let generation = next
            .active()
            .expect("a ledger that just bound has an active binding")
            .generation();

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ForemanBindingBound,
                source: internal_source(self.binding, "foreman_binding_bound")?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope::default(),
                metadata: SafeMetadata::new()
                    .int(
                        "generation",
                        store_u64(generation.get(), "events", "generation")?,
                    )
                    .int(
                        "capability_epoch",
                        store_u64(self.request.capability_epoch, "events", "capability_epoch")?,
                    )
                    .label(
                        "write_capability",
                        encode_write_capability(self.request.write_capability, "events")?,
                    ),
            },
        )?
        .seq();

        // The partial unique index permits exactly one active row, so the old
        // binding must be superseded in the same statement sequence, before the
        // new one is inserted.
        tx.conn().execute(
            "UPDATE foreman_bindings
                SET is_active = 0, superseded_event_seq = ?1
              WHERE is_active = 1",
            params![event::store_seq(seq)?],
        )?;
        tx.conn().execute(
            "INSERT INTO foreman_bindings (foreman_binding_id, provider,
                    canonical_conversation_id, canonical_conversation_url, browser_profile_id,
                    binding_generation, connector_abi, capability_epoch, write_capability_state,
                    is_active, bound_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10)",
            params![
                id_text(self.binding),
                self.request.provider.as_str(),
                self.request.conversation.as_token().as_str(),
                self.request.conversation_url_ref.as_str(),
                self.request.profile.as_str(),
                store_u64(generation.get(), "foreman_bindings", "binding_generation")?,
                self.request.connector_abi.as_str(),
                store_u64(
                    self.request.capability_epoch,
                    "foreman_bindings",
                    "capability_epoch"
                )?,
                encode_write_capability(self.request.write_capability, "foreman_bindings")?,
                event::store_seq(seq)?,
            ],
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(BoundForeman {
            binding: self.binding,
            generation,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}
