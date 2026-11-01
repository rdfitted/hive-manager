<script lang="ts">
  import Dialog from './Dialog.svelte';
  import Composer from '../composer/Composer.svelte';
</script>

<Dialog open={true} label="Composer dialog">
  <Composer sessionId={null} persistDraft={false} placeholder="Write task" />
</Dialog>
