// @ts-expect-error Node types are intentionally absent from the frontend tsconfig.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const readJson = (name: string) => JSON.parse(readFileSync(`src-tauri/${name}`, 'utf8'));
const base = readJson('tauri.conf.json');
const grants = [
  'core:window:allow-minimize',
  'core:window:allow-toggle-maximize',
  'core:window:allow-close',
  'core:window:allow-start-dragging',
];

describe('G7: platform window configuration preserves the locked preview', () => {
  it('leaves the base main window framed and the preview capability empty', () => {
    expect(base.app.windows.find((w: { label: string }) => w.label === 'main').decorations).toBe(true);
    expect(base.app.windows.find((w: { label: string }) => w.label === 'operator-preview').decorations).toBe(true);
    const preview = readJson('capabilities/operator-preview.json');
    expect(preview.windows).toEqual(['operator-preview']);
    expect(preview.permissions).toEqual([]);
  });

  it.each(['windows', 'macos'])('%s override repeats both complete windows and changes only allowed main fields', (platform) => {
    const override = readJson(`tauri.${platform}.conf.json`);
    expect(Object.keys(override)).toEqual(['app']);
    expect(Object.keys(override.app)).toEqual(['windows']);
    expect(override).not.toHaveProperty('version');
    expect(override.app.windows.map((w: { label: string }) => w.label)).toEqual(base.app.windows.map((w: { label: string }) => w.label));
    const main = override.app.windows.find((w: { label: string }) => w.label === 'main');
    const baseMain = base.app.windows.find((w: { label: string }) => w.label === 'main');
    expect(main).toEqual(platform === 'windows'
      ? { ...baseMain, decorations: false }
      : { ...baseMain, titleBarStyle: 'Overlay', hiddenTitle: true });
    const preview = override.app.windows.find((w: { label: string }) => w.label === 'operator-preview');
    expect(preview).toEqual(base.app.windows.find((w: { label: string }) => w.label === 'operator-preview'));
    expect(preview.decorations).toBe(true);
  });

  it('grants exactly the four new chrome operations to the main window only', () => {
    const capability = readJson('capabilities/default.json');
    expect(capability.windows).toEqual(['main']);
    expect(capability.permissions.filter((p: string) => p.startsWith('core:window:'))).toEqual([
      'core:window:allow-set-focus', ...grants,
    ]);
    for (const grant of grants) expect(capability.permissions).toContain(grant);
  });
});
