<script lang="ts">
    import type { Workspace } from '../../types/domain';

    export let workspace: Workspace;
</script>

<div class="workspace-badge" title={workspace.worktree_path || workspace.repo_path}>
    <span class="strategy">{workspace.strategy}</span>
    <span class="branch">
        <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3v12"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>
        {workspace.branch_name}
    </span>
    {#if workspace.is_dirty}
        <span class="dirty-dot" title="Uncommitted changes"></span>
    {/if}
</div>

<style>
    .workspace-badge {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        background: rgba(255, 255, 255, 0.05);
        padding: 2px 8px;
        border-radius: 4px;
        font-family: var(--font-mono);
        font-size: 11px;
        border: 1px solid rgba(255, 255, 255, 0.1);
    }

    .strategy {
        color: #888;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        font-size: 9px;
    }

    .branch {
        display: flex;
        align-items: center;
        gap: 4px;
        color: #ddd;
    }

    .dirty-dot {
        width: 6px;
        height: 6px;
        background: #f59e0b;
        border-radius: 50%;
    }
</style>
