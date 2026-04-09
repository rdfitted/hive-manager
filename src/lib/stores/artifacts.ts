import { writable } from 'svelte/store';
import type { ArtifactBundle, ResolverOutput } from '../types/domain';
import { apiUrl } from '$lib/config';

interface ArtifactsState {
    artifacts: Record<string, ArtifactBundle[]>; // cell_id -> artifacts
    resolverOutputs: Record<string, ResolverOutput>; // session_id -> resolver output
    artifactsLoading: Record<string, boolean>;
    artifactsError: Record<string, string | null>;
    resolverLoading: Record<string, boolean>;
    resolverError: Record<string, string | null>;
}

function createArtifactsStore() {
    const { subscribe, set, update } = writable<ArtifactsState>({
        artifacts: {},
        resolverOutputs: {},
        artifactsLoading: {},
        artifactsError: {},
        resolverLoading: {},
        resolverError: {},
    });

    return {
        subscribe,

        async fetchArtifacts(sessionId: string, cellId: string) {
            update(state => ({
                ...state,
                artifactsLoading: { ...state.artifactsLoading, [cellId]: true },
                artifactsError: { ...state.artifactsError, [cellId]: null },
            }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells/${cellId}/artifacts`));
                if (!response.ok) throw new Error(`Failed to fetch artifacts: ${response.statusText}`);
                const artifacts: ArtifactBundle[] = await response.json();
                
                update(state => ({
                    ...state,
                    artifacts: { ...state.artifacts, [cellId]: artifacts },
                    artifactsLoading: { ...state.artifactsLoading, [cellId]: false },
                }));
            } catch (err) {
                update(state => ({
                    ...state,
                    artifactsLoading: { ...state.artifactsLoading, [cellId]: false },
                    artifactsError: { ...state.artifactsError, [cellId]: (err as Error).message },
                }));
            }
        },

        async fetchResolverOutput(sessionId: string) {
            update(state => ({
                ...state,
                resolverLoading: { ...state.resolverLoading, [sessionId]: true },
                resolverError: { ...state.resolverError, [sessionId]: null },
            }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/resolver`));
                if (response.status === 404) {
                    update(state => ({
                        ...state,
                        resolverLoading: { ...state.resolverLoading, [sessionId]: false },
                    }));
                    return null;
                }
                if (!response.ok) throw new Error(`Failed to fetch resolver output: ${response.statusText}`);
                const output: ResolverOutput = await response.json();
                
                update(state => ({
                    ...state,
                    resolverOutputs: { ...state.resolverOutputs, [sessionId]: output },
                    resolverLoading: { ...state.resolverLoading, [sessionId]: false },
                }));
                return output;
            } catch (err) {
                update(state => ({
                    ...state,
                    resolverLoading: { ...state.resolverLoading, [sessionId]: false },
                    resolverError: { ...state.resolverError, [sessionId]: (err as Error).message },
                }));
                return null;
            }
        },

        updateArtifact(cellId: string, artifact: ArtifactBundle[]) {
            update(state => ({
                ...state,
                artifacts: { ...state.artifacts, [cellId]: artifact }
            }));
        },

        updateResolverOutput(sessionId: string, output: ResolverOutput) {
            update(state => ({
                ...state,
                resolverOutputs: { ...state.resolverOutputs, [sessionId]: output }
            }));
        }
    };
}

export const artifacts = createArtifactsStore();
