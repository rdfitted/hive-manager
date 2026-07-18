import { get } from 'svelte/store';
import { describe, expect, it } from 'vitest';

import { scratchTerminals, shellCommand } from './scratchTerminals';

describe('scratch terminal panes', () => {
  it('keeps pane and focus state isolated by session', () => {
    scratchTerminals.clearSession('session-a');
    scratchTerminals.clearSession('session-b');

    const paneA = scratchTerminals.add('session-a', 'C:\\repo-a', 'powershell');
    const paneB = scratchTerminals.add('session-b', 'C:\\repo-b', 'cmd');
    const state = get(scratchTerminals);

    expect(paneA.id).toMatch(/^scratch:session-a:/);
    expect(paneB.id).toMatch(/^scratch:session-b:/);
    expect(state.panesBySession['session-a']).toEqual([paneA]);
    expect(state.panesBySession['session-b']).toEqual([paneB]);
    expect(state.focusedBySession['session-a']).toBe(paneA.id);
    expect(state.focusedBySession['session-b']).toBe(paneB.id);
  });

  it('removes a closed pane without disturbing another session', () => {
    scratchTerminals.clearSession('session-a');
    scratchTerminals.clearSession('session-b');
    const paneA = scratchTerminals.add('session-a', 'C:\\repo-a', 'cmd');
    const paneB = scratchTerminals.add('session-b', 'C:\\repo-b', 'powershell');

    scratchTerminals.remove('session-a', paneA.id);
    const state = get(scratchTerminals);

    expect(state.panesBySession['session-a']).toEqual([]);
    expect(state.focusedBySession['session-a']).toBeNull();
    expect(state.panesBySession['session-b']).toEqual([paneB]);
  });

  it('maps the supported shell choices to explicit Windows executables', () => {
    expect(shellCommand('powershell')).toEqual({ command: 'powershell.exe', args: ['-NoLogo'] });
    expect(shellCommand('cmd')).toEqual({ command: 'cmd.exe', args: [] });
  });
});
