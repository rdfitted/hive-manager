import { writable } from 'svelte/store';
import type { Event } from '../types/domain';

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
    events: Event[];
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

    function prependEvent(raw: MessageEvent<string>) {
        try {
            const event: Event = JSON.parse(raw.data);
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

            eventSource = new EventSource(`http://localhost:18800/api/sessions/${sessionId}/stream`);

            eventSource.onopen = () => {
                update(state => ({ ...state, loading: false }));
            };

            eventSource.onmessage = prependEvent;
            EVENT_TYPES.forEach((eventType) => {
                eventSource?.addEventListener(eventType, prependEvent as EventListener);
            });

            eventSource.onerror = (err) => {
                console.error('EventSource failed:', err);
                update(state => ({ ...state, loading: false, error: 'Connection lost' }));
                if (eventSource) {
                    eventSource.close();
                    eventSource = null;
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
                const response = await fetch(`http://localhost:18800/api/sessions/${sessionId}/events`);
                if (!response.ok) throw new Error(`Failed to fetch events: ${response.statusText}`);
                const events: Event[] = await response.json();
                
                update(state => ({
                    ...state,
                    events: events.reverse(), // Show newest first
                    loading: false
                }));
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        }
    };
}

export const events = createEventsStore();
