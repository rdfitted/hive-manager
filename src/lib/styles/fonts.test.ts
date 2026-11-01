import { describe, expect, it, vi } from 'vitest';

const { readFileSync, readdirSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
  readdirSync(path: string, options: { recursive: true }): string[];
}>('node:fs');

describe('bundled fonts (G4)', () => {
  it('has no Google Fonts network references in app.html or styles', () => {
    const paths = ['src/app.html', ...readdirSync('src/lib/styles', { recursive: true })
      .filter((path) => path.endsWith('.css')).map((path) => `src/lib/styles/${path}`)];
    for (const path of paths) {
      expect(readFileSync(path, 'utf8'), path).not.toMatch(/fonts\.(?:googleapis|gstatic)\.com/i);
    }
  });

  it('loads both Plex families, subsets and weights through the token entry point', () => {
    expect(readFileSync('src/lib/styles/lattice-tokens.css', 'utf8')).toMatch(/^@import "\.\/fonts\.css";/);
    const fonts = readFileSync('src/lib/styles/fonts.css', 'utf8');
    for (const family of ['sans', 'mono']) for (const subset of ['latin', 'latin-ext']) {
      for (const weight of [400, 500, 600]) {
        const path = `@fontsource/ibm-plex-${family}/${subset}-${weight}.css`;
        expect(fonts).toContain(`@import "${path}";`);
        expect(readFileSync(`node_modules/${path}`, 'utf8')).toContain('@font-face');
      }
    }
  });
});
