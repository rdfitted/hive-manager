import { writable } from 'svelte/store';
import type { Event } from '../types/domain';

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

    return {
        subscribe,

        connect(sessionId: string) {
            if (eventSource) {
                eventSource.close();
            }

            update(state => ({ ...state, loading: true, error: null }));
            
            eventSource = new EventSource(`http://localhost:18800/api/sessions/${sessionId}/events/stream`);

            eventSource.onopen = () => {
                update(state => ({ ...state, loading: false }));
            };

            eventSource.onmessage = (msg) => {
                try {
                    const event: Event = JSON.parse(msg.data);
                    update(state => ({
                        ...state,
                        events: [event, ...state.events].slice(0, 1000) // Keep last 1000 events
                    }));
                } catch (err) {
                    console.error('Failed to parse event:', err);
                }
            };

            eventSource.onerror = (err) => {
                console.error('EventSource failed:', err);
                update(state => ({ ...state, error: 'Connection lost' }));
                if (eventSource) eventSource.close();
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
