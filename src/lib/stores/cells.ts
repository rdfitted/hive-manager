import { writable, get } from 'svelte/store';
import type { Cell } from '../types/domain';
import { apiUrl } from '$lib/config';
import { sessions } from './sessions';

interface CellsState {
    cells: Record<string, Cell>; // cell_id -> Cell
    loading: boolean;
    error: string | null;
}

function createCellsStore() {
    const { subscribe, set, update } = writable<CellsState>({
        cells: {},
        loading: false,
        error: null,
    });

    return {
        subscribe,

        async fetchCells(sessionId: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells`));
                if (!response.ok) throw new Error(`Failed to fetch cells: ${response.statusText}`);
                const cells: Cell[] = await response.json();
                
                update(state => {
                    const newCells = { ...state.cells };
                    cells.forEach(cell => {
                        newCells[cell.id] = cell;
                    });
                    return { ...state, cells: newCells, loading: false };
                });
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        async fetchCell(sessionId: string, cellId: string) {
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells/${cellId}`));
                if (!response.ok) throw new Error(`Failed to fetch cell: ${response.statusText}`);
                const cell: Cell = await response.json();
                
                update(state => ({
                    ...state,
                    cells: { ...state.cells, [cell.id]: cell }
                }));
            } catch (err) {
                update(state => ({ ...state, error: (err as Error).message }));
            }
        },

        updateCell(cell: Cell) {
            update(state => ({
                ...state,
                cells: { ...state.cells, [cell.id]: cell }
            }));
        }
    };
}

export const cells = createCellsStore();
