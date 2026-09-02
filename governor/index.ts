export { ClientIdentityMismatch, CommandMismatch, Governor, NotRecoverable, SessionIdentityMismatch } from "./governor.ts";
export type { ClientIdentityMismatchReason, CommandMismatchReason, CreateSessionSpec, CreatedSession, DispatchResult, GovernorOptions, RecoveryOutcome } from "./governor.ts";
export { DaemonClient, RequestTimeout, SubstrateMismatch, TransportLost, connectWithRetry } from "./prime/daemon-client.ts";
export { DEFAULT_LAUNCH_ENV_ALLOWLIST, buildLaunchEnv, launchEnvIsWithinAllowlist } from "./prime/env.ts";
export * from "./prime/protocol.ts";
export * from "./prime/substrate.ts";
export { ClientIdentityError, loadOrCreateClientIdentity, readClientIdentity } from "./prime/client-identity.ts";
export type { ClientIdentityRecord } from "./prime/client-identity.ts";
export { DEFAULT_POLICY, LEGACY_GLOBAL_CODE_POLICY, NAIVE_POLICY, assertProductionPolicy, classifyMutationOutcome } from "./mutation/classify.ts";
export type { ClassificationPolicy, Observation, PreEffectProof, UncertainReason, Verdict } from "./mutation/classify.ts";
export { PRE_EFFECT_PROOF_MATRIX, ProofMatrix, REVIEWED_PROOFS } from "./mutation/proof.ts";
export type { EffectTiming, ProofReview } from "./mutation/proof.ts";
export { MutationLedger, MutationLedgerError, WITHHELD_COMMAND_FIELDS } from "./mutation/ledger.ts";
export type { AdoptionReport, DispatcherIdentity, MutationLedgerOptions, MutationRecord, MutationState, ResolutionEvidence } from "./mutation/ledger.ts";
export { COMMAND_DIGEST_PATTERN, canonicalJson, commandDigest } from "./mutation/digest.ts";
export { ShortWrite, createFileExclusiveDurable, fsyncDirectory, mkdirDurable, unlinkDurable, writeAllSync, writeFileDurable } from "./fs/durable.ts";
export type { DurableFs } from "./fs/durable.ts";
export { classifyProcessIdentity, currentProcessIdentity, processStartId } from "./process/identity.ts";
export type { ProcessIdentity, ProcessIdentityVerdict, ProcessProbe } from "./process/identity.ts";
export { SessionPathError, canonicalSessionPath, isAcceptableSessionPath } from "./session/paths.ts";
export type { CanonicalSessionPath } from "./session/paths.ts";
export { RecoveryLeaseContended, RecoveryLeaseHeld, RecoveryReclaimBlocked, SessionRegistry, StaleCursorError, StaleIncarnationError, UnknownSessionError } from "./session/registry.ts";
export type { Incarnation, RecoveryLease, RecoveryLeaseRecord, SessionRecord } from "./session/registry.ts";

// Substrate-neutral composition contracts. These adapt useful DeepSeek Harness
// patterns without copying its runtime or weakening Prime's durability authority.
export * from "./composition/capabilities.ts";
export * from "./composition/child.ts";
export * from "./composition/component.ts";
export * from "./composition/events.ts";
export * from "./composition/lifecycle.ts";
export * from "./composition/mailbox.ts";
export * from "./composition/sandbox.ts";
export * from "./composition/workflow.ts";
