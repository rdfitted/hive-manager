import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  calculateCriticalPathRemaining,
  calculateWaveStatistics,
  classifyHeartbeatStaleness,
  formatElapsedTime,
  getNodeAdjacency,
  selectActiveWave,
  truncateLabel,
} from './graphUtils';

describe('workgraph graphUtils', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-08-16T18:00:00.000Z'));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('truncateLabel constrains a fifty-character identifier without splitting Unicode', () => {
    const identifier = '1234567890'.repeat(5);

    expect(identifier).toHaveLength(50);
    expect(truncateLabel(identifier)).toBe('12345678…');
    expect(truncateLabel('123456789')).toBe('123456789');
    expect(truncateLabel('🐝'.repeat(10), 4)).toBe('🐝🐝🐝…');
    expect(truncateLabel('long', 1)).toBe('…');
    expect(truncateLabel('long', 0)).toBe('');
  });

  it('calculateWaveStatistics reports unique node and completed-wave counts', () => {
    const waves = [['T1'], ['T2', 'T3'], ['T3', 'T4']] as const;

    expect(calculateWaveStatistics(waves, new Set(['T1', 'T2', 'T3']))).toEqual({
      nodesComplete: 3,
      nodesObserved: 0,
      nodesInferred: 0,
      nodesTotal: 4,
      wavesComplete: 2,
      wavesTotal: 3,
    });
    expect(calculateWaveStatistics(waves, new Set(['T1', 'T2', 'T3', 'T4']))).toEqual({
      nodesComplete: 4,
      nodesObserved: 0,
      nodesInferred: 0,
      nodesTotal: 4,
      wavesComplete: 3,
      wavesTotal: 3,
    });
    expect(calculateWaveStatistics([], new Set())).toEqual({
      nodesComplete: 0,
      nodesObserved: 0,
      nodesInferred: 0,
      nodesTotal: 0,
      wavesComplete: 0,
      wavesTotal: 0,
    });
  });

  it('calculateWaveStatistics separates direct evidence from lane inference', () => {
    const waves = [['declared', 'queue'], ['observed', 'inferred', 'plan']] as const;

    expect(
      calculateWaveStatistics(
        waves,
        new Set(['declared', 'queue', 'observed', 'inferred', 'plan']),
        {
          declared: 'declared',
          queue: 'queue',
          observed: 'observed',
          inferred: 'inferred',
          plan: 'plan',
        },
      ),
    ).toEqual({
      nodesComplete: 5,
      nodesObserved: 3,
      nodesInferred: 1,
      nodesTotal: 5,
      wavesComplete: 2,
      wavesTotal: 2,
    });
  });

  it('calculateCriticalPathRemaining counts unfinished work in a partially complete graph', () => {
    const criticalPath = ['T1', 'T2', 'T4', 'T4'] as const;

    expect(calculateCriticalPathRemaining(criticalPath, new Set(['T1']))).toBe(2);
    expect(calculateCriticalPathRemaining(criticalPath, new Set(['T1', 'T2', 'T4']))).toBe(0);
    expect(calculateCriticalPathRemaining([], new Set())).toBe(0);
  });

  it('selectActiveWave finds a mid-graph running node after earlier work completes', () => {
    const waves = [['T1'], ['T2', 'T3'], ['T4']] as const;
    const completedWhileT2Runs = new Set(['T1', 'T3']);

    expect(selectActiveWave(waves, completedWhileT2Runs)).toBe(1);
    expect(selectActiveWave(waves, new Set(['T1', 'T2', 'T3']))).toBe(2);
    expect(selectActiveWave(waves, new Set(['T1', 'T2', 'T3', 'T4']))).toBeNull();
    expect(selectActiveWave([], new Set())).toBeNull();
  });

  it('getNodeAdjacency returns stable, unique immediate dependencies and dependents', () => {
    const edges = [
      { source: 'T1', target: 'T2' },
      { source: 'T3', target: 'T2' },
      { source: 'T1', target: 'T2' },
      { source: 'T2', target: 'T4' },
      { source: 'T2', target: 'T5' },
      { source: 'T2', target: 'T2' },
      { source: 'unrelated', target: 'other' },
    ];

    expect(getNodeAdjacency('T2', edges)).toEqual({
      dependencies: ['T1', 'T3'],
      dependents: ['T4', 'T5'],
    });
    expect(getNodeAdjacency('missing', edges)).toEqual({ dependencies: [], dependents: [] });
  });

  it('formatElapsedTime ticks locally, freezes on completion, and handles invalid timing', () => {
    const startedAt = '2026-08-16T17:59:01.000Z';

    expect(formatElapsedTime(startedAt)).toBe('59s');
    vi.advanceTimersByTime(1000);
    expect(formatElapsedTime(startedAt)).toBe('1m 00s');
    expect(
      formatElapsedTime(
        '2026-08-16T16:57:56.000Z',
        '2026-08-16T18:00:00.000Z',
      ),
    ).toBe('1h 02m 04s');
    vi.advanceTimersByTime(60_000);
    expect(
      formatElapsedTime(
        '2026-08-16T16:57:56.000Z',
        '2026-08-16T18:00:00.000Z',
      ),
    ).toBe('1h 02m 04s');
    expect(formatElapsedTime('not-a-timestamp')).toBe('—');
    expect(formatElapsedTime('2026-08-16T19:00:00.000Z')).toBe('0s');
  });

  it('classifyHeartbeatStaleness distinguishes unknown, fresh, and stale timestamps', () => {
    const staleAfterMs = 3 * 60 * 1000;

    expect(
      classifyHeartbeatStaleness('2026-08-16T17:57:00.000Z', staleAfterMs),
    ).toBe('fresh');
    vi.advanceTimersByTime(1);
    expect(
      classifyHeartbeatStaleness('2026-08-16T17:57:00.000Z', staleAfterMs),
    ).toBe('stale');
    expect(classifyHeartbeatStaleness(null, staleAfterMs)).toBe('unknown');
    expect(classifyHeartbeatStaleness('not-a-timestamp', staleAfterMs)).toBe('unknown');
    expect(
      classifyHeartbeatStaleness('2026-08-16T19:00:00.000Z', staleAfterMs),
    ).toBe('fresh');
  });
});
