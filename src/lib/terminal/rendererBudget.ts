/** Maximum number of terminal panes allowed to hold a WebGL renderer grant. */
export const MAX_WEBGL_RENDERER_GRANTS = 8;

// Set iteration follows insertion order, so the first id is always the least
// recently promoted grant. Keeping this registry at module scope makes the cap
// apply across every Terminal instance and every terminal mount site.
const grants = new Set<string>();

/**
 * Acquire an available renderer grant.
 *
 * Re-acquiring a held id is idempotent and does not change its LRU position.
 * Returns false when the budget is full; callers can keep using the DOM
 * renderer until the pane is promoted.
 */
export function acquire(id: string): boolean {
  if (grants.has(id)) return true;
  if (grants.size >= MAX_WEBGL_RENDERER_GRANTS) return false;

  grants.add(id);
  return true;
}

/**
 * Give an id a renderer grant and mark it most recently used.
 *
 * When the budget is full, the least-recently-used grant is removed and its id
 * is returned so the caller can move that pane back to the DOM renderer.
 * Returns null when no eviction was necessary.
 */
export function promote(id: string): string | null {
  if (grants.delete(id)) {
    grants.add(id);
    return null;
  }

  let evictedId: string | null = null;
  if (grants.size >= MAX_WEBGL_RENDERER_GRANTS) {
    evictedId = grants.values().next().value ?? null;
    if (evictedId !== null) grants.delete(evictedId);
  }

  grants.add(id);
  return evictedId;
}

/** Release a renderer grant. Returns whether the id held one. */
export function release(id: string): boolean {
  return grants.delete(id);
}
