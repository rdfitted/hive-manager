import { writable } from 'svelte/store';
import type { Agent } from '../types/domain';
import { apiUrl } from '$lib/config';

interface AgentsState {
    agents: Record<string, Agent>; // agent_id -> Agent
    loading: boolean;
    error: string | null;
}

function createAgentsStore() {
    const { subscribe, set, update } = writable<AgentsState>({
        agents: {},
        loading: false,
        error: null,
    });

    return {
        subscribe,

        async fetchAgents(sessionId: string, cellId: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells/${cellId}/agents`));
                if (!response.ok) throw new Error(`Failed to fetch agents: ${response.statusText}`);
                const agents: Agent[] = await response.json();
                
                update(state => {
                    const newAgents = { ...state.agents };
                    agents.forEach(agent => {
                        newAgents[agent.id] = agent;
                    });
                    return { ...state, agents: newAgents, loading: false };
                });
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        updateAgent(agent: Agent) {
            update(state => ({
                ...state,
                agents: { ...state.agents, [agent.id]: agent }
            }));
        }
    };
}

export const agents = createAgentsStore();
