<script lang="ts">
  /**
   * Approval tool-render widget.
   *
   * data holds a server-issued approvalId. Legacy actionId-only cards are inert.
   * The backend binds the ID to a pending request and makes the decision single-use.
   */
  import type { ToolRendererProps } from './registry';

  let { data, onapprove, onreject }: ToolRendererProps = $props();

  interface ApprovalData {
    title?: string;
    description?: string;
    actionId?: string;
    approvalId?: string;
    destructive?: boolean;
  }

  function asApproval(value: unknown): ApprovalData {
    if (value && typeof value === 'object') return value as ApprovalData;
    return {};
  }

  const approval = $derived(asApproval(data));
  let decided = $state(false);

  function approve() {
    if (decided || !approval.approvalId || !onapprove) return;
    decided = true;
    onapprove({ approvalId: approval.approvalId });
  }

  function reject() {
    if (decided || !approval.approvalId || !onreject) return;
    decided = true;
    onreject({ approvalId: approval.approvalId });
  }
</script>

<div class="approval-widget lattice-forced-colors-boundary">
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
      disabled={decided || !approval.approvalId || !onapprove}
      onclick={approve}
    >
      Approve
    </button>
    <button type="button" class="lattice-btn lattice-btn--compact lattice-btn--danger" data-testid="reject" disabled={decided || !approval.approvalId || !onreject} onclick={reject}>
      Reject
    </button>
  </div>
</div>

<style>
  .approval-widget {
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--surface-raised);
    box-shadow: var(--edge-lip);
    border-radius: var(--radius-lg);
    padding: 12px;
  }

  .approval-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .approval-desc {
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 4px;
    line-height: 1.5;
  }

  .approval-actions {
    display: flex;
    gap: 8px;
  }

</style>
