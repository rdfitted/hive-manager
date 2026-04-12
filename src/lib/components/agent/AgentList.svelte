<script lang="ts">
    import { Bug, Crown, MagnifyingGlass, Scales, TestTube } from 'phosphor-svelte';
    import { agents } from '../../stores/agents';
    import { ui } from '../../stores/ui';
    import AgentStatusBadge from './AgentStatusBadge.svelte';
    import type { Agent } from '../../types/domain';

    export let agentIds: string[];

    $: cellAgents = agentIds.map(id => $agents.agents[id]).filter(Boolean) as Agent[];

    function selectAgent(id: string) {
        ui.setSelectedAgent(id);
    }
</script>

<div class="agent-list">
    {#each cellAgents as agent (agent.id)}
        <button 
            class="agent-item" 
            class:selected={$ui.selectedAgentId === agent.id}
            on:click={() => selectAgent(agent.id)}
        >
            <div class="role-icon" title={agent.role}>
                {#if agent.role === 'queen'}
                    <Crown size={16} weight="light" />
                {:else if agent.role === 'worker'}
                    <Bug size={16} weight="light" />
                {:else if agent.role === 'resolver'}
                    <Scales size={16} weight="light" />
                {:else if agent.role === 'reviewer'}
                    <MagnifyingGlass size={16} weight="light" />
                {:else if agent.role === 'tester'}
                    <TestTube size={16} weight="light" />
                {/if}
            </div>
            <div class="details">
                <span class="label">{agent.label}</span>
                <AgentStatusBadge status={agent.status} />
            </div>
        </button>
    {:else}
        <div class="empty">No agents in this cell</div>
    {/each}
</div>

<style>
    .agent-list {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .agent-item {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 6px 8px;
        background: rgba(255, 255, 255, 0.03);
        border: 1px solid transparent;
        border-radius: var(--radius-sm);
        text-align: left;
        cursor: pointer;
        transition: all 0.2s;
        color: inherit;
    }

    .agent-item:hover {
        background: rgba(255, 255, 255, 0.08);
    }

    .agent-item.selected {
        background: rgba(59, 130, 246, 0.15);
        border-color: rgba(59, 130, 246, 0.5);
    }

    .role-icon {
        width: 24px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.2);
        border-radius: 50%;
    }

    .details {
        display: flex;
        flex-direction: column;
        flex: 1;
    }

    .label {
        font-weight: 500;
        font-size: 12px;
    }

    .empty {
        font-size: 11px;
        color: var(--text-secondary);
        padding: 4px;
        font-style: italic;
    }
</style>
