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

const MAX_PAYLOAD_SEARCH_CACHE_ENTRIES = 1000;
const payloadSearchCache = new Map<string, string>();

function getPayloadSearchText(event: Event): string {
    const cached = payloadSearchCache.get(event.id);
    if (cached !== undefined) {
        return cached;
    }

    const serializedPayload =
        typeof event.payload === 'string'
            ? event.payload
            : JSON.stringify(event.payload ?? null);
    const normalizedPayload = serializedPayload.toLowerCase();
    payloadSearchCache.set(event.id, normalizedPayload);
    if (payloadSearchCache.size > MAX_PAYLOAD_SEARCH_CACHE_ENTRIES) {
        const oldestKey = payloadSearchCache.keys().next().value;
        if (oldestKey !== undefined) {
            payloadSearchCache.delete(oldestKey);
        }
    }
    return normalizedPayload;
}

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
        const searchLower = $filters.searchText.trim().toLowerCase();

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
            if (searchLower) {
                const matchesSearch =
                    event.event_type.toLowerCase().includes(searchLower) ||
                    getPayloadSearchText(event).includes(searchLower);
                if (!matchesSearch) {
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
