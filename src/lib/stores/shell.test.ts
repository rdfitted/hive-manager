import { describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import { createShellStore } from './shell';

describe('shell store', () => {
  it('issues strictly increasing ids for repeated start actions', () => {
    const shell = createShellStore();

    shell.requestStartAction('hive');
    const first = get(shell).startAction;
    shell.requestStartAction('hive');
    const second = get(shell).startAction;

    expect(first).toMatchObject({ action: 'hive' });
    expect(second).toMatchObject({ action: 'hive' });
    expect(second?.id).toBeGreaterThan(first?.id ?? Number.NEGATIVE_INFINITY);
  });

  it('opens and closes the Add Worker dialog', () => {
    const shell = createShellStore();

    shell.openAddWorker();
    expect(get(shell).addWorkerOpen).toBe(true);

    shell.closeAddWorker();
    expect(get(shell).addWorkerOpen).toBe(false);
  });
});
