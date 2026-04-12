import { get, writable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import type { Cell } from '../types/domain';
import { apiUrl } from '$lib/config';
import { sessions } from './sessions';

interface CellsState {
    cells: Record<string, Cell>; // cell_id -> Cell
    loading: boolean;
    error: string | null;
    sessionNotFound: boolean;
}

interface CellUpdatedEvent {
    session_id: string;
    cells?: Cell[];
}

function mergeCells(existing: Record<string, Cell>, incoming: Cell[]): Record<string, Cell> {
    return {
        ...existing,
        ...Object.fromEntries(incoming.map((cell) => [cell.id, cell])),
    };
}

function createCellsStore() {
    const { subscribe, update } = writable<CellsState>({
        cells: {},
        loading: false,
        error: null,
        sessionNotFound: false,
    });
    let onExternalRefresh: ((sessionId: string) => void) | null = null;

    void listen<CellUpdatedEvent>('cell-updated', (event) => {
        const current = get(sessions).activeSessionId;
        const updatedCells = event.payload.cells;
        if (event.payload.session_id === current) {
            onExternalRefresh?.(event.payload.session_id);
            if (updatedCells) {
                update((state) => ({
                    ...state,
                    cells: mergeCells(state.cells, updatedCells),
                    error: null,
                }));
            } else {
                void cells.fetchCells(event.payload.session_id);
            }
        }
    });

    return {
        subscribe,

        async fetchCells(sessionId: string) {
            update((state) => ({ ...state, loading: true, error: null, sessionNotFound: false }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells`));

                if (response.status === 404) {
                    update((state) => ({
                        ...state,
                        cells: {},
                        loading: false,
                        sessionNotFound: true,
                    }));
                    return;
                }

                if (!response.ok) throw new Error(`Failed to fetch cells: ${response.statusText}`);
                const cells: Cell[] = await response.json();

                update((state) => ({
                    ...state,
                    cells: mergeCells(state.cells, cells),
                    loading: false,
                    sessionNotFound: false,
                }));
            } catch (err) {
                update((state) => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        async fetchCell(sessionId: string, cellId: string) {
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells/${cellId}`));
                if (!response.ok) throw new Error(`Failed to fetch cell: ${response.statusText}`);
                const cell: Cell = await response.json();
                
                update((state) => ({
                    ...state,
                    cells: { ...state.cells, [cell.id]: cell },
                }));
            } catch (err) {
                update((state) => ({ ...state, error: (err as Error).message }));
            }
        },

        updateCell(cell: Cell) {
            update((state) => ({
                ...state,
                cells: { ...state.cells, [cell.id]: cell },
            }));
        },

        setExternalRefreshHandler(handler: ((sessionId: string) => void) | null) {
            onExternalRefresh = handler;
        },
    };
}

export const cells = createCellsStore();
