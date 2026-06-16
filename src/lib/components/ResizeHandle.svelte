<script lang="ts">
  interface Props {
    /** Called with the pointer's clientX while dragging. */
    onResize: (clientX: number) => void;
    /** Accessible label, e.g. "Resize sidebar". */
    label: string;
    /** Notified when a drag starts/ends, so panels can suspend width transitions. */
    onDragChange?: (dragging: boolean) => void;
  }

  let { onResize, label, onDragChange }: Props = $props();
  let dragging = $state(false);

  function handlePointerDown(event: PointerEvent) {
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    dragging = true;
    onDragChange?.(true);
  }

  function handlePointerMove(event: PointerEvent) {
    if (!dragging) return;
    onResize(event.clientX);
  }

  function handlePointerUp(event: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    onDragChange?.(false);
    const handle = event.currentTarget as HTMLElement;
    handle.releasePointerCapture(event.pointerId);
  }
</script>

<div
  class="resize-handle"
  class:dragging
  role="separator"
  aria-orientation="vertical"
  aria-label={label}
  onpointerdown={handlePointerDown}
  onpointermove={handlePointerMove}
  onpointerup={handlePointerUp}
  onpointercancel={handlePointerUp}
></div>

<style>
  .resize-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
    z-index: 5;
    touch-action: none;
  }

  .resize-handle:hover,
  .resize-handle.dragging {
    background: color-mix(in srgb, var(--accent-cyan) 35%, transparent);
  }
</style>
