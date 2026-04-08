/**
 * TypeScript mirrors of Rust domain types.
 * 
 * Serde enum normalization: Rust sends unit enums as "snake_case" strings
 * and object variants as { "variant_name": data }.
 */

export interface Session {
    id: string;
    name: string;
    objective: string;
    project_path: string;
    mode: SessionMode;
    status: SessionStatus;
    created_at: string;
    updated_at: string;
    cells: string[];
    launch_config: LaunchConfig;
    artifacts: ArtifactBundle[];
    events: string[];
}

export type SessionMode = 'hive' | 'fusion';

export interface LaunchConfig {
    plan_source?: string;
    default_cli: string;
    default_model?: string;
    worker_count: number;
    variant_count?: number;
    with_planning: boolean;
    with_evaluator: boolean;
    smoke_test: boolean;
}

export type SessionStatus =
    | 'drafting'
    | 'preparing'
    | 'launching'
    | 'active'
    | 'resolving'
    | 'completed'
    | 'partial_failure'
    | 'failed'
    | 'cancelled';

export interface Cell {
    id: string;
    session_id: string;
    cell_type: CellType;
    name: string;
    status: CellStatus;
    objective: string;
    workspace: Workspace;
    agents: string[];
    artifacts?: ArtifactBundle;
    events: string[];
    depends_on: string[];
}

export type CellType = 'hive' | 'resolver';

export type CellStatus =
    | 'queued'
    | 'preparing'
    | 'launching'
    | 'running'
    | 'summarizing'
    | 'completed'
    | 'waiting_input'
    | 'failed'
    | 'killed';

export interface Agent {
    id: string;
    cell_id: string;
    role: AgentRole;
    label: string;
    cli: string;
    model?: string;
    status: AgentStatus;
    process_ref?: string;
    terminal_ref?: string;
    last_event_at?: string;
}

export type AgentRole = 'queen' | 'worker' | 'resolver' | 'reviewer' | 'tester';

export type AgentStatus =
    | 'queued'
    | 'launching'
    | 'running'
    | 'completed'
    | 'waiting_input'
    | 'failed'
    | 'killed';

export interface Event {
    id: string;
    session_id: string;
    cell_id?: string;
    agent_id?: string;
    event_type: EventType;
    timestamp: string;
    payload: any;
    severity: Severity;
}

export type EventType =
    | 'session_created'
    | 'session_status_changed'
    | 'cell_created'
    | 'cell_status_changed'
    | 'workspace_created'
    | 'agent_launched'
    | 'agent_completed'
    | 'agent_waiting_input'
    | 'agent_failed'
    | 'artifact_updated'
    | 'resolver_selected_candidate';

export type Severity = 'info' | 'warning' | 'error';

export interface Workspace {
    strategy: WorkspaceStrategy;
    repo_path: string;
    base_branch: string;
    branch_name: string;
    worktree_path?: string;
    is_dirty: boolean;
}

export type WorkspaceStrategy = 'none' | 'shared_cell' | 'isolated_cell';

export interface ArtifactBundle {
    summary?: string;
    changed_files: string[];
    commits: string[];
    branch: string;
    test_results?: any;
    diff_summary?: string;
    unresolved_issues: string[];
    confidence?: number;
    recommended_next_step?: string;
}

export interface ResolverOutput {
    selected_candidate: string;
    rationale: string;
    tradeoffs: string[];
    hybrid_integration_plan?: string;
    final_recommendation?: string;
}

export interface SessionTemplate {
    id: string;
    name: string;
    description: string;
    mode: SessionMode;
    cells: CellTemplate[];
    workspace_strategy: WorkspaceStrategy;
    is_builtin: boolean;
}

export interface CellTemplate {
    role: string; // references AgentRole or custom role
    cli: string;
    model?: string;
    prompt_template: string; // references role template key
}

export interface RolePack {
    id: string;
    name: string;
    roles: CellTemplate[];
}

/**
 * Helper to extract the variant name from a Serde-serialized enum with data.
 * If input is a string, returns the string.
 * If input is an object { variant: data }, returns the key.
 */
export function serdeEnumVariantName(value: any): string {
    if (typeof value === 'string') return value;
    if (typeof value === 'object' && value !== null) {
        return Object.keys(value)[0];
    }
    return String(value);
}
