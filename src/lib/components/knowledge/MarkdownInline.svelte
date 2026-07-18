<script lang="ts">
  import { parseInlineMarkdown } from '$lib/knowledge/markdown';

  interface Props { text: string; }
  let { text }: Props = $props();
  let tokens = $derived(parseInlineMarkdown(text));
</script>

<span class="inline-markdown">
  {#each tokens as token, index (index)}
    {#if token.type === 'strong'}
      <strong>{token.text}</strong>
    {:else if token.type === 'emphasis'}
      <em>{token.text}</em>
    {:else if token.type === 'code'}
      <code>{token.text}</code>
    {:else if token.type === 'link'}
      <a href={token.href} target="_blank" rel="noreferrer">{token.text}</a>
    {:else if token.type === 'wikilink'}
      <span class="wikilink">{token.text}</span>
    {:else}
      {token.text}
    {/if}
  {/each}
</span>

<style>
  .inline-markdown :global(strong) { color: var(--text-primary); font-weight: 600; }
  .inline-markdown :global(em) { color: var(--accent-chrome); }
  .inline-markdown :global(code) {
    padding: 1px 4px;
    border: 1px solid var(--border-structural);
    background: var(--bg-void);
    color: var(--accent-chrome);
    font: 0.9em var(--font-mono);
  }
  .inline-markdown :global(a) {
    color: var(--accent-cyan);
    text-decoration-color: color-mix(in srgb, var(--accent-cyan) 45%, transparent);
    text-underline-offset: 2px;
  }
  .inline-markdown :global(a:hover) { text-decoration-color: var(--accent-cyan); }
  .wikilink {
    color: var(--accent-cyan);
    font-family: var(--font-mono);
  }
  .wikilink::before { content: '[['; color: var(--text-disabled); }
  .wikilink::after { content: ']]'; color: var(--text-disabled); }
</style>
