<script lang="ts">
  /**
   * Dispatch host for tool-render widgets.
   *
   * Resolves a renderer for a ConversationMessage and mounts it dynamically
   * (Svelte 5 runes: components are dynamic by default), forwarding the widget's
   * approve / reject signals up to the chat core via optional callback props. On
   * NO match OR any thrown render error, falls back to a formatted-JSON <pre> of
   * `data ?? content` (issue #127, criterion 3).
   *
   * This is the ONLY component the chat core dispatches to — adding a new
   * renderer never requires editing ConversationViewer (criterion 4).
   */
  import type { ConversationMessage } from '$lib/stores/conversations';
  import { resolveToolRenderer, type ApprovalActionDetail } from './registry';
  import { registerBuiltinRenderers } from './builtins';

  let {
    message,
    onapprove,
    onreject,
  }: {
    message: ConversationMessage;
    onapprove?: (detail: ApprovalActionDetail) => void;
    onreject?: (detail: ApprovalActionDetail) => void;
  } = $props();

  // Ensure built-ins are registered before the first resolve. Idempotent.
  registerBuiltinRenderers();

  const resolved = $derived(
    resolveToolRenderer({
      renderer: message.renderer,
      toolName: message.from,
      data: message.data,
    }),
  );

  // The data handed to the widget; fall back to the message content when the
  // envelope carried no structured data.
  const widgetData = $derived(message.data ?? message.content);

  // Capitalized alias so the template can mount it directly (Svelte 5 runes:
  // components are dynamic by default, no <svelte:component> needed). A
  // render-time throw inside the widget is caught by the <svelte:boundary>
  // below, which drops back to the fallback (criterion 3).
  const Widget = $derived(resolved?.component ?? null);

  const fallbackJson = $derived.by(() => {
    const value = message.data ?? message.content;
    try {
      return typeof value === 'string' ? value : JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  });

  function forwardApprove(detail: ApprovalActionDetail) {
    onapprove?.(detail);
  }

  function forwardReject(detail: ApprovalActionDetail) {
    onreject?.(detail);
  }
</script>

<div class="tool-render-host">
  {#if Widget}
    <svelte:boundary>
      <Widget data={widgetData} onapprove={forwardApprove} onreject={forwardReject} />
      {#snippet failed()}
        <pre class="tool-render-fallback">{fallbackJson}</pre>
      {/snippet}
    </svelte:boundary>
  {:else}
    <pre class="tool-render-fallback">{fallbackJson}</pre>
  {/if}
</div>

<style>
  .tool-render-host {
    width: 100%;
  }

  .tool-render-fallback {
    margin: 0;
    padding: 8px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-secondary);
    background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
