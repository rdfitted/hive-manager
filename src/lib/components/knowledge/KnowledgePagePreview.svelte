<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { X } from 'phosphor-svelte';
  import MarkdownInline from './MarkdownInline.svelte';
  import { parseMarkdown } from '$lib/knowledge/markdown';
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
  let blocks = $derived(page ? parseMarkdown(page.content) : []);

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

<aside class="preview" aria-label="Knowledge page preview" aria-busy={loading}>
  <header>
    <div class="preview-kicker">Read-only preview</div>
    <button
      bind:this={closeButton}
      type="button"
      class="close"
      onclick={closePreview}
      aria-label="Close page preview"
    >
      <X size={16} weight="light" />
    </button>
  </header>

  {#if loading}
    <div class="preview-state">
      <span class="loader"></span>
      <strong>Opening page</strong>
      <span>{selectedId}</span>
    </div>
  {:else if error}
    <div class="preview-state error-state" role="alert">
      <strong>Page unavailable</strong>
      <span>{error}</span>
      <button type="button" onclick={onRetry}>Try again</button>
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

    <article class="markdown">
      {#each blocks as block, index (index)}
        {#if block.type === 'heading'}
          {#if block.level === 1}<h1><MarkdownInline text={block.text} /></h1>
          {:else if block.level === 2}<h2><MarkdownInline text={block.text} /></h2>
          {:else if block.level === 3}<h3><MarkdownInline text={block.text} /></h3>
          {:else}<h4><MarkdownInline text={block.text} /></h4>{/if}
        {:else if block.type === 'paragraph'}
          <p><MarkdownInline text={block.text} /></p>
        {:else if block.type === 'quote'}
          <blockquote><MarkdownInline text={block.text} /></blockquote>
        {:else if block.type === 'code'}
          <div class="code-block">
            {#if block.language}<span>{block.language}</span>{/if}
            <pre><code>{block.text}</code></pre>
          </div>
        {:else if block.type === 'list'}
          {#if block.ordered}
            <ol>{#each block.items as item}<li><MarkdownInline text={item} /></li>{/each}</ol>
          {:else}
            <ul>{#each block.items as item}<li><MarkdownInline text={item} /></li>{/each}</ul>
          {/if}
        {:else}
          <hr />
        {/if}
      {/each}
    </article>
  {/if}
</aside>

<style>
  .preview {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--bg-surface);
    border-left: 1px solid var(--border-structural);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 44px;
    padding: 0 var(--space-3) 0 var(--space-4);
    border-bottom: 1px solid var(--border-structural);
  }

  .preview-kicker,
  .folder-label {
    font: var(--text-micro) var(--font-mono);
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .close:hover {
    color: var(--text-primary);
    border-color: var(--border-structural);
    background: var(--bg-elevated);
  }

  .page-head {
    padding: var(--space-5) var(--space-5) var(--space-4);
    border-bottom: 1px solid var(--border-structural);
    background: linear-gradient(150deg, color-mix(in srgb, var(--accent-cyan) 4%, transparent), transparent 52%);
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
    font-family: var(--font-display);
    font-size: var(--text-h1);
    line-height: 1.15;
    letter-spacing: 0.015em;
  }

  .page-meta {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    color: var(--text-disabled);
    font: var(--text-micro) var(--font-mono);
  }

  .page-meta code {
    overflow: hidden;
    color: var(--text-secondary);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .page-meta .truncated {
    color: var(--accent-amber);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .markdown {
    flex: 1;
    overflow: auto;
    padding: var(--space-5);
    color: var(--text-primary);
    font-size: 13px;
    line-height: 1.65;
  }

  .markdown h1,
  .markdown h2,
  .markdown h3,
  .markdown h4 {
    margin: 1.55em 0 0.55em;
    color: var(--text-primary);
    font-family: var(--font-display);
    line-height: 1.2;
  }

  .markdown h1:first-child,
  .markdown h2:first-child,
  .markdown h3:first-child {
    margin-top: 0;
  }

  .markdown h1 { font-size: 22px; }
  .markdown h2 { font-size: 19px; }
  .markdown h3 { font-size: 16px; }
  .markdown h4 { font-size: 14px; color: var(--accent-chrome); }

  .markdown p {
    margin: 0 0 1em;
    white-space: pre-wrap;
  }

  .markdown ul,
  .markdown ol {
    margin: 0 0 1em;
    padding-left: 1.5em;
  }

  .markdown li {
    margin-bottom: 0.3em;
  }

  .markdown li::marker {
    color: var(--accent-cyan);
  }

  .markdown blockquote {
    margin: 1em 0;
    padding: var(--space-2) var(--space-3);
    border-left: 2px solid var(--accent-amber);
    background: color-mix(in srgb, var(--accent-amber) 5%, transparent);
    color: var(--text-secondary);
    white-space: pre-wrap;
  }

  .markdown hr {
    margin: var(--space-5) 0;
    border: 0;
    border-top: 1px solid var(--border-structural);
  }

  .code-block {
    position: relative;
    margin: 1em 0;
    border: 1px solid var(--border-structural);
    background: var(--bg-void);
  }

  .code-block > span {
    position: absolute;
    top: 5px;
    right: 7px;
    color: var(--text-disabled);
    font: 9px var(--font-mono);
    text-transform: uppercase;
  }

  .code-block pre {
    margin: 0;
    padding: var(--space-3);
    overflow: auto;
    color: var(--accent-chrome);
    font: 11px/1.55 var(--font-mono);
  }

  .preview-state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-5);
    color: var(--text-secondary);
    text-align: center;
  }

  .preview-state strong {
    color: var(--text-primary);
    font-family: var(--font-display);
  }

  .preview-state span:not(.loader) {
    max-width: 280px;
    font: var(--text-micro) var(--font-mono);
    overflow-wrap: anywhere;
  }

  .preview-state button {
    margin-top: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--accent-cyan);
    background: transparent;
    color: var(--accent-cyan);
    font: var(--text-small) var(--font-mono);
    cursor: pointer;
  }

  .loader {
    width: 18px;
    height: 18px;
    border: 1px solid var(--border-structural);
    border-top-color: var(--accent-cyan);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .error-state strong {
    color: var(--status-error);
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  @media (prefers-reduced-motion: reduce) {
    .loader { animation: none; }
  }
</style>
