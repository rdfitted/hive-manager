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
  let detailsOpen = $state(false);
  let receiptBody: string | undefined;
  let remaining = 8000;
  $effect(() => {
    if (receiptBody !== body) {
      receiptBody = body;
      remaining = 8000;
      if (detailsOpen) { detailsOpen = false; return; }
    }
    if (summary.persistent || detailsOpen) return;
    const started = Date.now();
    const timer = setTimeout(ondismiss, remaining);
    return () => {
      clearTimeout(timer);
      remaining = Math.max(0, remaining - (Date.now() - started));
    };
  });
</script>

<div class="flush-receipt">
  <div class="receipt-line">
    <p role="status">{summary.sentence}</p>
    <button type="button" class="lattice-btn lattice-btn--ghost" aria-label="Dismiss Enter receipt" onpointerdown={(event) => event.preventDefault()} onclick={ondismiss}>Dismiss</button>
  </div>
  {#key body}<details bind:open={detailsOpen}>
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
