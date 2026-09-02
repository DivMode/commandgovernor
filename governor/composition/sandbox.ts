/*
 * DSH donor pattern: sandboxing is a replaceable capability and enforcement
 * completeness is a reported fact. Governor extends the contract beyond DSH's
 * filesystem-only SandboxMode so network/process/credential authority cannot
 * be accidentally inferred from a file sandbox.
 */

export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";
export type Enforcement = "full" | "partial";
export type NetworkBoundary = "isolated" | "restricted" | "host";
export type ProcessBoundary = "isolated" | "restricted" | "host";
export type CredentialBoundary = "none" | "brokered" | "ambient";

export interface SandboxPolicy {
	readonly mode: SandboxMode;
	readonly workspaceRoot: string;
	readonly sessionId?: string;
}

export interface SandboxReport {
	readonly filesystem: Enforcement;
	readonly network: NetworkBoundary;
	readonly process: ProcessBoundary;
	readonly credentials: CredentialBoundary;
	readonly backend: string;
}

export interface ConfinedExecution {
	readonly argv: readonly string[];
	readonly report: SandboxReport;
}

export interface SandboxProvider {
	readonly name: string;
	confine(argv: readonly string[], policy: ExcludeDangerousSandboxPolicy): Promise<ConfinedExecution> | ConfinedExecution;
}

export interface ExcludeDangerousSandboxPolicy extends SandboxPolicy {
	readonly mode: Exclude<SandboxMode, "danger-full-access">;
}

export interface SandboxRequirement {
	readonly filesystem?: Enforcement;
	readonly network?: Exclude<NetworkBoundary, "host">;
	readonly process?: Exclude<ProcessBoundary, "host">;
	readonly credentials?: Exclude<CredentialBoundary, "ambient">;
}

export class SandboxRequirementError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "SandboxRequirementError";
	}
}

function enforcementRank(value: Enforcement): number {
	return value === "full" ? 1 : 0;
}

function isolationRank(value: "isolated" | "restricted" | "host"): number {
	if (value === "isolated") return 2;
	if (value === "restricted") return 1;
	return 0;
}

function credentialRank(value: CredentialBoundary): number {
	if (value === "none") return 2;
	if (value === "brokered") return 1;
	return 0;
}

/** Fail closed when a backend's observed boundary is weaker than requested. */
export function assertSandboxSatisfies(report: SandboxReport, requirement: SandboxRequirement): void {
	if (requirement.filesystem !== undefined && enforcementRank(report.filesystem) < enforcementRank(requirement.filesystem)) {
		throw new SandboxRequirementError(`sandbox ${report.backend} filesystem enforcement is ${report.filesystem}, need ${requirement.filesystem}`);
	}
	if (requirement.network !== undefined && isolationRank(report.network) < isolationRank(requirement.network)) {
		throw new SandboxRequirementError(`sandbox ${report.backend} network boundary is ${report.network}, need ${requirement.network}`);
	}
	if (requirement.process !== undefined && isolationRank(report.process) < isolationRank(requirement.process)) {
		throw new SandboxRequirementError(`sandbox ${report.backend} process boundary is ${report.process}, need ${requirement.process}`);
	}
	if (requirement.credentials !== undefined && credentialRank(report.credentials) < credentialRank(requirement.credentials)) {
		throw new SandboxRequirementError(`sandbox ${report.backend} credential boundary is ${report.credentials}, need ${requirement.credentials}`);
	}
}
