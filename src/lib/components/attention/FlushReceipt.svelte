<script module lang="ts">
  export function receiptSummary(body: string): { sentence: string; persistent: boolean } {
    let confirmed: unknown;
    try { confirmed = JSON.parse(body)?.submit_confirmed; } catch { /* Legacy non-JSON response. */ }
    if (confirmed === true) return { sentence: 'Enter delivered.', persistent: false };
    if (confirmed === null) return { sentence: "Enter sent. Delivery isn't confirmed yet.", persistent: false };
    if (confirmed === false) return { sentence: 'Enter may not have landed. Check the terminal.', persistent: true };
    return { sentence: 'Enter sent.', persistent: false };
  }
</script>

<script lang="ts">
  let { body, ondismiss }: { body: string; ondismiss(): void } = $props();
  const summary = $derived(receiptSummary(body));
  $effect(() => {
    if (summary.persistent) return;
    const timer = setTimeout(ondismiss, 8000);
    return () => clearTimeout(timer);
  });
</script>

<div class="flush-receipt">
  <div class="receipt-line">
    <p role="status">{summary.sentence}</p>
    <button type="button" class="lattice-btn lattice-btn--ghost" aria-label="Dismiss Enter receipt" onpointerdown={(event) => event.preventDefault()} onclick={ondismiss}>Dismiss</button>
  </div>
  {#key body}<details>
    <summary>Details</summary>
    <pre aria-label="Inject receipt">{body}</pre>
  </details>{/key}
</div>

<style>
  .flush-receipt { margin-top: var(--space-2); font-size: var(--text-small); }
  .receipt-line { display: flex; align-items: center; gap: var(--space-2); }
  p { margin: 0; }
  summary { cursor: pointer; color: var(--text-secondary); }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 10rem; overflow: auto; }
  @media (forced-colors: active) { summary { color: ButtonText; } }
</style>
