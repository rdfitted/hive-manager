<script lang="ts">
  type SkeletonRadius = 'none' | 'sm' | 'md' | 'lg' | 'full';

  interface Props {
    /** Any CSS width; percentages and design tokens make multi-column shapes easy to compose. */
    width?: string;
    /** Any CSS height; use the spacing/type tokens for icons, rows, and text lines. */
    height?: string;
    radius?: SkeletonRadius;
  }

  let { width = '100%', height = '1em', radius = 'sm' }: Props = $props();
</script>

<span
  class="lattice-skel-bar"
  data-radius={radius}
  style:width={width}
  style:height={height}
  aria-hidden="true"
></span>
