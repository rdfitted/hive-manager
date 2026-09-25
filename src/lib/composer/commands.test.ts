import { describe, it, expect, vi } from 'vitest';
import { filterCommands, findCommand, SLASH_COMMANDS } from './commands';

// sources.ts pulls in @tauri-apps/api/core (invoke) and the sessions store; mock the Tauri
// core so importing the flatten/filter helpers stays a pure node test.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

describe('slash commands', () => {
  it('filters by typed prefix (case-insensitive)', () => {
    const res = filterCommands('pl');
    expect(res.map((c) => c.name)).toEqual(['plan']);

    const upper = filterCommands('PL');
    expect(upper.map((c) => c.name)).toEqual(['plan']);
  });

  it('empty query returns every command', () => {
    expect(filterCommands('').length).toBe(SLASH_COMMANDS.length);
  });

  it('keeps session modes out of the in-session composer', () => {
    for (const mode of ['hive', 'fusion', 'research', 'debate']) {
      expect(findCommand(mode)).toBeUndefined();
    }
  });

  it('never expands to slash text sent to an attached CLI', () => {
    expect(findCommand('ask')?.expand()).toBe('Answer without modifying any files: ');
    expect(findCommand('plan')?.expand()).toBe('Produce a plan only; do not modify files: ');
    for (const command of SLASH_COMMANDS) {
      expect(command.expand()).not.toMatch(/^\s*\//);
    }
    expect(findCommand('clear')?.expand()).toBe('');
    expect(findCommand('attach')?.expand()).toBe('');
  });

  it('quick actions are tagged with control actions, not insert', () => {
    expect(findCommand('clear')?.action).toBe('clear');
    expect(findCommand('attach')?.action).toBe('attach');
    expect(findCommand('ask')?.action).toBe('insert');
  });
});

describe('mention flatten-to-string', () => {
  it('flattens agents/sessions/files to plain tokens', async () => {
    const { flattenMention, filterMentions } = await import('./sources');

    expect(flattenMention({ kind: 'agent', id: 'a1', label: 'Queen' })).toBe('@Queen');
    expect(flattenMention({ kind: 'session', id: 's1', label: 'my-sess' })).toBe('#my-sess');
    expect(
      flattenMention({ kind: 'file', id: 'C:\\repo\\src\\main.rs', label: 'main.rs' })
    ).toBe('C:\\repo\\src\\main.rs');

    // filter is a case-insensitive substring over label + detail.
    const items = [
      { kind: 'agent' as const, id: 'a1', label: 'Backend', detail: 'Worker 0' },
      { kind: 'agent' as const, id: 'a2', label: 'Frontend', detail: 'Worker 1' },
    ];
    expect(filterMentions(items, 'front').map((m) => m.label)).toEqual(['Frontend']);
    expect(filterMentions(items, '').length).toBe(2);
  });

  it('builds agent mentions with safe role labels (no object-variant bug)', async () => {
    const { agentMentions } = await import('./sources');
    const items = agentMentions([
      { id: 'q', role: 'Queen', status: 'Running', config: { cli: 'claude', flags: [] }, parent_id: null },
      {
        id: 'w0',
        role: { Worker: { index: 0, parent: null } },
        status: 'Running',
        config: { cli: 'claude', flags: [], label: 'Backend' },
        parent_id: 'q',
      },
    ] as never);

    expect(items[0].label).toBe('Queen');
    // Worker uses its config label when present, role label otherwise — never "[object Object]".
    expect(items[1].label).toBe('Backend');
    expect(items[1].detail).toBe('Worker 0');
  });
});
