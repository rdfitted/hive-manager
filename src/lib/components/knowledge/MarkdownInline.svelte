<script lang="ts">
  import { parseInlineMarkdown } from '$lib/knowledge/markdown';
  import { openUrl } from '@tauri-apps/plugin-opener';

  interface Props { text: string; }
  let { text }: Props = $props();
  let tokens = $derived(parseInlineMarkdown(text));

  function openLink(event: MouseEvent, href: string) {
    event.preventDefault();
    // The parser admits only web URLs; check again at the native API boundary.
    try {
      const url = new URL(href);
      if (url.protocol === 'http:' || url.protocol === 'https:') void openUrl(url.toString()).catch(() => {});
    } catch { /* Incomplete streamed links remain inert. */ }
  }
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
      <a href={token.href} rel="noreferrer" onclick={(event) => openLink(event, token.href)}>{token.text}</a>
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
    padding: 2px 5px;
    background: var(--surface-raised);
    border-radius: var(--radius-md);
    color: var(--accent-chrome);
    font: 0.9em var(--font-mono);
  }
  .inline-markdown :global(a) {
    color: var(--accent);
    text-decoration-color: color-mix(in srgb, var(--accent) 45%, transparent);
    text-underline-offset: 2px;
  }
  .inline-markdown :global(a:hover) { text-decoration-color: var(--accent); }
  .inline-markdown :global(a:focus-visible) { outline: 2px solid var(--focus); outline-offset: 2px; }
  .wikilink {
    color: var(--accent);
    font-family: var(--font-body);
  }
  .wikilink::before { content: '[['; color: var(--text-muted); }
  .wikilink::after { content: ']]'; color: var(--text-muted); }
</style>
