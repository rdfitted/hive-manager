<script lang="ts">
  import Composer, { type ComposerControls } from './Composer.svelte';
  import ComposerToolbar from './ComposerToolbar.svelte';
  import Dialog from '../ui/Dialog.svelte';

  let { onsubmit, onform, observeControls = (controls) => controls, sending = false, sessionId = null, persistDraft = false, value = '', disabled = false, dialog = false }: {
    onsubmit?: (text: string) => void;
    onform?: () => void;
    observeControls?: (controls: ComposerControls) => ComposerControls;
    sending?: boolean;
    sessionId?: string | null;
    persistDraft?: boolean;
    value?: string;
    disabled?: boolean;
    dialog?: boolean;
  } = $props();
</script>

{#snippet card()}
<form onsubmit={(event) => { event.preventDefault(); onform?.(); }}>
  <Composer {sessionId} {persistDraft} {value} {onsubmit} {disabled} placeholder="Message Alpha">
    {#snippet toolbar(controls)}
      <ComposerToolbar controls={observeControls(controls)} {sending} accessLabel="Full access" model="gpt-fixture" effort="high" />
    {/snippet}
  </Composer>
</form>
{/snippet}

{#if dialog}
  <Dialog open={true} label="Toolbar dialog">{@render card()}</Dialog>
{:else}
  {@render card()}
{/if}
