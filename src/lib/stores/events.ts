import { writable } from 'svelte/store';
import type { Event as SessionEvent } from '../types/domain';
import { apiUrl } from '$lib/config';

const EVENT_TYPES = [
    'session_created',
    'session_status_changed',
    'cell_created',
    'cell_status_changed',
    'workspace_created',
    'agent_launched',
    'agent_completed',
    'agent_waiting_input',
    'agent_failed',
    'artifact_updated',
    'resolver_selected_candidate',
] as const;

interface EventsState {
    events: SessionEvent[];
    loading: boolean;
    error: string | null;
}

function createEventsStore() {
    const { subscribe, set, update } = writable<EventsState>({
        events: [],
        loading: false,
        error: null,
    });

    let eventSource: EventSource | null = null;

    function prependEvent(raw: MessageEvent<string>, source: EventSource) {
        if (eventSource !== source) {
            return;
        }

        try {
            const event: SessionEvent = JSON.parse(raw.data);
            update(state => ({
                ...state,
                events: [event, ...state.events].slice(0, 1000),
            }));
        } catch (err) {
            console.error('Failed to parse event:', err);
        }
    }

    return {
        subscribe,

        connect(sessionId: string) {
            if (eventSource) {
                eventSource.close();
            }

            update(state => ({ ...state, loading: true, error: null }));

            const source = new EventSource(apiUrl(`/api/sessions/${sessionId}/stream`));
            eventSource = source;

            source.onopen = () => {
                if (eventSource !== source) {
                    return;
                }

                update(state => ({ ...state, loading: false }));
            };

            source.onmessage = (event) => {
                prependEvent(event, source);
            };
            EVENT_TYPES.forEach((eventType) => {
                source.addEventListener(eventType, (event) => {
                    prependEvent(event as MessageEvent<string>, source);
                });
            });

            source.onerror = (err) => {
                console.error('EventSource failed:', err);
                if (eventSource !== source) {
                    return;
                }

                const permanentlyClosed = source.readyState === EventSource.CLOSED;
                if (permanentlyClosed) {
                    this.disconnect();
                    update(state => ({ ...state, loading: false, error: 'Connection failed (404 or server error)' }));
                } else {
                    update(state => ({ ...state, loading: false, error: 'Connection lost, retrying...' }));
                }
            };
        },

        disconnect() {
            if (eventSource) {
                eventSource.close();
                eventSource = null;
            }
        },

        async fetchEvents(sessionId: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/events`));
                if (!response.ok) throw new Error(`Failed to fetch events: ${response.statusText}`);
                const events: SessionEvent[] = await response.json();
                
                update(state => ({
                    ...state,
                    events: [...events].reverse(), // Show newest first
                    loading: false
                }));
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        }
    };
}

export const events = createEventsStore();
