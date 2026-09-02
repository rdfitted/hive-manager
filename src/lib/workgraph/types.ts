export type WorkGraphView = 'plan' | 'runtime' | 'divergence';
export type WorkGraphSource = 'live' | 'archive';
export type WorkGraphSourceSelector = 'auto' | WorkGraphSource;

export type NodeKind = 'task' | 'review' | 'join' | 'checkpoint' | 'context';

export type NodeStatus =
  | 'pending'
  | 'ready'
  | 'running'
  | 'completed'
  | 'failed'
  | 'blocked'
  | 'cancelled';

/** Evidence source for a terminal status, in backend precedence order. */
export type CompletionProvenance =
  | 'declared'
  | 'queue'
  | 'observed'
  | 'inferred'
  | 'plan';

export type BindingRef =
  | { kind: 'role'; value: string }
  | { kind: 'zone'; value: string };

export interface NodeContract {
  inputs: string[];
  outputs: string[];
  acceptance: string[];
}

export interface ContractSummary {
  input_count: number;
  output_count: number;
  acceptance_count: number;
}

export interface CompositeExpansion {
  template: string;
  parameters: Record<string, string>;
}

export interface WorkGraphNodeProgress {
  started_at: string | null;
  finished_at: string | null;
  attempts: number;
  agent_id: string | null;
  last_heartbeat_at: string | null;
}

export interface WorkGraphNode {
  id: string;
  title: string;
  kind: NodeKind;
  status: NodeStatus;
  lane: BindingRef;
  contract: NodeContract;
  contract_summary: ContractSummary;
  expansion: CompositeExpansion | null;
  /** Omitted on the plan view; runtime data may explicitly report no progress. */
  progress?: WorkGraphNodeProgress | null;
}

export type EdgeKind =
  | 'depends_on'
  | 'produces'
  | 'consumes'
  | 'reviews'
  | 'informs'
  | 'touches';

export type EdgeProvenance = 'planner' | 'codegraph' | 'knowledge' | 'runtime';

export interface WorkGraphEdge {
  source: string;
  target: string;
  kind: EdgeKind;
  provenance: EdgeProvenance;
}

export interface EdgeProvenanceResponse {
  source: string;
  target: string;
  kind: EdgeKind;
  provenance: EdgeProvenance;
}

export type DivergenceKind =
  | 'node_added'
  | 'node_removed'
  | 'node_restructured'
  | 'edge_added'
  | 'edge_removed'
  | 'edge_rewired';

export type GraphMutationType =
  | 'split'
  | 'merge'
  | 'reorder'
  | 'composite_expanded'
  | 'review_round_added'
  | 'review_verdict_recorded'
  | 'remediation_detour'
  | 'contradiction_adjudicated'
  | 'checkpoint_inserted'
  | 'other';

export interface DivergenceRecord {
  kind: DivergenceKind;
  node_id: string | null;
  source: string | null;
  target: string | null;
  replacement_source: string | null;
  replacement_target: string | null;
}

export interface DivergenceSummary {
  counts_by_mutation_type: Partial<Record<DivergenceKind, number>>;
  recorded_runtime_mutations: Partial<Record<GraphMutationType, number>>;
  records: DivergenceRecord[];
}

export type WorkGraphOmissionReason =
  | 'codegraph_unavailable'
  | 'project_knowledge_unavailable'
  | 'source_unreadable'
  | 'resolution_incomplete'
  | 'completion_unresolved';

export interface WorkGraphOmission {
  reason: WorkGraphOmissionReason;
  count: number;
  detail: string;
  examples: string[];
}

export interface WorkGraphResponse {
  view: WorkGraphView;
  source: WorkGraphSource;
  nodes: WorkGraphNode[];
  edges: WorkGraphEdge[];
  waves: string[][];
  status_by_node: Record<string, NodeStatus>;
  completion_provenance: Record<string, CompletionProvenance>;
  completion_source_refs: Record<string, string[]>;
  lane_assignment: Record<string, BindingRef>;
  agents_by_lane: Record<string, string[]>;
  critical_path: string[];
  provenance_by_edge: EdgeProvenanceResponse[];
  divergence: DivergenceSummary | null;
  /** Omitted by serde when no graph or projection omissions were recorded. */
  omissions?: WorkGraphOmission[];
}
