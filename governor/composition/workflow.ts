/*
 * DeepSeek Harness proves the value of model-authored orchestration programs.
 * Governor adopts the orchestration shape but starts with a bounded declarative
 * IR instead of arbitrary JavaScript. A code-backed executor can be benchmarked
 * later without changing the durable workflow contract.
 */

export interface WorkflowDelegateNode {
	readonly kind: "delegate";
	readonly role: string;
	readonly promptDigest: string;
	readonly provider?: string;
	readonly resultKey?: string;
}

export interface WorkflowSequenceNode {
	readonly kind: "sequence";
	readonly steps: readonly WorkflowNode[];
}

export interface WorkflowParallelNode {
	readonly kind: "parallel";
	readonly branches: readonly WorkflowNode[];
}

export interface WorkflowPipelineNode {
	readonly kind: "pipeline";
	readonly stages: readonly WorkflowNode[];
}

export interface WorkflowPhaseNode {
	readonly kind: "phase";
	readonly title: string;
	readonly body: WorkflowNode;
}

export type WorkflowNode =
	| WorkflowDelegateNode
	| WorkflowSequenceNode
	| WorkflowParallelNode
	| WorkflowPipelineNode
	| WorkflowPhaseNode;

export interface WorkflowDefinition {
	readonly id: string;
	readonly name: string;
	readonly description: string;
	readonly root: WorkflowNode;
}

export interface WorkflowLimits {
	readonly maxDepth: number;
	readonly maxDelegates: number;
	readonly maxParallelWidth: number;
	readonly maxNodes: number;
}

export const DEFAULT_WORKFLOW_LIMITS: WorkflowLimits = {
	maxDepth: 8,
	maxDelegates: 32,
	maxParallelWidth: 8,
	maxNodes: 128,
};

export interface WorkflowStats {
	readonly nodes: number;
	readonly delegates: number;
	readonly maxDepth: number;
	readonly maxParallelWidth: number;
}

export class WorkflowValidationError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "WorkflowValidationError";
	}
}

function nonEmpty(value: string, field: string): void {
	if (value.length === 0 || value.trim() !== value) throw new WorkflowValidationError(`${field} must be non-empty and trimmed`);
}

export function validateWorkflow(
	definition: WorkflowDefinition,
	limits: WorkflowLimits = DEFAULT_WORKFLOW_LIMITS,
): WorkflowStats {
	nonEmpty(definition.id, "workflow id");
	nonEmpty(definition.name, "workflow name");
	nonEmpty(definition.description, "workflow description");
	for (const [name, value] of Object.entries(limits)) {
		if (!Number.isSafeInteger(value) || value < 1) throw new WorkflowValidationError(`${name} must be a positive safe integer`);
	}

	let nodes = 0;
	let delegates = 0;
	let observedDepth = 0;
	let observedParallelWidth = 0;

	const visit = (node: WorkflowNode, depth: number): void => {
		nodes += 1;
		observedDepth = Math.max(observedDepth, depth);
		if (nodes > limits.maxNodes) throw new WorkflowValidationError(`workflow exceeds maxNodes=${limits.maxNodes}`);
		if (depth > limits.maxDepth) throw new WorkflowValidationError(`workflow exceeds maxDepth=${limits.maxDepth}`);

		switch (node.kind) {
			case "delegate":
				delegates += 1;
				nonEmpty(node.role, "delegate role");
				nonEmpty(node.promptDigest, "delegate promptDigest");
				if (delegates > limits.maxDelegates) throw new WorkflowValidationError(`workflow exceeds maxDelegates=${limits.maxDelegates}`);
				return;
			case "phase":
				nonEmpty(node.title, "phase title");
				visit(node.body, depth + 1);
				return;
			case "parallel":
				if (node.branches.length === 0) throw new WorkflowValidationError("parallel requires at least one branch");
				observedParallelWidth = Math.max(observedParallelWidth, node.branches.length);
				if (node.branches.length > limits.maxParallelWidth) {
					throw new WorkflowValidationError(`parallel exceeds maxParallelWidth=${limits.maxParallelWidth}`);
				}
				for (const child of node.branches) visit(child, depth + 1);
				return;
			case "sequence":
				if (node.steps.length === 0) throw new WorkflowValidationError("sequence requires at least one step");
				for (const child of node.steps) visit(child, depth + 1);
				return;
			case "pipeline":
				if (node.stages.length === 0) throw new WorkflowValidationError("pipeline requires at least one stage");
				for (const child of node.stages) visit(child, depth + 1);
				return;
		}
	};

	visit(definition.root, 1);
	return { nodes, delegates, maxDepth: observedDepth, maxParallelWidth: observedParallelWidth };
}

export type WorkflowOutcome = "completed" | "failed" | "cancelled" | "uncertain";

export interface WorkflowExecutionResult {
	readonly outcome: WorkflowOutcome;
	readonly outputDigest?: string;
	readonly errorCode?: string;
}

export interface WorkflowExecutor {
	execute(definition: WorkflowDefinition, signal: AbortSignal): Promise<WorkflowExecutionResult>;
}
