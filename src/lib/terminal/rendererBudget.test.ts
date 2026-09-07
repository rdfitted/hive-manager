import { afterEach, describe, expect, it } from 'vitest';
import {
  MAX_WEBGL_RENDERER_GRANTS,
  acquire,
  promote,
  release,
} from './rendererBudget';

const idsUsedByTests = new Set<string>();

function id(value: string): string {
  idsUsedByTests.add(value);
  return value;
}

afterEach(() => {
  for (const rendererId of idsUsedByTests) release(rendererId);
  idsUsedByTests.clear();
});

describe('renderer budget', () => {
  it('keeps the WebGL grant policy below Chromium\'s context ceiling', () => {
    // Keep this literal independent of production: eight leaves headroom below
    // Chromium's approximate 16-context ceiling for other application canvases.
    expect(MAX_WEBGL_RENDERER_GRANTS).toBe(8);
  });

  it('caps grants at eight across more than eight concurrent acquires', () => {
    const results = Array.from(
      { length: MAX_WEBGL_RENDERER_GRANTS + 4 },
      (_, index) => acquire(id(`pane-${index}`)),
    );

    expect(results.filter(Boolean)).toHaveLength(MAX_WEBGL_RENDERER_GRANTS);
    expect(results.slice(0, MAX_WEBGL_RENDERER_GRANTS)).toEqual(
      Array(MAX_WEBGL_RENDERER_GRANTS).fill(true),
    );
    expect(results.slice(MAX_WEBGL_RENDERER_GRANTS)).toEqual(Array(4).fill(false));
  });

  it('promotes a pane by evicting and reporting the least-recently-used id', () => {
    const initialIds = Array.from(
      { length: MAX_WEBGL_RENDERER_GRANTS },
      (_, index) => id(`initial-${index}`),
    );
    for (const rendererId of initialIds) expect(acquire(rendererId)).toBe(true);

    expect(promote(id('promoted-pane'))).toBe(initialIds[0]);
    expect(acquire(initialIds[0])).toBe(false);
    expect(acquire('promoted-pane')).toBe(true);
  });

  it('refreshes a held grant so the next promotion evicts the next-oldest id', () => {
    const initialIds = Array.from(
      { length: MAX_WEBGL_RENDERER_GRANTS },
      (_, index) => id(`refresh-${index}`),
    );
    for (const rendererId of initialIds) acquire(rendererId);

    expect(promote(initialIds[0])).toBeNull();
    expect(promote(id('newly-promoted-pane'))).toBe(initialIds[1]);
  });

  it('release frees a slot for a later acquire', () => {
    const initialIds = Array.from(
      { length: MAX_WEBGL_RENDERER_GRANTS },
      (_, index) => id(`release-${index}`),
    );
    for (const rendererId of initialIds) acquire(rendererId);

    expect(acquire(id('waiting-pane'))).toBe(false);
    expect(release(initialIds[3])).toBe(true);
    expect(acquire('waiting-pane')).toBe(true);
    expect(release(initialIds[3])).toBe(false);
  });

  it('reacquiring a held id does not consume another slot', () => {
    const heldId = id('held-pane');
    expect(acquire(heldId)).toBe(true);
    expect(acquire(heldId)).toBe(true);

    for (let index = 1; index < MAX_WEBGL_RENDERER_GRANTS; index += 1) {
      expect(acquire(id(`other-${index}`))).toBe(true);
    }
    expect(acquire(id('over-cap-pane'))).toBe(false);
  });
});
