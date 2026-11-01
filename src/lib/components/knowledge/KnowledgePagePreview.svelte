<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { X } from 'phosphor-svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import SkelBar from '$lib/components/SkelBar.svelte';
  import MarkdownBlocks from '../ui/MarkdownBlocks.svelte';
  import { describeOmission, folderColor, formatLastUpdated } from '$lib/knowledge/graphUtils';
  import type { KnowledgePage } from '$lib/knowledge/types';

  interface FocusTarget {
    readonly isConnected: boolean;
    focus: (options?: FocusOptions) => void;
  }

  interface Props {
    selectedId: string;
    page: KnowledgePage | null;
    loading: boolean;
    error: string | null;
    onClose: () => void | Promise<unknown>;
    onRetry: () => void;
    returnFocus?: FocusTarget | null;
  }

  let {
    selectedId,
    page,
    loading,
    error,
    onClose,
    onRetry,
    returnFocus = null,
  }: Props = $props();
  let closeButton: HTMLButtonElement;

  onMount(() => {
    closeButton.focus({ preventScroll: true });
  });

  async function closePreview() {
    const target = returnFocus;
    try {
      await onClose();
    } finally {
      await tick();
      if (target?.isConnected) target.focus({ preventScroll: true });
    }
  }
</script>

<aside
  class="preview lattice-panel"
  aria-label={`Knowledge page preview: ${selectedId}`}
  aria-busy={loading}
>
  <header>
    <div class="preview-kicker">Read-only preview</div>
    <button
      bind:this={closeButton}
      type="button"
      class="lattice-btn lattice-btn--ghost lattice-btn--icon"
      onclick={closePreview}
      aria-label="Close page preview"
    >
      <X size={16} weight="light" />
    </button>
  </header>

  {#snippet pageSkeleton()}
    <div class="preview-page-skeleton-shape">
      <SkelBar width="7rem" height="0.65rem" />
      <SkelBar width="min(22rem, 76%)" height="1.65rem" radius="md" />
      <div class="preview-page-skeleton-meta">
        <SkelBar width="64%" height="0.6rem" />
        <SkelBar width="9rem" height="0.6rem" />
      </div>
      <div class="preview-page-skeleton-lines">
        <SkelBar width="100%" height="0.75rem" />
        <SkelBar width="92%" height="0.75rem" />
        <SkelBar width="78%" height="0.75rem" />
        <SkelBar width="96%" height="0.75rem" />
        <SkelBar width="69%" height="0.75rem" />
      </div>
    </div>
  {/snippet}

  <Skeleton loading={loading} skeleton={pageSkeleton} class="preview-skeleton">
    {#if error}
      <div class="preview-state error-state" role="alert">
        <strong>Page unavailable</strong>
        <span>{error}</span>
        <button
          class="lattice-btn lattice-btn--secondary"
          type="button"
          onclick={onRetry}
        >
          Try again
        </button>
      </div>
    {:else if page}
      <div class="page-head">
        <div class="folder-label">
          <span style:background={folderColor(page.folder)}></span>
          {page.folder}
        </div>
        <h2>{page.title}</h2>
        <div class="page-meta">
          <code>{page.path}</code>
          <span>Updated {formatLastUpdated(page.last_updated)}</span>
          {#each page.omissions ?? [] as omission (omission.reason)}
            <span class="truncated">{describeOmission(omission)}</span>
          {:else}
            {#if page.truncated}<span class="truncated">Preview capped for safety</span>{/if}
          {/each}
        </div>
      </div>

      <article class="markdown lattice-scroll-content">
        <MarkdownBlocks source={page.content} />
      </article>
    {/if}
  </Skeleton>
</aside>

<style>
  .preview {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 44px;
    padding: 0 var(--space-3) 0 var(--space-4);
    box-shadow: var(--edge-seam);
  }

  .preview-kicker,
  .folder-label {
    font: var(--text-micro) var(--font-body);
    color: var(--text-muted);
  }

  .page-head {
    padding: var(--space-5) var(--space-5) var(--space-4);
    background: linear-gradient(150deg, color-mix(in srgb, var(--accent) 4%, transparent), transparent 52%);
    box-shadow: var(--edge-seam);
  }

  .folder-label {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .folder-label span {
    width: 7px;
    height: 7px;
  }

  .page-head h2 {
    margin: var(--space-3) 0;
    font-family: var(--font-body);
    font-size: var(--text-h1);
    line-height: 1.15;
  }

  .page-meta {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    color: var(--text-disabled);
    font: var(--text-micro) var(--font-body);
  }

  .page-meta code {
    overflow: hidden;
    color: var(--text-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .page-meta .truncated {
    color: var(--accent);
  }

  .preview :global(.preview-skeleton) {
    flex: 1;
    min-height: 0;
  }

  .preview :global(.preview-skeleton > .lattice-skeleton-content) {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .preview-page-skeleton-shape {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    box-sizing: border-box;
    min-height: 100%;
    padding: var(--space-5);
  }

  .preview-page-skeleton-meta,
  .preview-page-skeleton-lines {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .preview-page-skeleton-lines {
    margin-top: var(--space-3);
  }

  .markdown {
    flex: 1;
    overflow: auto;
    padding: var(--space-5);
    color: var(--text-primary);
    font-size: 13px;
    line-height: 1.65;
  }

  .preview-state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-5);
    color: var(--text-muted);
    text-align: center;
  }

  .preview-state strong {
    color: var(--text-primary);
    font-family: var(--font-body);
  }

  .preview-state span {
    max-width: 280px;
    font: var(--text-micro) var(--font-body);
    overflow-wrap: anywhere;
  }

  .error-state strong {
    color: var(--status-error);
  }
</style>
