<script lang="ts" module>
  // The approved mockup's own line set: 16-unit grid, 1.5 stroke, round joins.
  const paths = {
    activity: 'M1.5 8h3l2-5 3 10 2-5h3',
    all: 'M3 4h10M3 8h10M3 12h10',
    bell: 'M4 11V7.5a4 4 0 0 1 8 0V11l1 1.5H3zM6.5 14.5h3',
    changes: 'M5 2v8M5 14v-1M2.5 5.5h5M11 2v1M11 6v8M8.5 10.5h5',
    chat: 'M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z',
    check: 'M8 2.5a5.5 5.5 0 1 0 0 11a5.5 5.5 0 1 0 0-11M5.5 8.2l1.8 1.8 3.2-3.6',
    chevron: 'M6 3.5l4.5 4.5L6 12.5',
    clock: 'M8 2.5a5.5 5.5 0 1 0 0 11a5.5 5.5 0 1 0 0-11M8 5v3l2 1.5',
    file: 'M4 2h5l3 3v9H4zM9 2v3h3',
    hex: 'M8 2l5.2 3v6L8 14l-5.2-3V5z',
    hold: 'M5.5 3.5v9M10.5 3.5v9',
    log: 'M2.5 4h1M6 4h7.5M2.5 8h1M6 8h7.5M2.5 12h1M6 12h7.5',
    open: 'M9.5 2.5h4v4M13.5 2.5l-5 5M6.5 13.5h-4v-4M2.5 13.5l5-5',
    play: 'M5 3.2l7.5 4.8L5 12.8z',
    restore: 'M13.5 6.5h-4v-4M9.5 6.5l4.5-4.5M2.5 9.5h4v4M6.5 9.5L2 14',
    send: 'M8 13V3M4 7l4-4 4 4',
    split: 'M3.5 2.5h9a1.5 1.5 0 0 1 1.5 1.5v8a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 12V4a1.5 1.5 0 0 1 1.5-1.5M2 9h12',
    term: 'M3 5l3 3-3 3M8 11h5',
    up: 'M4 10l4-4 4 4',
  } as const;

  export type GlyphName = keyof typeof paths;
</script>

<script lang="ts">
  let { name, size = 14 }: { name: GlyphName; size?: number } = $props();
</script>

<svg class="glyph" width={size} height={size} viewBox="0 0 16 16" data-glyph={name} aria-hidden="true" focusable="false">
  <path d={paths[name]} />
</svg>

<style>
  .glyph {
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
</style>
