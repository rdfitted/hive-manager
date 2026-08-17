import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/svelte';
import ProgressHeader from './ProgressHeader.svelte';

afterEach(cleanup);

describe('ProgressHeader', () => {
  it('renders complete-graph counts with no critical path remaining', () => {
    render(ProgressHeader, {
      nodesComplete: 4,
      nodesTotal: 4,
      wavesComplete: 3,
      wavesTotal: 3,
      criticalPathRemaining: 0,
    });

    expect(screen.getByTestId('nodes-progress').textContent).toBe('4 / 4');
    expect(screen.getByTestId('waves-progress').textContent).toBe('3 / 3');
    expect(screen.getByTestId('critical-path-remaining').textContent).toBe('0');

    const header = screen.getByRole('banner', { name: 'Work graph progress' });
    expect(header.classList.contains('lattice-forced-colors-boundary')).toBe(true);
  });

  it('renders partial-graph counts and the remaining critical-path length', () => {
    render(ProgressHeader, {
      nodesComplete: 2,
      nodesTotal: 5,
      wavesComplete: 1,
      wavesTotal: 3,
      criticalPathRemaining: 2,
    });

    expect(screen.getByTestId('nodes-progress').textContent).toBe('2 / 5');
    expect(screen.getByTestId('waves-progress').textContent).toBe('1 / 3');
    expect(screen.getByTestId('critical-path-remaining').textContent).toBe('2');
  });
});
