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

<div class="approval-widget" class:destructive={approval.destructive}>
  <div class="approval-body">
    <div class="approval-title">{approval.title ?? 'Approval required'}</div>
    {#if approval.description}
      <div class="approval-desc">{approval.description}</div>
    {/if}
  </div>
  <div class="approval-actions">
    <button class="approve-btn" data-testid="approve" onclick={approve}>
      Approve
    </button>
    <button class="reject-btn" data-testid="reject" onclick={reject}>
      Reject
    </button>
  </div>
</div>

<style>
  .approval-widget {
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    border: 1px solid var(--border-structural);
    border-left: 3px solid var(--accent-cyan);
    border-radius: var(--radius-sm);
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

  .approve-btn,
  .reject-btn {
    padding: 5px 14px;
    font-size: 12px;
    font-weight: 600;
    border: none;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .approve-btn {
    background: var(--status-success);
    color: white;
  }

  .destructive .approve-btn {
    background: var(--status-error);
  }

  .reject-btn {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    color: var(--text-secondary);
  }

  .approve-btn:hover {
    opacity: 0.9;
  }

  .reject-btn:hover {
    color: var(--text-primary);
    border-color: var(--text-secondary);
  }
</style>
