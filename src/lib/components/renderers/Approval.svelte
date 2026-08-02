<script lang="ts">
  /**
   * Approval tool-render widget.
   *
   * data shape: { title: string; description?: string; actionId?: string; destructive?: boolean }
   *
   * Invokes the optional `onapprove` / `onreject` callback props with
   * { actionId } (Svelte 5 runes idiom). Wiring these to the queen/worker is OUT
   * OF SCOPE for #127 (no general "approve tool result" endpoint exists; that
   * belongs to #123's Action contract). For now ToolRenderHost re-emits them as
   * Svelte events and ConversationViewer console-logs them, so the widget and
   * the future contract are independently testable.
   */
  import type { ToolRendererProps } from './registry';

  let { data, onapprove, onreject }: ToolRendererProps = $props();

  interface ApprovalData {
    title?: string;
    description?: string;
    actionId?: string;
    destructive?: boolean;
  }

  function asApproval(value: unknown): ApprovalData {
    if (value && typeof value === 'object') return value as ApprovalData;
    return {};
  }

  const approval = $derived(asApproval(data));

  function approve() {
    onapprove?.({ actionId: approval.actionId });
  }

  function reject() {
    onreject?.({ actionId: approval.actionId });
  }
</script>

<div class="approval-widget lattice-forced-colors-boundary" class:destructive={approval.destructive}>
  <div class="approval-body">
    <div class="approval-title">{approval.title ?? 'Approval required'}</div>
    {#if approval.description}
      <div class="approval-desc">{approval.description}</div>
    {/if}
  </div>
  <div class="approval-actions">
    <button
      type="button"
      class="lattice-btn lattice-btn--compact lattice-btn--filled {approval.destructive ? 'lattice-btn--danger' : 'lattice-btn--success'}"
      data-testid="approve"
      onclick={approve}
    >
      Approve
    </button>
    <button type="button" class="lattice-btn lattice-btn--compact lattice-btn--danger" data-testid="reject" onclick={reject}>
      Reject
    </button>
  </div>
</div>

<style>
  .approval-widget {
    display: flex;
    flex-direction: column;
    gap: 10px;
    /* Tool results are nested work products, so their body uses the shared sunken tone. */
    background: var(--bg-sunken);
    box-shadow: inset 0 0 0 1px var(--border-structural);
    border-left: 3px solid var(--accent-cyan);
    border-radius: var(--radius-lg);
    padding: 12px;
  }

  .approval-widget.destructive {
    border-left-color: var(--status-error);
  }

  .approval-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .approval-desc {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: 4px;
    line-height: 1.5;
  }

  .approval-actions {
    display: flex;
    gap: 8px;
  }

</style>
