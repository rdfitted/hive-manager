import { writable } from 'svelte/store';
import type { ArtifactBundle } from '../types/domain';
import { apiUrl } from '$lib/config';

interface ArtifactsState {
    artifacts: Record<string, ArtifactBundle[]>; // cell_id -> artifacts
    loading: boolean;
    error: string | null;
}

function createArtifactsStore() {
    const { subscribe, set, update } = writable<ArtifactsState>({
        artifacts: {},
        loading: false,
        error: null,
    });

    return {
        subscribe,

        async fetchArtifacts(sessionId: string, cellId: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells/${cellId}/artifacts`));
                if (!response.ok) throw new Error(`Failed to fetch artifacts: ${response.statusText}`);
                const artifacts: ArtifactBundle[] = await response.json();
                
                update(state => ({
                    ...state,
                    artifacts: { ...state.artifacts, [cellId]: artifacts },
                    loading: false
                }));
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        updateArtifact(cellId: string, artifact: ArtifactBundle[]) {
            update(state => ({
                ...state,
                artifacts: { ...state.artifacts, [cellId]: artifact }
            }));
        }
    };
}

export const artifacts = createArtifactsStore();
