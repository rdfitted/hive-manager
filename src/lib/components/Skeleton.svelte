<script lang="ts">
  import type { Snippet } from 'svelte';

  type SkeletonLayout = 'flow' | 'inline';

  interface Props {
    /** True while the placeholder is visible; false reveals the real content. */
    loading: boolean;
    /** Content-shaped placeholder composed from one or more SkelBar instances. */
    skeleton: Snippet;
    /** Real content held in the second crossfade layer. */
    children: Snippet;
    /** Flow preserves block layout; inline composes inside compact labels/controls. */
    layout?: SkeletonLayout;
    /** Root hook; scoped callers must style it through an anchored :global(...) selector. */
    class?: string;
  }

  let {
    loading,
    skeleton,
    children,
    layout = 'flow',
    class: className = '',
  }: Props = $props();
</script>

<div
  class="lattice-skeleton {className}"
  class:is-revealed={!loading}
  class:is-resetting={loading}
  data-layout={layout}
  aria-busy={loading}
>
  <div class="lattice-skeleton-placeholder" aria-hidden="true">
    {@render skeleton()}
  </div>
  <div class="lattice-skeleton-content" aria-hidden={loading} inert={loading}>
    {@render children()}
  </div>
</div>
