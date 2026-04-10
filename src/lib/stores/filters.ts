import { writable, derived } from 'svelte/store';
import type { Event, EventType, Severity } from '../types/domain';
import { events } from './events';

export interface EventFilters {
    types: EventType[];
    severities: Severity[];
    cellId: string | null;
    agentId: string | null;
    searchText: string;
    startTime: string | null;
    endTime: string | null;
}

const initialFilters: EventFilters = {
    types: [],
    severities: [],
    cellId: null,
    agentId: null,
    searchText: '',
    startTime: null,
    endTime: null,
};

function createFiltersStore() {
    const { subscribe, set, update } = writable<EventFilters>(initialFilters);

    return {
        subscribe,
        set,
        update,
        reset: () => set(initialFilters),
        toggleType: (type: EventType) => update(f => ({
            ...f,
            types: f.types.includes(type) 
                ? f.types.filter(t => t !== type) 
                : [...f.types, type]
        })),
        toggleSeverity: (severity: Severity) => update(f => ({
            ...f,
            severities: f.severities.includes(severity) 
                ? f.severities.filter(s => s !== severity) 
                : [...f.severities, severity]
        })),
        setCellId: (id: string | null) => update(f => ({ ...f, cellId: id })),
        setAgentId: (id: string | null) => update(f => ({ ...f, agentId: id })),
        setSearchText: (text: string) => update(f => ({ ...f, searchText: text })),
        setTimeRange: (start: string | null, end: string | null) => update(f => ({ 
            ...f, 
            startTime: start, 
            endTime: end 
        })),
    };
}

export const filters = createFiltersStore();

export const filteredEvents = derived(
    [events, filters],
    ([$events, $filters]) => {
        return $events.events.filter(event => {
            if ($filters.types.length > 0 && !$filters.types.includes(event.event_type)) {
                return false;
            }
            if ($filters.severities.length > 0 && !$filters.severities.includes(event.severity)) {
                return false;
            }
            if ($filters.cellId && event.cell_id !== $filters.cellId) {
                return false;
            }
            if ($filters.agentId && event.agent_id !== $filters.agentId) {
                return false;
            }
            if ($filters.searchText) {
                const searchLower = $filters.searchText.toLowerCase();
                const payloadString = JSON.stringify(event.payload).toLowerCase();
                if (!payloadString.includes(searchLower) && !event.event_type.toLowerCase().includes(searchLower)) {
                    return false;
                }
            }
            if ($filters.startTime && new Date(event.timestamp) < new Date($filters.startTime)) {
                return false;
            }
            if ($filters.endTime && new Date(event.timestamp) > new Date($filters.endTime)) {
                return false;
            }
            return true;
        });
    }
);
