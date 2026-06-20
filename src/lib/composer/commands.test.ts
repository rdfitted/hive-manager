import { describe, it, expect, vi } from 'vitest';
import { filterCommands, findCommand, SLASH_COMMANDS } from './commands';

// sources.ts pulls in @tauri-apps/api/core (invoke) and the sessions store; mock the Tauri
// core so importing the flatten/filter helpers stays a pure node test.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

describe('slash commands', () => {
  it('filters by typed prefix (case-insensitive)', () => {
    const res = filterCommands('res');
    expect(res.map((c) => c.name)).toEqual(['research']);

    const upper = filterCommands('RES');
    expect(upper.map((c) => c.name)).toEqual(['research']);
  });

  it('empty query returns every command', () => {
    expect(filterCommands('').length).toBe(SLASH_COMMANDS.length);
  });

  it('the three real SessionMode commands resolve to their expansions', () => {
    expect(findCommand('hive')?.expand()).toBe('/hive ');
    expect(findCommand('fusion')?.expand()).toBe('/fusion ');
    expect(findCommand('research')?.expand()).toBe('/research ');
    expect(findCommand('debate')?.expand()).toBe('/debate ');
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
