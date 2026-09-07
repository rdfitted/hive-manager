<script lang="ts">
  import type { AgentConfig } from '$lib/stores/sessions';
  import AgentConfigEditor from './AgentConfigEditor.svelte';

  export let config: AgentConfig;
  export let idPrefix: string;
  export let showLabel = true;

  let changeCount = 0;

  function handleChange(event: CustomEvent<AgentConfig>): void {
    config = event.detail;
    changeCount += 1;
  }
</script>

<AgentConfigEditor
  {config}
  {idPrefix}
  {showLabel}
  on:change={handleChange}
/>
<output data-testid="config">{JSON.stringify(config)}</output>
<output data-testid="change-count">{changeCount}</output>
