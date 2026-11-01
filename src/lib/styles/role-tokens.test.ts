import { describe, expect, it, vi } from 'vitest';

// Read production sources without the Vite CSS stubs or live user settings.
const { readFileSync, readdirSync, statSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
  readdirSync(path: string): string[];
  statSync(path: string): { isDirectory(): boolean };
}>('node:fs');

type Allowance = { reason: string; occurrences: Record<string, number> };

// Only the terminal's pinned palette and terminal-local overlays retain legacy
// inputs. Exact budgets prevent this exception spreading to new UI uses.
const hexAllowlist: Record<string, Allowance> = {};
const roleAllowlist: Record<string, Allowance> = {
  'src/lib/components/Terminal.svelte': {
    reason: 'Pinned xterm palette reads plus terminal inset, drag target, search and terminal-local status text; preserve terminal palette independently of navy UI roles.',
    occurrences: { '--bg-void': 4, '--accent-cyan': 3, '--text-secondary': 5 },
  },
};

function componentFiles(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = `${directory}/${name}`;
    return statSync(path).isDirectory() ? componentFiles(path) : (path.endsWith('.svelte') || (path.endsWith('.css') && !path.startsWith('src/lib/styles/'))) ? [path] : [];
  });
}

function violations(pattern: RegExp, allowlist: Record<string, Allowance>): string[] {
  return componentFiles('src').flatMap((path) => {
    const counts: Record<string, number> = {};
    for (const match of readFileSync(path, 'utf8').matchAll(pattern)) {
      counts[match[0]] = (counts[match[0]] ?? 0) + 1;
    }
    return Object.entries(counts).flatMap(([value, count]) => {
      const budget = allowlist[path]?.occurrences[value] ?? 0;
      return count > budget ? [`${path}: ${value} has ${count} occurrences; allowed ${budget}`] : [];
    });
  });
}

describe('UI color migration guards', () => {
  it('requires a specific reason and occurrence budget for every Terminal exception', () => {
    expect(Object.keys(hexAllowlist)).toEqual([]);
    expect(Object.keys(roleAllowlist)).toEqual(['src/lib/components/Terminal.svelte']);
    for (const allowance of [...Object.values(hexAllowlist), ...Object.values(roleAllowlist)]) {
      expect(allowance.reason.trim().length).toBeGreaterThan(30);
      expect(Object.keys(allowance.occurrences).length).toBeGreaterThan(0);
      for (const count of Object.values(allowance.occurrences)) expect(count).toBeGreaterThan(0);
    }
  });

  it('rejects new six-digit hex colors in components (G26)', () => {
    expect(violations(/#[0-9a-f]{6}\b/gi, hexAllowlist)).toEqual([]);
  });

  it('rejects new legacy UI token uses in components (G28)', () => {
    expect(violations(/--(?:bg-void|accent-cyan|accent-amber|text-secondary|status-queued|status-canceled)\b/g, roleAllowlist)).toEqual([]);
  });

  it('keeps global CSS migrated without any legacy UI roles', () => {
    for (const name of readdirSync('src/lib/styles/globals').filter((name) => name.endsWith('.css'))) {
      expect(readFileSync(`src/lib/styles/globals/${name}`, 'utf8'), name)
        .not.toMatch(/--(?:bg-void|accent-cyan|accent-amber|text-secondary|status-queued|status-canceled)\b/);
    }
  });
});
