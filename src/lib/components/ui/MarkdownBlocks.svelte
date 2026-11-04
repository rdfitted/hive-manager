<script lang="ts">
  import { parseMarkdown, type MarkdownBlock } from '$lib/knowledge/markdown';
  import MarkdownInline from '../knowledge/MarkdownInline.svelte';
  import HtmlPreview from './HtmlPreview.svelte';

  let { source, allowHtmlPreview = false }: { source: string; allowHtmlPreview?: boolean } = $props();
  const blocks = $derived(parseMarkdown(source));
</script>

{#snippet renderBlocks(entries: MarkdownBlock[])}
  {#each entries as block, index (index)}
    {#if block.type === 'heading'}
      <svelte:element this={`h${block.level}`}><MarkdownInline text={block.text} /></svelte:element>
    {:else if block.type === 'paragraph'}
      <p><MarkdownInline text={block.text} /></p>
    {:else if block.type === 'quote'}
      <blockquote><MarkdownInline text={block.text} /></blockquote>
    {:else if block.type === 'code'}
      <div class="code-block">
        {#if block.language}<span class="language">{block.language}</span>{/if}
        <!-- Scrollable code needs a tab stop so keyboard users can pan long lines. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <pre class="lattice-scroll-content" tabindex="0" role="region" aria-label={block.language ? `Code block: ${block.language}` : 'Code block'}><code>{block.text}</code></pre>
        {#if allowHtmlPreview && block.language.toLowerCase() === 'html'}
          {#key block.text}<HtmlPreview source={block.text} />{/key}
        {/if}
      </div>
    {:else if block.type === 'list'}
      {#if block.ordered}
        <ol start={block.start ?? 1}>
          {#each block.items as item, itemIndex}
            <li><MarkdownInline text={item} />{#if block.children?.[itemIndex]}{@render renderBlocks(block.children[itemIndex])}{/if}</li>
          {/each}
        </ol>
      {:else}
        <ul>
          {#each block.items as item, itemIndex}
            <li><MarkdownInline text={item} />{#if block.children?.[itemIndex]}{@render renderBlocks(block.children[itemIndex])}{/if}</li>
          {/each}
        </ul>
      {/if}
    {:else if block.type === 'table'}
      <!-- The named scroll region must be reachable to pan a wide table by keyboard. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="table-scroll lattice-scroll-content" tabindex="0" role="region" aria-label="Markdown table">
        <table>
          <thead><tr>{#each block.headers as header, column}<th scope="col" style:text-align={block.align[column] ?? 'left'}><MarkdownInline text={header} /></th>{/each}</tr></thead>
          <tbody>{#each block.rows as row}<tr>{#each row as cell, column}<td style:text-align={block.align[column] ?? 'left'}><MarkdownInline text={cell} /></td>{/each}</tr>{/each}</tbody>
        </table>
      </div>
    {:else}
      <hr />
    {/if}
  {/each}
{/snippet}

<div class="markdown-blocks">{@render renderBlocks(blocks)}</div>

<style>
  .markdown-blocks { min-width: 0; max-width: 100%; color: var(--text-primary); font-size: inherit; line-height: 1.65; overflow-wrap: anywhere; }
  h1, h2, h3, h4, h5, h6 { margin: 1.3em 0 0.5em; font-family: var(--font-body); line-height: 1.25; }
  .markdown-blocks > :first-child { margin-top: 0; }
  h1 { font-size: var(--text-h1); }
  h2 { font-size: var(--text-h2); }
  h3 { font-size: var(--text-h3); }
  h4, h5, h6 { font-size: var(--text-base); }
  p { margin: 0 0 1em; white-space: pre-wrap; }
  ul, ol { margin: 0 0 1em; padding-left: 1.6em; }
  li { margin: 0.3em 0; }
  li > ul, li > ol { margin-bottom: 0; }
  li::marker { color: var(--text-muted); }
  blockquote { margin: 1em 0; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-muted); white-space: pre-wrap; }
  hr { margin: var(--space-4) 0; border: 0; height: 1px; background: var(--border); }
  .code-block { min-width: 0; margin: 1em 0; border-radius: var(--radius-md); background: var(--input); }
  .language { display: block; padding: var(--space-1) var(--space-3) 0; color: var(--text-muted); font: var(--text-micro) var(--font-mono); }
  pre { box-sizing: border-box; max-width: 100%; margin: 0; padding: var(--space-3); overflow: auto; white-space: pre; overflow-wrap: normal; font: var(--text-small)/1.55 var(--font-mono); }
  .table-scroll { max-width: 100%; margin: 1em 0; overflow: auto; border-radius: var(--radius-md); }
  table { width: 100%; border-collapse: collapse; font-size: inherit; }
  th, td { padding: var(--space-2) var(--space-3); white-space: nowrap; }
  th { background: var(--surface-raised); font-weight: 600; }
  tbody tr:nth-child(even) { background: var(--surface-raised); }
  .table-scroll:focus-visible, pre:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
  @media (forced-colors: active) { blockquote, .code-block, .table-scroll { border: 1px solid CanvasText; } th, td { border: 1px solid CanvasText; } }
</style>
