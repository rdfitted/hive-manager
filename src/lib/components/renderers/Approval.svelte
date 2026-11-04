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
    requesterAgentId?: string;
    destructive?: boolean;
  }

  function asApproval(value: unknown): ApprovalData {
    if (value && typeof value === 'object') return value as ApprovalData;
    return {};
  }

  const approval = $derived(asApproval(data));
  let decided = $state(false);
  let pending = $state(false);
  let error = $state('');

  async function decide(callback: ToolRendererProps['onapprove']) {
    if (decided || pending || !approval.approvalId || !callback) return;
    pending = true;
    error = '';
    try {
      await callback({ approvalId: approval.approvalId });
      decided = true;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      // The backend consumes before delivery; an ambiguous send cannot be retried.
      decided = message.startsWith('[approval_consumed] ') || /Approval is stale/.test(message);
      error = message.replace(/^\[approval_consumed\] /, '');
    } finally {
      pending = false;
    }
  }

  function approve() {
    return decide(onapprove);
  }

  function reject() {
    return decide(onreject);
  }
</script>

<div class="approval-widget lattice-forced-colors-boundary">
  <div class="approval-body">
    <div class="approval-title">{approval.title ?? 'Approval required'}</div>
    {#if approval.requesterAgentId}
      <div class="approval-desc">Requested by {approval.requesterAgentId}</div>
    {/if}
    {#if approval.description}
      <div class="approval-desc">{approval.description}</div>
    {/if}
  </div>
  {#if error}
    <div role="alert">{error}</div>
  {/if}
  <div class="approval-actions">
    <button
      type="button"
      class="lattice-btn lattice-btn--compact lattice-btn--filled {approval.destructive ? 'lattice-btn--danger' : 'lattice-btn--success'}"
      data-testid="approve"
      disabled={decided || pending || !approval.approvalId || !onapprove}
      onclick={approve}
    >
      Approve
    </button>
    <button type="button" class="lattice-btn lattice-btn--compact lattice-btn--danger" data-testid="reject" disabled={decided || pending || !approval.approvalId || !onreject} onclick={reject}>
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
