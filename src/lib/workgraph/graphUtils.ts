export const DEFAULT_LABEL_MAX_CHARACTERS = 9;
export const DEFAULT_HEARTBEAT_STALE_AFTER_MS = 3 * 60 * 1000;

export type TimestampLike = string | number | Date | null | undefined;

export interface DirectedEdge {
  source: string;
  target: string;
}

export interface WaveStatistics {
  nodesComplete: number;
  nodesTotal: number;
  wavesComplete: number;
  wavesTotal: number;
}

export interface NodeAdjacency {
  /** Nodes that must complete before this node can run. */
  dependencies: string[];
  /** Nodes that directly depend on this node. */
  dependents: string[];
}

export type HeartbeatStaleness = 'unknown' | 'fresh' | 'stale';

/**
 * Shortens a label by Unicode code points so an ellipsis never splits a
 * surrogate pair. The default is sized for the work graph's 62px node box.
 */
export function truncateLabel(
  label: string,
  maxCharacters = DEFAULT_LABEL_MAX_CHARACTERS,
): string {
  if (maxCharacters <= 0) return '';

  const characters = Array.from(label);
  if (characters.length <= maxCharacters) return label;
  if (maxCharacters === 1) return '…';
  return `${characters.slice(0, maxCharacters - 1).join('')}…`;
}

/**
 * Counts unique graph nodes and treats a wave as complete only when it has at
 * least one node and every node in it is complete.
 */
export function calculateWaveStatistics(
  waves: readonly (readonly string[])[],
  completedNodeIds: ReadonlySet<string>,
): WaveStatistics {
  const nodeIds = new Set(waves.flatMap((wave) => wave));
  let nodesComplete = 0;
  for (const id of nodeIds) {
    if (completedNodeIds.has(id)) nodesComplete += 1;
  }

  const wavesComplete = waves.filter(
    (wave) => wave.length > 0 && wave.every((id) => completedNodeIds.has(id)),
  ).length;

  return {
    nodesComplete,
    nodesTotal: nodeIds.size,
    wavesComplete,
    wavesTotal: waves.length,
  };
}

/**
 * Counts the unfinished nodes on the graph's critical path. Duplicate IDs are
 * ignored so malformed path data cannot inflate the displayed remainder.
 */
export function calculateCriticalPathRemaining(
  criticalPath: readonly string[],
  completedNodeIds: ReadonlySet<string>,
): number {
  const remainingNodeIds = new Set(
    criticalPath.filter((id) => !completedNodeIds.has(id)),
  );
  return remainingNodeIds.size;
}

/**
 * Returns the earliest wave containing unfinished work, or null once the
 * graph is complete. The zero-based index is kept separate from presentation.
 */
export function selectActiveWave(
  waves: readonly (readonly string[])[],
  completedNodeIds: ReadonlySet<string>,
): number | null {
  const index = waves.findIndex(
    (wave) => wave.length > 0 && wave.some((id) => !completedNodeIds.has(id)),
  );
  return index < 0 ? null : index;
}

/**
 * Resolves both sides of a node's immediate directed adjacency. Work-graph
 * edges point from a prerequisite (`source`) to its dependent (`target`).
 */
export function getNodeAdjacency(
  nodeId: string,
  edges: readonly DirectedEdge[],
): NodeAdjacency {
  const dependencies = new Set<string>();
  const dependents = new Set<string>();

  for (const edge of edges) {
    if (edge.source === edge.target) continue;
    if (edge.target === nodeId) dependencies.add(edge.source);
    if (edge.source === nodeId) dependents.add(edge.target);
  }

  return {
    dependencies: [...dependencies],
    dependents: [...dependents],
  };
}

function timestampMilliseconds(value: TimestampLike): number | null {
  if (value === null || value === undefined || value === '') return null;
  if (value instanceof Date) {
    const timestamp = value.getTime();
    return Number.isFinite(timestamp) ? timestamp : null;
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) return null;
    // Rust timestamps may be seconds or milliseconds since the epoch.
    return value < 10_000_000_000 ? value * 1000 : value;
  }
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? timestamp : null;
}

/**
 * Formats a running or finished interval. Missing/invalid start times remain
 * explicitly unknown; future clocks clamp to zero rather than going negative.
 */
export function formatElapsedTime(
  startedAt: TimestampLike,
  finishedAt: TimestampLike = null,
  nowMs = Date.now(),
): string {
  const startedMs = timestampMilliseconds(startedAt);
  if (startedMs === null) return '—';

  const finishedMs = timestampMilliseconds(finishedAt);
  const elapsedSeconds = Math.floor(Math.max(0, (finishedMs ?? nowMs) - startedMs) / 1000);
  if (elapsedSeconds < 60) return `${elapsedSeconds}s`;

  const seconds = elapsedSeconds % 60;
  const elapsedMinutes = Math.floor(elapsedSeconds / 60);
  if (elapsedMinutes < 60) {
    return `${elapsedMinutes}m ${String(seconds).padStart(2, '0')}s`;
  }

  const hours = Math.floor(elapsedMinutes / 60);
  const minutes = elapsedMinutes % 60;
  return `${hours}h ${String(minutes).padStart(2, '0')}m ${String(seconds).padStart(2, '0')}s`;
}

/**
 * Classifies heartbeat freshness without coupling to a transport payload.
 * Absent or malformed timestamps are unknown, never falsely stale.
 */
export function classifyHeartbeatStaleness(
  heartbeatAt: TimestampLike,
  staleAfterMs = DEFAULT_HEARTBEAT_STALE_AFTER_MS,
  nowMs = Date.now(),
): HeartbeatStaleness {
  const heartbeatMs = timestampMilliseconds(heartbeatAt);
  if (heartbeatMs === null || !Number.isFinite(staleAfterMs) || staleAfterMs < 0) {
    return 'unknown';
  }

  const ageMs = Math.max(0, nowMs - heartbeatMs);
  return ageMs > staleAfterMs ? 'stale' : 'fresh';
}
